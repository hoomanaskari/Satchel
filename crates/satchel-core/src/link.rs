//! Link previews: which copied links Satchel may fetch, which addresses it
//! may connect to, and what it reads from the page.
//!
//! Everything here is pure and conservative. A link preview sends the link
//! to its site, so a link is only fetched when it looks like an ordinary
//! public web page: no credentials, no one-time or signed-URL markers, no
//! local or private host. When unsure, the answer is "do not fetch".
//!
//! The network itself lives in the app (`src-tauri/src/link_preview`); it
//! calls `fetch_target` for the copied link, `redirect_target` for every
//! redirect, `resource_url` for the preview image and icon, `is_public_ip`
//! for every resolved address, and `parse_html` for the page.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

pub use url::Url;
use url::Host;

/// Longest link (in bytes, as copied) Satchel considers for a preview.
pub const MAX_URL_LEN: usize = 2048;
/// Bytes of a page `parse_html` looks at. Metadata lives in `<head>`, which
/// is almost always well inside this.
pub const MAX_HTML_BYTES: usize = 512 * 1024;
/// Longest title kept, in characters.
pub const MAX_TITLE_CHARS: usize = 200;
/// Longest site name kept, in characters.
pub const MAX_SITE_CHARS: usize = 80;

/// Why a link is not fetched. Content-free, so it can be logged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Skip {
    /// Not an `http`/`https` URL (`file:`, `mailto:`, custom schemes...), or
    /// not a URL at all.
    NotWeb,
    /// Longer than `MAX_URL_LEN`.
    TooLong,
    /// Carries a user name or password (`https://user:pass@host`).
    Credentials,
    /// An IP address, a non-default port, or a local, private or reserved
    /// host name (`localhost`, `printer.local`, `intranet`...).
    PrivateHost,
    /// Looks like a one-time, signed or account link: tokens, keys,
    /// signatures, password resets, magic links, unsubscribe, OAuth...
    Sensitive,
    /// A redirect to plain `http`.
    Insecure,
}

// ------------------------------------------------------------ the link

/// The URL to fetch for a copied link, or why it is not fetched.
///
/// The result is always `https` (a plain `http` link is upgraded, never
/// fetched in clear text), has no fragment, and has well-known tracking
/// parameters (`utm_*`, `fbclid`, ...) removed, so they are not sent either.
pub fn fetch_target(text: &str) -> Result<Url, Skip> {
    let text = text.trim();
    if text.len() > MAX_URL_LEN {
        return Err(Skip::TooLong);
    }
    if text.is_empty() || text.chars().any(char::is_whitespace) {
        return Err(Skip::NotWeb);
    }
    let url = Url::parse(text).map_err(|_| Skip::NotWeb)?;
    check(url, true)
}

/// The next URL of a redirect from `current` to `location`. A redirect gets
/// every check the copied link got (a link shortener can lead to a magic
/// link), and may not leave `https`.
pub fn redirect_target(current: &Url, location: &str) -> Result<Url, Skip> {
    let location = location.trim();
    if location.len() > MAX_URL_LEN {
        return Err(Skip::TooLong);
    }
    let url = current.join(location).map_err(|_| Skip::NotWeb)?;
    match url.scheme() {
        "https" => check(url, false),
        "http" => Err(Skip::Insecure),
        _ => Err(Skip::NotWeb),
    }
}

/// The preview image or icon `href` of a page at `base`: an `https` URL on a
/// public host name with the default port, or `None`. Unlike page links,
/// image URLs are published by the site for exactly this use, so signed CDN
/// parameters are fine and are not stripped.
pub fn resource_url(base: &Url, href: &str) -> Option<Url> {
    let href = href.trim();
    if href.is_empty() || href.len() > MAX_URL_LEN {
        return None;
    }
    let mut url = base.join(href).ok()?;
    if url.scheme() == "http" {
        url.set_scheme("https").ok()?;
    }
    if url.scheme() != "https" || has_credentials(&url) || url.port().is_some() {
        return None;
    }
    if !public_domain(&url) {
        return None;
    }
    url.set_fragment(None);
    Some(url)
}

/// The site's `/favicon.ico`, used when a page names no image or icon.
pub fn favicon_url(page: &Url) -> Option<Url> {
    resource_url(page, "/favicon.ico")
}

/// The host as shown on a card: lowercase, without a leading `www.`.
pub fn display_host(url: &Url) -> String {
    let host = url.host_str().unwrap_or_default().trim_end_matches('.');
    host.strip_prefix("www.").unwrap_or(host).to_ascii_lowercase()
}

fn has_credentials(url: &Url) -> bool {
    !url.username().is_empty() || url.password().is_some()
}

fn check(mut url: Url, allow_upgrade: bool) -> Result<Url, Skip> {
    match url.scheme() {
        "https" => {}
        "http" if allow_upgrade => {
            // An explicit port on an `http` link (`:8080`) is kept by
            // `set_scheme` and refused below; `:80` was already dropped by
            // the parser as the default.
            url.set_scheme("https").map_err(|_| Skip::NotWeb)?;
        }
        "http" => return Err(Skip::Insecure),
        _ => return Err(Skip::NotWeb),
    }
    if has_credentials(&url) {
        return Err(Skip::Credentials);
    }
    if url.port().is_some() || !public_domain(&url) {
        return Err(Skip::PrivateHost);
    }
    if sensitive_host(&url) || sensitive_path(&url) || sensitive_fragment(&url) {
        return Err(Skip::Sensitive);
    }
    let kept = query_without_tracking(&url)?;
    url.set_fragment(None);
    if kept.is_empty() {
        url.set_query(None);
    } else {
        let mut q = url::form_urlencoded::Serializer::new(String::new());
        for (k, v) in &kept {
            q.append_pair(k, v);
        }
        url.set_query(Some(&q.finish()));
    }
    Ok(url)
}

// ------------------------------------------------------------- hosts

/// Top-level domains (and suffixes) that never name a public web site.
const LOCAL_SUFFIXES: &[&str] = &[
    "localhost",
    "local",
    "localdomain",
    "internal",
    "intranet",
    "lan",
    "home",
    "corp",
    "private",
    "test",
    "example",
    "invalid",
    "onion",
    "arpa",
    "home.arpa",
    "localtest.me",
];

/// First host labels of sign-in, account and mail hosts.
const SENSITIVE_HOST_LABELS: &[&str] = &[
    "login", "signin", "auth", "sso", "oauth", "accounts", "account", "id", "identity", "idp",
    "secure", "mail", "webmail", "admin", "vpn", "intranet", "pay", "payments", "checkout",
    "billing", "wallet",
];

/// A domain name (not an IP address) with at least two labels and a public
/// top-level domain.
fn public_domain(url: &Url) -> bool {
    let Some(Host::Domain(host)) = url.host() else {
        return false;
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() || !host.contains('.') || host.len() > 253 {
        return false;
    }
    // `url` turns dotted numbers into an IP host; a purely numeric last
    // label that slipped through (`1.2.3.4.`) is still not a name.
    let tld = host.rsplit('.').next().unwrap_or_default();
    if tld.is_empty() || tld.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    !LOCAL_SUFFIXES
        .iter()
        .any(|s| host == *s || host.ends_with(&format!(".{s}")))
}

fn sensitive_host(url: &Url) -> bool {
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    let first = host.split('.').next().unwrap_or_default();
    SENSITIVE_HOST_LABELS.contains(&first)
}

// ------------------------------------------------------- path / query

/// Whole path segments, or pieces of them split at `-`, `_` and `.`, that
/// mark account, one-time or payment pages.
const SENSITIVE_WORDS: &[&str] = &[
    "reset", "password", "passwords", "passwd", "login", "signin", "logout", "signout", "unsubscribe",
    "unsub", "optout", "verify", "verification", "confirm", "confirmation", "activate", "activation",
    "invite", "invites", "invitation", "oauth", "oauth2", "authorize", "authorization", "auth",
    "callback", "token", "tokens", "sso", "saml", "otp", "2fa", "mfa", "magic", "onetime", "secret",
    "secrets", "apikey", "credentials", "session", "redeem", "claim", "pay", "payment", "payments",
    "checkout", "invoice", "invoices", "billing", "receipt", "receipts", "account", "settings",
];

/// Joined forms checked against the whole segment.
const SENSITIVE_SEGMENTS: &[&str] = &["one-time", "sign-in", "log-in", "sign-out", "log-out", "opt-out"];

fn pieces(s: &str) -> impl Iterator<Item = &str> {
    s.split(['-', '_', '.', '~', '+', ' ']).filter(|p| !p.is_empty())
}

fn percent_decoded(s: &str) -> String {
    url::form_urlencoded::parse(format!("x={s}").as_bytes())
        .next()
        .map(|(_, v)| v.into_owned())
        .unwrap_or_default()
}

fn sensitive_path(url: &Url) -> bool {
    let Some(segments) = url.path_segments() else {
        // `cannot-be-a-base` URLs are not web pages.
        return true;
    };
    for raw in segments {
        let decoded = percent_decoded(raw);
        if token_like(&decoded) {
            return true;
        }
        let seg = decoded.to_ascii_lowercase();
        if SENSITIVE_SEGMENTS.contains(&seg.as_str()) {
            return true;
        }
        if pieces(&seg).any(|p| SENSITIVE_WORDS.contains(&p)) {
            return true;
        }
    }
    false
}

/// Tracking parameters removed before fetching (and never sent).
fn is_tracking_param(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.starts_with("utm_")
        || matches!(
            n.as_str(),
            "fbclid"
                | "gclid"
                | "dclid"
                | "gbraid"
                | "wbraid"
                | "msclkid"
                | "yclid"
                | "twclid"
                | "ttclid"
                | "igshid"
                | "mc_cid"
                | "mc_eid"
                | "_hsenc"
                | "_hsmi"
                | "si"
                | "ref_src"
                | "ref_url"
        )
}

/// Parameter names containing one of these (after removing `-`, `_` and
/// `.`, lowercased) mark a link as sensitive.
const SENSITIVE_PARAM_PARTS: &[&str] = &[
    "token", "secret", "passw", "pwd", "signature", "credential", "apikey", "accesskey", "privatekey",
    "auth", "session", "sessid", "otp", "nonce", "ticket", "invite", "magic", "reset", "verif",
    "confirm", "unsub", "jwt", "hmac", "redirect", "returnto", "returnurl", "callback", "continue",
    "email", "phone", "login", "code", "access", "secure", "expires",
];

/// Parameter names that are sensitive only as a whole.
const SENSITIVE_PARAMS: &[&str] = &[
    "key", "sig", "sid", "pass", "pw", "state", "exp", "next", "u", "url", "uri", "target", "dest",
    "destination", "return", "to", "id_hint", "user", "username", "uid",
];

fn sensitive_param(name: &str, value: &str) -> bool {
    let n: String = name
        .chars()
        .filter(|c| !matches!(c, '-' | '_' | '.'))
        .collect::<String>()
        .to_ascii_lowercase();
    if sensitive_name_part(&n) || SENSITIVE_PARAMS.contains(&n.as_str()) || n.ends_with("key") {
        return true;
    }
    let v = value.to_ascii_lowercase();
    // A link inside a link: redirects and sign-in flows.
    if v.contains("://") || v.starts_with("//") {
        return true;
    }
    // An address in a link identifies a person.
    if v.contains('@') {
        return true;
    }
    token_like(value)
}

fn sensitive_name_part(name: &str) -> bool {
    SENSITIVE_PARAM_PARTS.iter().any(|p| name.contains(p))
}

/// The query's pairs without tracking parameters, or `Sensitive`.
fn query_without_tracking(url: &Url) -> Result<Vec<(String, String)>, Skip> {
    let mut kept = Vec::new();
    for (k, v) in url.query_pairs() {
        if is_tracking_param(&k) {
            continue;
        }
        if sensitive_param(&k, &v) {
            return Err(Skip::Sensitive);
        }
        kept.push((k.into_owned(), v.into_owned()));
    }
    Ok(kept)
}

/// A fragment is never sent, but one that carries parameters or a token
/// (`#access_token=...`, `#/reset/abc123...`) marks a sign-in or one-time
/// link.
fn sensitive_fragment(url: &Url) -> bool {
    let Some(frag) = url.fragment() else {
        return false;
    };
    let frag = percent_decoded(frag);
    let lower = frag.to_ascii_lowercase();
    if frag.contains('=') {
        let pairs = url::form_urlencoded::parse(frag.trim_start_matches(['#', '!', '/', '?']).as_bytes());
        for (k, v) in pairs {
            if sensitive_param(&k, &v) {
                return true;
            }
        }
    }
    token_like(&frag)
        || lower
            .split(['/', '?', '&', '='])
            .any(|seg| SENSITIVE_SEGMENTS.contains(&seg) || pieces(seg).any(|p| SENSITIVE_WORDS.contains(&p)))
}

/// Looks like a random identifier rather than a word or a number: a UUID,
/// or a run of at least 16 letters and digits with both in it, or at least
/// 24 letters in mixed case. Article slugs ("how-to-bake-bread"), numeric
/// ids (tweets, issues) and short ids (YouTube's 11 characters) are not.
pub fn token_like(s: &str) -> bool {
    if contains_uuid(s) {
        return true;
    }
    s.split(|c: char| !c.is_ascii_alphanumeric()).any(|run| {
        let letters = run.bytes().filter(u8::is_ascii_alphabetic).count();
        let digits = run.bytes().filter(u8::is_ascii_digit).count();
        let upper = run.bytes().filter(u8::is_ascii_uppercase).count();
        (run.len() >= 16 && letters > 0 && digits > 0)
            || (run.len() >= 24 && upper > 0 && upper < letters)
    })
}

fn contains_uuid(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() < 36 {
        return false;
    }
    (0..=b.len() - 36).any(|i| {
        b[i..i + 36].iter().enumerate().all(|(j, c)| match j {
            8 | 13 | 18 | 23 => *c == b'-',
            _ => c.is_ascii_hexdigit(),
        })
    })
}

// ------------------------------------------------------- addresses

/// True only for globally routable unicast addresses. Loopback, private,
/// link-local, shared (CGNAT), multicast, documentation, benchmarking and
/// reserved ranges are refused, and IPv6 forms that embed an IPv4 address
/// (mapped, compatible, NAT64, 6to4) are judged by that address.
pub fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => public_v4(v4),
        IpAddr::V6(v6) => public_v6(v6),
    }
}

fn public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_multicast()
        || ip.is_broadcast()
        || ip.is_documentation()
        || a == 0
        || a >= 240
        || (a == 100 && (64..128).contains(&b)) // shared address space (CGNAT)
        || (a == 192 && b == 0 && c == 0) // IETF protocol assignments
        || (a == 192 && b == 88 && c == 99) // 6to4 relay anycast
        || (a == 198 && (b == 18 || b == 19))) // benchmarking
}

fn public_v6(ip: Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return public_v4(v4);
    }
    let s = ip.segments();
    // IPv4-compatible (::a.b.c.d, deprecated) and NAT64 (64:ff9b::/96).
    if s[..6] == [0, 0, 0, 0, 0, 0] || s[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
        if ip.is_unspecified() || ip.is_loopback() {
            return false;
        }
        let v4 = Ipv4Addr::new((s[6] >> 8) as u8, s[6] as u8, (s[7] >> 8) as u8, s[7] as u8);
        return public_v4(v4);
    }
    // 6to4 (2002::/16) embeds the IPv4 address in segments 1 and 2.
    if s[0] == 0x2002 {
        let v4 = Ipv4Addr::new((s[1] >> 8) as u8, s[1] as u8, (s[2] >> 8) as u8, s[2] as u8);
        return public_v4(v4);
    }
    // Only global unicast (2000::/3) is public, minus documentation
    // (2001:db8::/32), ORCHID and benchmarking (2001:2::/48, 2001:10::/28,
    // 2001:20::/28) and Teredo (2001::/32, which tunnels to anywhere).
    (s[0] & 0xe000) == 0x2000
        && !(s[0] == 0x2001 && (s[1] == 0x0db8 || s[1] == 0 || s[1] == 2 || (s[1] & 0xfff0) == 0x10 || (s[1] & 0xfff0) == 0x20))
        && !(s[0] == 0x3fff && s[1] < 0x1000) // 3fff::/20 documentation
}

// ------------------------------------------------------------ the page

/// What a page says about itself.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PageMeta {
    pub title: Option<String>,
    pub site_name: Option<String>,
    /// The preview image (`og:image`, `twitter:image`).
    pub image: Option<Url>,
    /// The best icon the page names (touch icon, then the largest icon).
    pub icon: Option<Url>,
}

#[derive(Default)]
struct Collected {
    title_tag: Option<Vec<u8>>,
    og_title: Option<Vec<u8>>,
    twitter_title: Option<Vec<u8>>,
    site_name: Option<Vec<u8>>,
    app_name: Option<Vec<u8>>,
    images: [Option<Vec<u8>>; 5],
    icon: Option<(u32, Vec<u8>)>,
    base: Option<Vec<u8>>,
    meta_charset: Option<String>,
}

/// Reads the title, site name, preview image and icon from the `<head>` of
/// an HTML document. Never runs or loads anything; looks at most at the
/// first `MAX_HTML_BYTES`; stops at `<body>` or `</head>`; ignores the
/// contents of `<script>`, `<style>`, `<noscript>`, `<template>` and
/// comments. Text is decoded from the header's charset, else the page's
/// `<meta charset>`, else UTF-8; UTF-8, ASCII, ISO-8859-1 and Windows-1252
/// are understood, and text in any other encoding is dropped rather than
/// shown garbled. URLs are resolved against `base` (or the page's
/// `<base href>`) and must pass `resource_url`.
pub fn parse_html(html: &[u8], header_charset: Option<&str>, base: &Url) -> PageMeta {
    let html = &html[..html.len().min(MAX_HTML_BYTES)];
    let html = html.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(html);
    if html.starts_with(b"\xFE\xFF") || html.starts_with(b"\xFF\xFE") {
        // UTF-16: tags are not ASCII bytes; nothing to read safely.
        return PageMeta::default();
    }
    let c = collect(html);
    let charset = header_charset
        .map(str::to_string)
        .or(c.meta_charset.clone())
        .unwrap_or_else(|| "utf-8".into());
    let text = |raw: &Option<Vec<u8>>, max: usize| raw.as_deref().and_then(|b| decode_text(b, &charset)).and_then(|s| clean_text(&s, max));
    let attr = |raw: &[u8]| decode_text(raw, &charset).map(|s| decode_entities(&s));

    let base = c
        .base
        .as_deref()
        .and_then(attr)
        .and_then(|href| resource_url(base, &href))
        .unwrap_or_else(|| base.clone());
    let title = text(&c.og_title, MAX_TITLE_CHARS)
        .or_else(|| text(&c.twitter_title, MAX_TITLE_CHARS))
        .or_else(|| text(&c.title_tag, MAX_TITLE_CHARS));
    let site_name = text(&c.site_name, MAX_SITE_CHARS).or_else(|| text(&c.app_name, MAX_SITE_CHARS));
    let image = c
        .images
        .iter()
        .flatten()
        .filter_map(|raw| attr(raw))
        .find_map(|href| resource_url(&base, &href));
    let icon = c
        .icon
        .as_ref()
        .and_then(|(_, raw)| attr(raw))
        .and_then(|href| resource_url(&base, &href));
    PageMeta { title, site_name, image, icon }
}

fn collect(html: &[u8]) -> Collected {
    let mut c = Collected::default();
    let mut i = 0;
    while let Some(off) = memchr(b'<', &html[i..]) {
        i += off;
        let rest = &html[i..];
        if rest.starts_with(b"<!--") {
            i = find(html, i + 4, b"-->").map_or(html.len(), |e| e + 3);
            continue;
        }
        if rest.starts_with(b"<!") || rest.starts_with(b"<?") {
            i = memchr(b'>', &html[i..]).map_or(html.len(), |e| i + e + 1);
            continue;
        }
        if rest.starts_with(b"</") {
            let (name, after) = tag_name(html, i + 2);
            if name == "head" {
                break;
            }
            i = memchr(b'>', &html[after..]).map_or(html.len(), |e| after + e + 1);
            continue;
        }
        let (name, after) = tag_name(html, i + 1);
        if name.is_empty() {
            i += 1;
            continue;
        }
        let (attrs, end) = attributes(html, after);
        i = end;
        match name.as_str() {
            "body" => break,
            "script" | "style" | "noscript" | "template" | "textarea" | "svg" | "iframe" | "object" => {
                i = close_tag(html, i, &name);
            }
            "title" => {
                let stop = find_ci(html, i, b"</title").unwrap_or(html.len());
                if c.title_tag.is_none() {
                    c.title_tag = Some(html[i..stop].to_vec());
                }
                i = stop;
            }
            "meta" => meta(&mut c, &attrs),
            "link" => link(&mut c, &attrs),
            "base" if c.base.is_none() => {
                c.base = get(&attrs, "href").map(<[u8]>::to_vec);
            }
            _ => {}
        }
    }
    c
}

type Attrs = Vec<(String, Vec<u8>)>;

fn get<'a>(attrs: &'a Attrs, name: &str) -> Option<&'a [u8]> {
    attrs.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_slice())
}

fn lower_ascii(raw: &[u8]) -> String {
    String::from_utf8_lossy(raw).trim().to_ascii_lowercase()
}

fn set_once(slot: &mut Option<Vec<u8>>, value: Option<&[u8]>) {
    if slot.is_none()
        && let Some(v) = value.filter(|v| !v.iter().all(u8::is_ascii_whitespace))
    {
        *slot = Some(v.to_vec());
    }
}

fn meta(c: &mut Collected, attrs: &Attrs) {
    if c.meta_charset.is_none() {
        if let Some(cs) = get(attrs, "charset") {
            c.meta_charset = Some(lower_ascii(cs));
        } else if get(attrs, "http-equiv").map(lower_ascii).as_deref() == Some("content-type")
            && let Some(content) = get(attrs, "content")
        {
            c.meta_charset = charset_param(&String::from_utf8_lossy(content));
        }
    }
    let key = get(attrs, "property").or_else(|| get(attrs, "name")).map(lower_ascii);
    let Some(key) = key else { return };
    let content = get(attrs, "content");
    match key.as_str() {
        "og:title" => set_once(&mut c.og_title, content),
        "twitter:title" => set_once(&mut c.twitter_title, content),
        "og:site_name" => set_once(&mut c.site_name, content),
        "application-name" => set_once(&mut c.app_name, content),
        "og:image:secure_url" => set_once(&mut c.images[0], content),
        "og:image" => set_once(&mut c.images[1], content),
        "og:image:url" => set_once(&mut c.images[2], content),
        "twitter:image" => set_once(&mut c.images[3], content),
        "twitter:image:src" => set_once(&mut c.images[4], content),
        _ => {}
    }
}

fn link(c: &mut Collected, attrs: &Attrs) {
    let Some(rel) = get(attrs, "rel").map(lower_ascii) else { return };
    let Some(href) = get(attrs, "href") else { return };
    let tokens: Vec<&str> = rel.split_ascii_whitespace().collect();
    let touch = tokens.iter().any(|t| t.starts_with("apple-touch-icon"));
    if !touch && !tokens.contains(&"icon") {
        return;
    }
    let ty = get(attrs, "type").map(lower_ascii).unwrap_or_default();
    let href_l = lower_ascii(href);
    if ty.contains("svg") || href_l.ends_with(".svg") || href_l.starts_with("data:") {
        return;
    }
    // Declared size; a touch icon without one is 180 px by convention, a
    // plain icon without one is assumed to be a 16 px favicon.
    let size = get(attrs, "sizes")
        .map(lower_ascii)
        .and_then(|s| s.split_ascii_whitespace().filter_map(|d| d.split('x').next()?.parse::<u32>().ok()).max())
        .unwrap_or(if touch { 180 } else { 16 })
        .min(1024);
    if c.icon.as_ref().is_none_or(|(best, _)| size > *best) {
        c.icon = Some((size, href.to_vec()));
    }
}

fn memchr(needle: u8, hay: &[u8]) -> Option<usize> {
    hay.iter().position(|&b| b == needle)
}

fn find(hay: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    if from >= hay.len() {
        return None;
    }
    hay[from..].windows(needle.len()).position(|w| w == needle).map(|p| from + p)
}

fn find_ci(hay: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    if from >= hay.len() {
        return None;
    }
    hay[from..]
        .windows(needle.len())
        .position(|w| w.eq_ignore_ascii_case(needle))
        .map(|p| from + p)
}

/// Skips to just after `</name ...>` (or to the end).
fn close_tag(html: &[u8], from: usize, name: &str) -> usize {
    let needle = format!("</{name}");
    match find_ci(html, from, needle.as_bytes()) {
        Some(p) => memchr(b'>', &html[p..]).map_or(html.len(), |e| p + e + 1),
        None => html.len(),
    }
}

fn tag_name(html: &[u8], from: usize) -> (String, usize) {
    let mut end = from;
    while end < html.len() && (html[end].is_ascii_alphanumeric() || html[end] == b'-' || html[end] == b':') {
        end += 1;
    }
    (String::from_utf8_lossy(&html[from..end]).to_ascii_lowercase(), end)
}

const MAX_ATTRS: usize = 64;
const MAX_ATTR_VALUE: usize = 8 * 1024;

/// Parses attributes up to the end of the tag. Returns them (names
/// lowercased, values raw, overlong values dropped) and the index after `>`.
fn attributes(html: &[u8], mut i: usize) -> (Attrs, usize) {
    let mut attrs = Vec::new();
    let n = html.len();
    loop {
        while i < n && (html[i].is_ascii_whitespace() || html[i] == b'/') {
            i += 1;
        }
        if i >= n {
            return (attrs, n);
        }
        if html[i] == b'>' {
            return (attrs, i + 1);
        }
        let start = i;
        while i < n && !html[i].is_ascii_whitespace() && !matches!(html[i], b'=' | b'>' | b'/') {
            i += 1;
        }
        let name = String::from_utf8_lossy(&html[start..i]).to_ascii_lowercase();
        while i < n && html[i].is_ascii_whitespace() {
            i += 1;
        }
        let mut value: &[u8] = b"";
        if i < n && html[i] == b'=' {
            i += 1;
            while i < n && html[i].is_ascii_whitespace() {
                i += 1;
            }
            if i < n && (html[i] == b'"' || html[i] == b'\'') {
                let q = html[i];
                let vstart = i + 1;
                let vend = memchr(q, &html[vstart..]).map_or(n, |e| vstart + e);
                value = &html[vstart..vend];
                i = (vend + 1).min(n);
            } else {
                let vstart = i;
                while i < n && !html[i].is_ascii_whitespace() && html[i] != b'>' {
                    i += 1;
                }
                value = &html[vstart..i];
            }
        }
        if !name.is_empty() && attrs.len() < MAX_ATTRS && value.len() <= MAX_ATTR_VALUE {
            attrs.push((name, value.to_vec()));
        }
    }
}

/// The `charset=` parameter of a `Content-Type` value, lowercased.
pub fn charset_param(content_type: &str) -> Option<String> {
    content_type.split(';').skip(1).find_map(|p| {
        let (k, v) = p.split_once('=')?;
        k.trim()
            .eq_ignore_ascii_case("charset")
            .then(|| v.trim().trim_matches(['"', '\'']).to_ascii_lowercase())
            .filter(|v| !v.is_empty())
    })
}

/// Windows-1252 code points for 0x80..=0x9F (the rest of the range is
/// ISO-8859-1, which maps bytes to the same code points).
const CP1252_HIGH: [char; 32] = [
    '\u{20AC}', '\u{81}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}', '\u{2021}', '\u{02C6}', '\u{2030}',
    '\u{0160}', '\u{2039}', '\u{0152}', '\u{8D}', '\u{017D}', '\u{8F}', '\u{90}', '\u{2018}', '\u{2019}', '\u{201C}',
    '\u{201D}', '\u{2022}', '\u{2013}', '\u{2014}', '\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}', '\u{0153}', '\u{9D}',
    '\u{017E}', '\u{0178}',
];

/// Decodes text in `charset`. `None` for an encoding Satchel does not read
/// (unless the bytes happen to be ASCII), so it is never shown garbled.
fn decode_text(bytes: &[u8], charset: &str) -> Option<String> {
    match charset.trim() {
        "utf-8" | "utf8" | "unicode-1-1-utf-8" => Some(String::from_utf8_lossy(bytes).into_owned()),
        "iso-8859-1" | "latin1" | "latin-1" | "iso8859-1" | "l1" | "windows-1252" | "cp1252" | "us-ascii" | "ascii" => {
            // The WHATWG encoding standard treats all of these as 1252.
            Some(
                bytes
                    .iter()
                    .map(|&b| if (0x80..0xA0).contains(&b) { CP1252_HIGH[(b - 0x80) as usize] } else { b as char })
                    .collect(),
            )
        }
        _ => bytes.is_ascii().then(|| String::from_utf8_lossy(bytes).into_owned()),
    }
}

/// Character references in text and attribute values: numeric ones and the
/// named ones titles actually use. Unknown names are left as written.
fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let end = rest[1..].find(|c: char| !(c.is_ascii_alphanumeric() || c == '#')).map(|e| e + 1);
        let (name, consumed) = match end {
            Some(e) if rest[e..].starts_with(';') => (&rest[1..e], e + 1),
            Some(e) => (&rest[1..e], e),
            None => (&rest[1..], rest.len()),
        };
        match entity(name) {
            Some(ch) if !name.is_empty() => {
                out.push(ch);
                rest = &rest[consumed..];
            }
            _ => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn entity(name: &str) -> Option<char> {
    if let Some(num) = name.strip_prefix('#') {
        let code = match num.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok(),
            None => num.parse().ok(),
        };
        // An invalid or NUL reference becomes U+FFFD, which `clean_text`
        // drops, as browsers would show it.
        return Some(code.and_then(char::from_u32).filter(|c| *c != '\0').unwrap_or('\u{FFFD}'));
    }
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => ' ',
        "ndash" => '\u{2013}',
        "mdash" => '\u{2014}',
        "hellip" => '\u{2026}',
        "lsquo" => '\u{2018}',
        "rsquo" => '\u{2019}',
        "ldquo" => '\u{201C}',
        "rdquo" => '\u{201D}',
        "laquo" => '\u{AB}',
        "raquo" => '\u{BB}',
        "middot" => '\u{B7}',
        "bull" => '\u{2022}',
        "copy" => '\u{A9}',
        "reg" => '\u{AE}',
        "trade" => '\u{2122}',
        "eacute" => '\u{E9}',
        "egrave" => '\u{E8}',
        "aacute" => '\u{E1}',
        "agrave" => '\u{E0}',
        "uuml" => '\u{FC}',
        "ouml" => '\u{F6}',
        "auml" => '\u{E4}',
        "szlig" => '\u{DF}',
        "ccedil" => '\u{E7}',
        "ntilde" => '\u{F1}',
        "pipe" | "vert" => '|',
        _ => return None,
    })
}

/// Decodes entities, drops control and bidirectional-override characters
/// (which could make a title read as something else), collapses
/// whitespace and caps the length. `None` when nothing is left.
fn clean_text(raw: &str, max_chars: usize) -> Option<String> {
    let decoded = decode_entities(raw);
    let mut out = String::with_capacity(decoded.len().min(max_chars * 4));
    let mut count = 0;
    let mut space = false;
    for ch in decoded.chars() {
        let bidi = matches!(ch, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}' | '\u{061C}');
        if bidi || ch == '\u{FEFF}' || ch == '\u{FFFD}' {
            continue;
        }
        if ch.is_whitespace() || ch.is_control() {
            space = !out.is_empty();
            continue;
        }
        if count + usize::from(space) >= max_chars {
            out.push('\u{2026}');
            break;
        }
        if space {
            out.push(' ');
            count += 1;
            space = false;
        }
        out.push(ch);
        count += 1;
    }
    (!out.is_empty()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(s: &str) -> String {
        match fetch_target(s) {
            Ok(u) => u.to_string(),
            Err(e) => panic!("{s} was skipped: {e:?}"),
        }
    }

    fn skip(s: &str) -> Skip {
        match fetch_target(s) {
            Ok(u) => panic!("{s} would be fetched as {u}"),
            Err(e) => e,
        }
    }

    #[test]
    fn ordinary_pages_are_fetched() {
        for url in [
            "https://www.nytimes.com/2024/05/01/world/europe/some-long-article-slug.html",
            "https://en.wikipedia.org/wiki/Clipboard_(computing)",
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            "https://youtu.be/dQw4w9WgXcQ",
            "https://vimeo.com/76979871",
            "https://x.com/someone/status/1790000000000000000",
            "https://github.com/tauri-apps/tauri/issues/1234",
            "https://www.amazon.com/Some-Product-Name/dp/B0ABCDEFGH",
            "https://www.apple.com/macbook-pro/",
            "https://medium.com/@writer/how-to-bake-bread-3f2a1b4c5d6e",
            "https://news.ycombinator.com/item?id=40000000",
            "https://example.co.uk/search?q=rust+clipboard&page=2",
            "https://xn--bcher-kva.example.com/",
            "  https://www.bbc.co.uk/news  ",
        ] {
            ok(url);
        }
    }

    #[test]
    fn http_is_upgraded_and_never_sent_in_clear() {
        assert_eq!(ok("http://www.bbc.co.uk/news"), "https://www.bbc.co.uk/news");
        assert_eq!(ok("http://bbc.co.uk:80/"), "https://bbc.co.uk/");
        assert_eq!(skip("http://bbc.co.uk:8080/"), Skip::PrivateHost);
    }

    #[test]
    fn tracking_parameters_and_fragments_are_not_sent() {
        assert_eq!(
            ok("https://shop.example.com/item/42?utm_source=x&utm_medium=y&color=red&fbclid=abc#reviews"),
            "https://shop.example.com/item/42?color=red"
        );
        assert_eq!(ok("https://youtu.be/dQw4w9WgXcQ?si=Ab3dEf9hIjKlMn0p"), "https://youtu.be/dQw4w9WgXcQ");
        assert_eq!(ok("https://a.example.com/?gclid=1&msclkid=2"), "https://a.example.com/");
    }

    #[test]
    fn non_web_and_malformed_links_are_skipped() {
        for url in [
            "file:///etc/passwd",
            "ftp://example.com/file",
            "mailto:someone@example.com",
            "javascript:alert(1)",
            "data:text/html,hi",
            "slack://open",
            "about:blank",
            "not a url",
            "https://exa mple.com/",
            "",
            "https://",
        ] {
            assert_eq!(skip(url), Skip::NotWeb, "{url}");
        }
    }

    #[test]
    fn long_links_are_skipped() {
        let long = format!("https://example.com/{}", "a".repeat(MAX_URL_LEN));
        assert_eq!(skip(&long), Skip::TooLong);
    }

    #[test]
    fn credentials_in_the_link_are_skipped() {
        assert_eq!(skip("https://user:pass@example.com/"), Skip::Credentials);
        assert_eq!(skip("https://user@example.com/"), Skip::Credentials);
        assert_eq!(skip("http://admin:admin@example.com/"), Skip::Credentials);
    }

    #[test]
    fn local_private_and_numeric_hosts_are_skipped() {
        for url in [
            "https://localhost/",
            "https://localhost:3000/",
            "http://127.0.0.1/",
            "https://127.1/",
            "https://0x7f000001/",
            "https://2130706433/",
            "https://[::1]/",
            "https://[fe80::1]/",
            "https://10.0.0.1/admin",
            "https://192.168.1.1/",
            "https://169.254.169.254/latest/meta-data/",
            "https://8.8.8.8/",
            "https://[2606:4700::1111]/",
            "https://printer.local/",
            "https://nas.lan/",
            "https://intranet/",
            "https://wiki.corp/page",
            "https://service.internal/",
            "https://app.test/",
            "https://router.home.arpa/",
            "https://something.onion/",
            "https://localtest.me/",
            "https://sub.localtest.me/",
            "https://example.com:8443/",
            "https://example.com:444/",
        ] {
            assert_eq!(skip(url), Skip::PrivateHost, "{url}");
        }
    }

    #[test]
    fn one_time_signed_and_account_links_are_skipped() {
        for url in [
            // Tokens, keys and signatures in parameters.
            "https://example.com/welcome?token=abc",
            "https://example.com/page?access_token=abc",
            "https://example.com/page?id_token=abc",
            "https://example.com/page?api_key=abc",
            "https://example.com/page?apiKey=abc",
            "https://example.com/page?key=abc",
            "https://example.com/page?sig=abc",
            "https://bucket.s3.amazonaws.com/file.pdf?X-Amz-Algorithm=AWS4&X-Amz-Credential=a&X-Amz-Signature=b",
            "https://storage.googleapis.com/b/o?X-Goog-Signature=abc",
            "https://acct.blob.core.windows.net/c/f?sv=2020&se=2024&sp=r&sig=abc",
            "https://cdn.example.com/f?Expires=1700000000&Signature=abc&Key-Pair-Id=K",
            "https://example.com/page?session=abc",
            "https://example.com/page?sid=abc",
            "https://example.com/page?password=hunter2",
            "https://us02web.zoom.us/j/123456789?pwd=abcdef",
            "https://example.com/callback?code=abc&state=xyz",
            "https://example.com/page?otp=123456",
            "https://example.com/page?nonce=abc",
            "https://example.com/page?ticket=ST-1",
            "https://example.com/page?jwt=eyJ",
            "https://example.com/page?hmac=abc",
            // Sign-in flows and links inside links.
            "https://example.com/page?redirect_uri=https%3A%2F%2Fapp.example.com",
            "https://example.com/page?next=/dashboard",
            "https://example.com/page?continue=https://x.example.com",
            "https://example.com/go?url=https://other.example.com",
            "https://example.com/go?to=//other.example.com",
            // Personal data in parameters.
            "https://example.com/page?email=me%40example.com",
            "https://example.com/page?e=me@example.com",
            // Paths.
            "https://example.com/reset-password/abc",
            "https://example.com/password/reset",
            "https://example.com/auth/callback",
            "https://example.com/oauth/authorize",
            "https://example.com/login",
            "https://example.com/sign-in",
            "https://example.com/magic-link/abc",
            "https://example.com/users/confirmation",
            "https://example.com/verify/email",
            "https://example.com/unsubscribe",
            "https://example.com/u/unsubscribe/abc",
            "https://example.com/invite/abc",
            "https://example.com/one-time/abc",
            "https://example.com/activate",
            "https://example.com/gift/redeem",
            "https://example.com/checkout",
            "https://example.com/invoices/123",
            "https://example.com/account",
            // Random identifiers (capability links).
            "https://docs.google.com/document/d/1aBcDeFgHiJkLmNoPqRsTuVwXyZ0123456789abcdEf/edit",
            "https://example.com/share/550e8400-e29b-41d4-a716-446655440000",
            "https://www.notion.so/Page-Title-0123456789abcdef0123456789abcdef",
            "https://example.com/s/a1b2c3d4e5f6g7h8i9",
            "https://example.com/view?doc=a1b2c3d4e5f6g7h8i9",
            "https://example.com/d/AbCdEfGhIjKlMnOpQrStUvWxYz",
            "https://github.com/x/y/commit/0123456789abcdef0123456789abcdef01234567",
            // Fragments.
            "https://example.com/#access_token=abc&token_type=bearer",
            "https://example.com/app#/reset/abc",
            "https://example.com/app#a1b2c3d4e5f6g7h8i9j0",
            // Sign-in, account and mail hosts.
            "https://accounts.google.com/o/whatever",
            "https://login.microsoftonline.com/common",
            "https://auth.example.com/",
            "https://sso.example.com/",
            "https://mail.google.com/mail/u/0/",
            "https://id.example.com/",
            "https://checkout.stripe.com/c/pay",
        ] {
            assert_eq!(skip(url), Skip::Sensitive, "{url}");
        }
    }

    #[test]
    fn token_heuristic_spares_words_numbers_and_short_ids() {
        for s in ["how-to-bake-bread", "1790000000000000000", "dQw4w9WgXcQ", "B0ABCDEFGH", "2024", "Clipboard_(computing)", "issues"] {
            assert!(!token_like(s), "{s}");
        }
        for s in ["a1b2c3d4e5f6g7h8", "550e8400-e29b-41d4-a716-446655440000", "AbCdEfGhIjKlMnOpQrStUvWx"] {
            assert!(token_like(s), "{s}");
        }
    }

    #[test]
    fn redirects_are_checked_like_links_and_stay_on_https() {
        let here = Url::parse("https://bit.ly/abc").unwrap();
        assert_eq!(redirect_target(&here, "https://www.example.com/article").unwrap().as_str(), "https://www.example.com/article");
        assert_eq!(redirect_target(&here, "/other").unwrap().as_str(), "https://bit.ly/other");
        assert_eq!(redirect_target(&here, "http://www.example.com/"), Err(Skip::Insecure));
        assert_eq!(redirect_target(&here, "https://127.0.0.1/"), Err(Skip::PrivateHost));
        assert_eq!(redirect_target(&here, "https://localhost/"), Err(Skip::PrivateHost));
        assert_eq!(redirect_target(&here, "https://example.com/login?token=abc"), Err(Skip::Sensitive));
        assert_eq!(redirect_target(&here, "file:///etc/passwd"), Err(Skip::NotWeb));
        assert_eq!(redirect_target(&here, "https://u:p@example.com/"), Err(Skip::Credentials));
    }

    #[test]
    fn resource_urls_are_https_public_and_resolved() {
        let base = Url::parse("https://www.example.com/a/b.html").unwrap();
        let r = |h: &str| resource_url(&base, h).map(|u| u.to_string());
        assert_eq!(r("/img/og.png").as_deref(), Some("https://www.example.com/img/og.png"));
        assert_eq!(r("og.png").as_deref(), Some("https://www.example.com/a/og.png"));
        assert_eq!(r("//cdn.example.net/x.jpg?sig=1").as_deref(), Some("https://cdn.example.net/x.jpg?sig=1"));
        assert_eq!(r("http://cdn.example.net/x.jpg").as_deref(), Some("https://cdn.example.net/x.jpg"));
        assert_eq!(r("https://10.0.0.1/x.png"), None);
        assert_eq!(r("https://localhost/x.png"), None);
        assert_eq!(r("https://cdn.example.net:8443/x.png"), None);
        assert_eq!(r("file:///x.png"), None);
        assert_eq!(r("data:image/png;base64,AAAA"), None);
        assert_eq!(r("https://u:p@cdn.example.net/x.png"), None);
        assert_eq!(r(""), None);
        assert_eq!(favicon_url(&base).unwrap().as_str(), "https://www.example.com/favicon.ico");
        assert_eq!(display_host(&base), "example.com");
    }

    #[test]
    fn only_public_addresses_are_allowed() {
        let public = ["93.184.216.34", "8.8.8.8", "1.1.1.1", "2606:4700:4700::1111", "2a00:1450:4001::200e", "::ffff:8.8.8.8", "64:ff9b::808:808"];
        for ip in public {
            assert!(is_public_ip(ip.parse().unwrap()), "{ip}");
        }
        let private = [
            "127.0.0.1", "127.255.255.254", "0.0.0.0", "0.1.2.3", "10.1.2.3", "172.16.0.1", "172.31.255.255", "192.168.0.1",
            "169.254.169.254", "100.64.0.1", "100.127.255.255", "192.0.0.1", "192.0.2.1", "198.18.0.1", "198.51.100.1",
            "203.0.113.1", "224.0.0.1", "239.255.255.250", "240.0.0.1", "255.255.255.255", "::", "::1", "fe80::1",
            "fc00::1", "fd12:3456::1", "ff02::1", "::ffff:127.0.0.1", "::ffff:10.0.0.1", "::127.0.0.1", "64:ff9b::7f00:1",
            "64:ff9b::a00:1", "2002:7f00:1::1", "2002:c0a8:101::1", "2001:db8::1", "2001::1", "3fff::1", "100::1",
        ];
        for ip in private {
            assert!(!is_public_ip(ip.parse().unwrap()), "{ip}");
        }
    }

    // ------------------------------------------------------------ html

    fn base() -> Url {
        Url::parse("https://www.example.com/articles/one").unwrap()
    }

    fn parse(html: &str) -> PageMeta {
        parse_html(html.as_bytes(), None, &base())
    }

    #[test]
    fn reads_open_graph_twitter_and_title() {
        let m = parse(
            r#"<!doctype html><html><head>
            <meta charset="utf-8">
            <title>Fallback &amp; Title</title>
            <meta property="og:title" content="The &quot;Real&quot; Title">
            <meta property="og:site_name" content="Example News">
            <meta property="og:image" content="/img/cover.jpg">
            <link rel="icon" href="/favicon-32.png" sizes="32x32">
            <link rel="apple-touch-icon" href="/touch.png">
            </head><body><meta property="og:title" content="ignored"></body>"#,
        );
        assert_eq!(m.title.as_deref(), Some("The \"Real\" Title"));
        assert_eq!(m.site_name.as_deref(), Some("Example News"));
        assert_eq!(m.image.unwrap().as_str(), "https://www.example.com/img/cover.jpg");
        assert_eq!(m.icon.unwrap().as_str(), "https://www.example.com/touch.png");
    }

    #[test]
    fn falls_back_to_twitter_then_title_tag() {
        let m = parse("<head><meta name=twitter:title content='Tweet title'><title>T</title><meta name='twitter:image' content='https://cdn.example.net/t.png'>");
        assert_eq!(m.title.as_deref(), Some("Tweet title"));
        assert_eq!(m.image.unwrap().as_str(), "https://cdn.example.net/t.png");
        let m = parse("<HTML><HEAD><TITLE>\n  Shouting   Title \n</TITLE></HEAD>");
        assert_eq!(m.title.as_deref(), Some("Shouting Title"));
        assert_eq!(m.site_name, None);
        assert_eq!(m.image, None);
        assert_eq!(m.icon, None);
    }

    #[test]
    fn secure_image_url_wins_and_unsafe_images_are_dropped() {
        let m = parse(
            r#"<meta property="og:image" content="http://cdn.example.net/a.jpg">
               <meta property="og:image:secure_url" content="https://cdn.example.net/secure.jpg">"#,
        );
        assert_eq!(m.image.unwrap().as_str(), "https://cdn.example.net/secure.jpg");
        let m = parse(r#"<meta property="og:image" content="https://192.168.0.10/a.jpg"><meta name="twitter:image" content="https://cdn.example.net/b.jpg">"#);
        assert_eq!(m.image.unwrap().as_str(), "https://cdn.example.net/b.jpg");
        let m = parse(r#"<meta property="og:image" content="file:///etc/passwd">"#);
        assert_eq!(m.image, None);
    }

    #[test]
    fn largest_icon_wins_and_svg_icons_are_skipped() {
        let m = parse(
            r#"<link rel="icon" href="/16.png"><link rel="icon" sizes="192x192" href="/192.png">
               <link rel="icon" type="image/svg+xml" href="/icon.svg"><link rel="mask-icon" href="/mask.png">
               <link rel="shortcut icon" sizes="64x64" href="/64.ico">"#,
        );
        assert_eq!(m.icon.unwrap().as_str(), "https://www.example.com/192.png");
    }

    #[test]
    fn scripts_comments_and_body_are_ignored() {
        let m = parse(
            r#"<head><!-- <title>Commented</title> -->
            <script>var s = "<title>Scripted</title><meta property='og:title' content='x'>";</script>
            <style>/* <title>Styled</title> */</style>
            <noscript><meta property="og:image" content="/noscript.png"></noscript>
            <title>Real</title></head><body><title>Body</title>"#,
        );
        assert_eq!(m.title.as_deref(), Some("Real"));
        assert_eq!(m.image, None);
    }

    #[test]
    fn base_href_changes_resolution_only_when_safe() {
        let m = parse(r#"<base href="https://static.example.org/root/"><meta property="og:image" content="img.png">"#);
        assert_eq!(m.image.unwrap().as_str(), "https://static.example.org/root/img.png");
        let m = parse(r#"<base href="https://10.0.0.1/"><meta property="og:image" content="img.png">"#);
        assert_eq!(m.image.unwrap().as_str(), "https://www.example.com/articles/img.png");
    }

    #[test]
    fn malformed_html_does_not_panic_or_mislead() {
        for html in [
            "",
            "<",
            "<<<<>>>>",
            "<title>",
            "<title>Unclosed",
            "<meta property=\"og:title\" content=\"unterminated",
            "<meta property='og:title' content=>",
            "<!-- never closed",
            "<script>never closed",
            "<meta =x =y>",
            "<\u{0}\u{1}>",
            "</>",
            "<!doctype",
            "<title></title>",
            "<title>   </title>",
        ] {
            let m = parse(html);
            if let Some(t) = &m.title {
                assert!(!t.trim().is_empty(), "{html:?}");
            }
        }
        assert_eq!(parse("<title>Unclosed").title.as_deref(), Some("Unclosed"));
        assert_eq!(parse("<title>   </title>").title, None);
    }

    #[test]
    fn huge_input_is_bounded() {
        let mut html = String::from("<head>");
        html.push_str(&"<meta name=x content=y>".repeat(100_000));
        html.push_str("<title>Too far</title>");
        let started = std::time::Instant::now();
        let m = parse(&html);
        assert_eq!(m.title, None, "beyond MAX_HTML_BYTES");
        assert!(started.elapsed() < std::time::Duration::from_secs(2));

        let long = format!("<title>{}</title>", "word ".repeat(10_000));
        let t = parse(&long).title.unwrap();
        assert!(t.chars().count() <= MAX_TITLE_CHARS + 1);
        assert!(t.ends_with('\u{2026}'));

        let huge_attr = format!("<meta property=og:title content=\"{}\"><title>Short</title>", "a".repeat(100_000));
        assert_eq!(parse(&huge_attr).title.as_deref(), Some("Short"));
    }

    #[test]
    fn charsets_from_header_meta_or_utf8() {
        // "Café" in Latin-1 / Windows-1252.
        let latin1: &[u8] = b"<title>Caf\xE9 \x93quoted\x94</title>";
        assert_eq!(parse_html(latin1, Some("iso-8859-1"), &base()).title.as_deref(), Some("Café \u{201C}quoted\u{201D}"));
        assert_eq!(parse_html(latin1, Some("windows-1252"), &base()).title.as_deref(), Some("Café \u{201C}quoted\u{201D}"));
        let meta: &[u8] = b"<meta http-equiv=\"Content-Type\" content=\"text/html; charset=ISO-8859-1\"><title>Caf\xE9</title>";
        assert_eq!(parse_html(meta, None, &base()).title.as_deref(), Some("Café"));
        let meta2: &[u8] = b"<meta charset=windows-1252><title>Caf\xE9</title>";
        assert_eq!(parse_html(meta2, None, &base()).title.as_deref(), Some("Café"));
        // UTF-8, with and without a BOM; invalid bytes are dropped, not shown.
        assert_eq!(parse("<title>日本語のタイトル</title>").title.as_deref(), Some("日本語のタイトル"));
        let bom: &[u8] = b"\xEF\xBB\xBF<title>Caf\xC3\xA9</title>";
        assert_eq!(parse_html(bom, None, &base()).title.as_deref(), Some("Café"));
        let bad: &[u8] = b"<title>A\xFF\xFEB</title>";
        assert_eq!(parse_html(bad, Some("utf-8"), &base()).title.as_deref(), Some("AB"));
        // An encoding Satchel does not read: non-ASCII text is dropped.
        let sjis: &[u8] = b"<title>\x93\xfa\x96\x7b</title>";
        assert_eq!(parse_html(sjis, Some("shift_jis"), &base()).title, None);
        assert_eq!(parse_html(b"<title>Plain</title>", Some("shift_jis"), &base()).title.as_deref(), Some("Plain"));
        // UTF-16 is not parsed at all.
        assert_eq!(parse_html(b"\xFF\xFE<\x00t\x00", None, &base()), PageMeta::default());
        assert_eq!(charset_param("text/html; charset=\"UTF-8\""), Some("utf-8".into()));
        assert_eq!(charset_param("text/html"), None);
    }

    #[test]
    fn text_is_cleaned_of_controls_and_bidi_overrides() {
        let m = parse("<title>Safe\u{202E}txt.exe\u{2066}x\u{0007}y&#0;&#x202E;z</title>");
        assert_eq!(m.title.as_deref(), Some("Safetxt.exex yz"));
        let m = parse("<title>A &lt;b&gt; &#169; &#xA9; &unknown; &amp</title>");
        assert_eq!(m.title.as_deref(), Some("A <b> © © &unknown; &"));
    }
}
