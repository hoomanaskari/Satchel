//! The one place Satchel talks to the network: a small HTTPS client for
//! link previews (ureq + rustls, no cookies, no proxy, no JavaScript).
//!
//! Limits (see `docs/ARCHITECTURE.md`, "Link previews"):
//! - `https` only; plain `http` is upgraded by `link::fetch_target` and a
//!   redirect to `http` is refused.
//! - Every resolved address must be public (`link::is_public_ip`), checked
//!   in the resolver, so the connection goes to exactly the address that was
//!   checked (no DNS rebinding between check and connect). Every redirect
//!   hop goes through `link::redirect_target` (same checks as the copied
//!   link). At most `MAX_REDIRECTS`.
//! - 10 s in total per page (all redirects included) and per image, 5 s to
//!   connect.
//! - Pages: `text/html` or `application/xhtml+xml`, first 512 KiB only.
//!   Images: `image/*` except SVG, at most 5 MiB, and one of the formats
//!   `platform::is_web_image` allows.
//! - No cookies (ureq's cookie store is not compiled in), no stored
//!   credentials, no `Referer`, no `Accept-Language`, and the generic
//!   `Twitterbot/1.0` crawler user agent, which identifies neither Satchel
//!   nor this Mac.
//! - Environment proxies (`HTTP_PROXY`...) are ignored, so a variable set by
//!   some other tool cannot reroute previews.

use std::io::Read;
use std::net::IpAddr;
use std::time::{Duration, Instant};

use satchel_core::link;
use ureq::Agent;
use ureq::config::Config;
use ureq::http::{Response, StatusCode, header};
use ureq::unversioned::resolver::{DefaultResolver, ResolvedSocketAddrs, Resolver};
use ureq::unversioned::transport::{DefaultConnector, NextTimeout};
use satchel_core::link::Url;

use super::{FetchError, Fetcher, Page};

pub const MAX_REDIRECTS: usize = 5;
pub const PAGE_TIMEOUT: Duration = Duration::from_secs(10);
pub const IMAGE_TIMEOUT: Duration = Duration::from_secs(10);
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
pub const MAX_IMAGE_BYTES: u64 = 5 * 1024 * 1024;
/// A widely used link-preview crawler's user agent. Sites answer it with a
/// small page whose `<head>` carries the card metadata. Measured on YouTube,
/// GitHub, BBC, the Guardian, Apple, Wikipedia, the New York Times, Reddit
/// and Amazon: all gave their title in the first 26 KB. A Safari user agent
/// got YouTube's 1.4 MB page (title past the 512 KiB cap) and 403s from
/// news sites; `facebookexternalhit` got a 403 from Wikipedia; the Messages
/// previewer's combined string got the channel name as YouTube's title.
pub const USER_AGENT: &str = "Twitterbot/1.0";

/// Resolves with the system resolver and refuses the host if any address
/// is not allowed, so a name that resolves to a private address (or to a
/// mix) is never connected to.
#[derive(Debug)]
struct GuardedResolver {
    allow: fn(IpAddr) -> bool,
}

impl Resolver for GuardedResolver {
    fn resolve(&self, uri: &ureq::http::Uri, config: &Config, timeout: NextTimeout) -> Result<ResolvedSocketAddrs, ureq::Error> {
        let addrs = DefaultResolver::default().resolve(uri, config, timeout)?;
        if addrs.is_empty() || addrs.iter().any(|a| !(self.allow)(a.ip())) {
            return Err(ureq::Error::Io(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "address not allowed")));
        }
        Ok(addrs)
    }
}

pub struct HttpFetcher {
    agent: Agent,
    /// Only tests turn this off (a local plain-HTTP server).
    require_https: bool,
}

impl HttpFetcher {
    pub fn new() -> Self {
        Self::build(link::is_public_ip, true)
    }

    fn build(allow: fn(IpAddr) -> bool, require_https: bool) -> Self {
        let config = Agent::config_builder()
            .proxy(None)
            .https_only(require_https)
            .max_redirects(0)
            .http_status_as_error(false)
            .save_redirect_history(false)
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .timeout_global(Some(PAGE_TIMEOUT))
            .max_response_header_size(64 * 1024)
            .user_agent(USER_AGENT)
            .accept("text/html,application/xhtml+xml;q=0.9,*/*;q=0.1")
            .max_idle_connections(4)
            .max_idle_connections_per_host(1)
            .build();
        let agent = Agent::with_parts(config, DefaultConnector::new(), GuardedResolver { allow });
        Self { agent, require_https }
    }

    fn get(&self, url: &Url, timeout: Duration) -> Result<Response<ureq::Body>, FetchError> {
        if self.require_https && url.scheme() != "https" {
            return Err(FetchError::Blocked);
        }
        self.agent
            .get(url.as_str())
            .config()
            .timeout_global(Some(timeout))
            .build()
            .call()
            .map_err(classify)
    }
}

impl Default for HttpFetcher {
    fn default() -> Self {
        Self::new()
    }
}

fn classify(e: ureq::Error) -> FetchError {
    match e {
        ureq::Error::Io(io) if io.kind() == std::io::ErrorKind::PermissionDenied => FetchError::Blocked,
        ureq::Error::RequireHttpsOnly(_) => FetchError::Blocked,
        ureq::Error::Io(_)
        | ureq::Error::Timeout(_)
        | ureq::Error::HostNotFound
        | ureq::Error::ConnectionFailed
        | ureq::Error::BodyStalled => FetchError::Transient,
        _ => FetchError::Permanent,
    }
}

fn status_error(status: StatusCode) -> Option<FetchError> {
    if status.is_success() {
        return None;
    }
    Some(match status.as_u16() {
        408 | 425 | 429 | 500..=599 => FetchError::Transient,
        _ => FetchError::Permanent,
    })
}

fn content_type(resp: &Response<ureq::Body>) -> String {
    resp.headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn remaining(start: Instant, total: Duration) -> Result<Duration, FetchError> {
    total.checked_sub(start.elapsed()).filter(|d| !d.is_zero()).ok_or(FetchError::Transient)
}

impl Fetcher for HttpFetcher {
    fn page(&self, url: &Url) -> Result<Page, FetchError> {
        let start = Instant::now();
        let mut current = url.clone();
        let mut redirects = 0;
        let mut resp = loop {
            let resp = self.get(&current, remaining(start, PAGE_TIMEOUT)?)?;
            if !resp.status().is_redirection() {
                break resp;
            }
            redirects += 1;
            if redirects > MAX_REDIRECTS {
                return Err(FetchError::Permanent);
            }
            let location = resp
                .headers()
                .get(header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or(FetchError::Permanent)?;
            current = if self.require_https {
                link::redirect_target(&current, location).map_err(|_| FetchError::Blocked)?
            } else {
                current.join(location).map_err(|_| FetchError::Permanent)?
            };
        };
        if let Some(e) = status_error(resp.status()) {
            return Err(e);
        }
        let ty = content_type(&resp);
        let mime = ty.split(';').next().unwrap_or_default().trim();
        if mime != "text/html" && mime != "application/xhtml+xml" {
            return Err(FetchError::Permanent);
        }
        let charset = link::charset_param(&ty);
        let mut body = Vec::new();
        resp.body_mut()
            .as_reader()
            .take(link::MAX_HTML_BYTES as u64)
            .read_to_end(&mut body)
            .map_err(|_| FetchError::Transient)?;
        Ok(Page { url: current, body, charset })
    }

    fn image(&self, url: &Url) -> Result<Vec<u8>, FetchError> {
        // No redirects for images: CDNs serve them directly, and every hop
        // would be one more host learning about the page.
        let mut resp = self.get(url, IMAGE_TIMEOUT)?;
        if let Some(e) = status_error(resp.status()) {
            return Err(e);
        }
        if resp.status().is_redirection() {
            return Err(FetchError::Permanent);
        }
        let ty = content_type(&resp);
        let ok_type = ty.is_empty()
            || (ty.starts_with("image/") && !ty.contains("svg"))
            || ty.starts_with("application/octet-stream");
        if !ok_type {
            return Err(FetchError::Permanent);
        }
        let mut bytes = Vec::new();
        resp.body_mut()
            .as_reader()
            .take(MAX_IMAGE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| FetchError::Transient)?;
        if bytes.len() as u64 > MAX_IMAGE_BYTES || !crate::platform::is_web_image(&bytes) {
            return Err(FetchError::Permanent);
        }
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    /// A one-thread HTTP/1.1 server on loopback that answers each request
    /// from `respond(path)` and records the request heads.
    struct Server {
        port: u16,
        requests: Arc<Mutex<Vec<String>>>,
    }

    impl Server {
        fn start(respond: impl Fn(&str) -> Vec<u8> + Send + 'static) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            let requests = Arc::new(Mutex::new(Vec::new()));
            let log = requests.clone();
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(mut stream) = stream else { continue };
                    let mut head = Vec::new();
                    let mut byte = [0u8; 1];
                    while !head.ends_with(b"\r\n\r\n") && stream.read(&mut byte).map(|n| n == 1).unwrap_or(false) {
                        head.push(byte[0]);
                    }
                    let head = String::from_utf8_lossy(&head).into_owned();
                    let path = head.split_whitespace().nth(1).unwrap_or("/").to_string();
                    log.lock().unwrap().push(head);
                    let _ = stream.write_all(&respond(&path));
                    let _ = stream.flush();
                }
            });
            Self { port, requests }
        }
        fn url(&self, path: &str) -> Url {
            Url::parse(&format!("http://127.0.0.1:{}{path}", self.port)).unwrap()
        }
        fn heads(&self) -> Vec<String> {
            self.requests.lock().unwrap().clone()
        }
    }

    fn response(status: &str, headers: &[(&str, &str)], body: &[u8]) -> Vec<u8> {
        let mut out = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n", body.len());
        for (k, v) in headers {
            out.push_str(&format!("{k}: {v}\r\n"));
        }
        out.push_str("\r\n");
        let mut bytes = out.into_bytes();
        bytes.extend_from_slice(body);
        bytes
    }

    fn html(body: &str) -> Vec<u8> {
        response("200 OK", &[("Content-Type", "text/html; charset=ISO-8859-1")], body.as_bytes())
    }

    /// Allows loopback and plain HTTP, for the local test server only.
    fn local() -> HttpFetcher {
        HttpFetcher::build(|_| true, false)
    }

    #[test]
    fn the_real_fetcher_refuses_loopback_and_plain_http() {
        let server = Server::start(|_| html("<title>x</title>"));
        let real = HttpFetcher::new();
        assert_eq!(real.page(&server.url("/")).unwrap_err(), FetchError::Blocked, "plain http");
        // A name that resolves to loopback, over https: refused by the
        // resolver before any connection.
        let https = Url::parse(&format!("https://localhost:{}/", server.port)).unwrap();
        assert_eq!(real.page(&https).unwrap_err(), FetchError::Blocked);
        let guarded = HttpFetcher::build(link::is_public_ip, false);
        assert_eq!(guarded.page(&server.url("/")).unwrap_err(), FetchError::Blocked, "resolved to loopback");
        assert!(server.heads().is_empty(), "no request reached the server");
    }

    #[test]
    fn reads_a_page_with_a_generic_identity_and_no_cookies() {
        let server = Server::start(|_| {
            response(
                "200 OK",
                &[("Content-Type", "text/html; charset=utf-8"), ("Set-Cookie", "track=1; Path=/")],
                b"<title>Hello</title>",
            )
        });
        let f = local();
        let page = f.page(&server.url("/a")).unwrap();
        assert_eq!(page.body, b"<title>Hello</title>");
        assert_eq!(page.charset.as_deref(), Some("utf-8"));
        f.page(&server.url("/b")).unwrap();
        let heads = server.heads();
        assert_eq!(heads.len(), 2);
        for h in &heads {
            let lower = h.to_ascii_lowercase();
            assert!(lower.contains(&format!("user-agent: {}", USER_AGENT.to_ascii_lowercase())), "{h}");
            assert!(!lower.contains("cookie:"), "cookie sent: {h}");
            assert!(!lower.contains("referer:"), "{h}");
            assert!(!lower.contains("accept-language:"), "{h}");
            assert!(!lower.contains("authorization:"), "{h}");
        }
    }

    #[test]
    fn non_html_and_errors_are_classified() {
        let server = Server::start(|path| match path {
            "/pdf" => response("200 OK", &[("Content-Type", "application/pdf")], b"%PDF"),
            "/404" => response("404 Not Found", &[("Content-Type", "text/html")], b""),
            "/503" => response("503 Service Unavailable", &[], b""),
            "/429" => response("429 Too Many Requests", &[], b""),
            _ => html("ok"),
        });
        let f = local();
        assert_eq!(f.page(&server.url("/pdf")).unwrap_err(), FetchError::Permanent);
        assert_eq!(f.page(&server.url("/404")).unwrap_err(), FetchError::Permanent);
        assert_eq!(f.page(&server.url("/503")).unwrap_err(), FetchError::Transient);
        assert_eq!(f.page(&server.url("/429")).unwrap_err(), FetchError::Transient);
    }

    #[test]
    fn redirects_are_followed_a_few_times_only() {
        let server = Server::start(|path| {
            if let Some(n) = path.strip_prefix("/hop/") {
                let n: u32 = n.parse().unwrap();
                if n == 0 {
                    return html("<title>Arrived</title>");
                }
                return response("302 Found", &[("Location", &format!("/hop/{}", n - 1))], b"");
            }
            html("")
        });
        let f = local();
        let page = f.page(&server.url("/hop/3")).unwrap();
        assert_eq!(page.url.path(), "/hop/0");
        assert_eq!(page.body, b"<title>Arrived</title>");
        assert_eq!(f.page(&server.url(&format!("/hop/{}", MAX_REDIRECTS + 1))).unwrap_err(), FetchError::Permanent);
    }

    #[test]
    fn pages_are_cut_at_the_size_cap() {
        let big = format!("<title>Big</title>{}", "x".repeat(2 * link::MAX_HTML_BYTES));
        let server = Server::start(move |_| html(&big));
        let page = local().page(&server.url("/")).unwrap();
        assert_eq!(page.body.len(), link::MAX_HTML_BYTES);
    }

    #[test]
    fn a_slow_server_times_out() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let held: Vec<_> = listener.incoming().take(1).collect();
            std::thread::sleep(PAGE_TIMEOUT + Duration::from_secs(3));
            drop(held);
        });
        let started = Instant::now();
        let url = Url::parse(&format!("http://127.0.0.1:{port}/")).unwrap();
        assert_eq!(local().page(&url).unwrap_err(), FetchError::Transient);
        assert!(started.elapsed() < PAGE_TIMEOUT + Duration::from_secs(2));
    }

    #[test]
    fn images_are_typed_sniffed_and_capped() {
        let png = b"\x89PNG\r\n\x1a\nrest".to_vec();
        let png2 = png.clone();
        let server = Server::start(move |path| match path {
            "/ok.png" => response("200 OK", &[("Content-Type", "image/png")], &png2),
            "/svg" => response("200 OK", &[("Content-Type", "image/svg+xml")], b"<svg/>"),
            "/lies.png" => response("200 OK", &[("Content-Type", "image/png")], b"<html>"),
            "/huge" => {
                let mut b = b"\x89PNG\r\n\x1a\n".to_vec();
                b.resize(MAX_IMAGE_BYTES as usize + 10, 0);
                response("200 OK", &[("Content-Type", "image/png")], &b)
            }
            "/moved" => response("301 Moved", &[("Location", "/ok.png")], b""),
            _ => response("404 Not Found", &[], b""),
        });
        let f = local();
        assert_eq!(f.image(&server.url("/ok.png")).unwrap(), png);
        assert_eq!(f.image(&server.url("/svg")).unwrap_err(), FetchError::Permanent);
        assert_eq!(f.image(&server.url("/lies.png")).unwrap_err(), FetchError::Permanent);
        assert_eq!(f.image(&server.url("/huge")).unwrap_err(), FetchError::Permanent);
        assert_eq!(f.image(&server.url("/moved")).unwrap_err(), FetchError::Permanent);
    }
}
