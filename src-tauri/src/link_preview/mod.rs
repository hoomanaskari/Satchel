//! Link previews: the background workers that fetch a copied link's title
//! and image, when (and only when) "Show link previews" is on.
//!
//! Flow: capture queues a new or re-copied link clip in the store
//! (`on_copied`) and wakes the workers; links already in the history when
//! previews are on (an upgrade, or the setting turned back on) are queued by
//! a paced backfill, one every `BACKFILL_EVERY_MS`, newest first, starting
//! `BACKFILL_EVERY_MS` after launch, so there is never a burst of requests; a worker claims a due job, re-checks
//! the setting, the excluded apps and `satchel_core::link::fetch_target`,
//! fetches the page and one image through a `Fetcher`, and records the
//! outcome. Nothing here runs on the capture or shelf-show paths, and with
//! the setting off no job is queued or claimed and no request is made.
//!
//! The network sits behind `Fetcher` (`http::HttpFetcher` in the app) and
//! image decoding behind `ImageDecoder`, so the logic is tested with fakes.

pub mod http;

use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use satchel_core::link::{self, PageMeta};
use satchel_core::{
    ClipId, ClipKind, FetchedPreview, LinkImageKind, LinkPreviewJob, LinkPreviewOutcome, Millis, privacy,
};
use satchel_core::link::Url;

use crate::ctx::Ctx;
use crate::events::ChangeReason;

/// Previews fetched at the same time.
pub const WORKERS: usize = 2;
/// How long a claimed job is reserved for its worker (longer than a whole
/// page + image fetch can take).
pub const LEASE_MS: Millis = 2 * 60 * 1000;
/// First retry after a transient failure; each later one waits 4x longer.
pub const RETRY_BASE_MS: Millis = 60 * 1000;
/// Pace of the backfill: one older link queued at most this often (and the
/// first one this long after launch or after the setting is turned on).
pub const BACKFILL_EVERY_MS: Millis = 15 * 1000;

/// Longest side of a stored preview image: the card shows it 202 pt wide
/// (404 px on Retina), cropped to 92 pt tall.
pub const PHOTO_MAX_SIDE: u32 = 480;
/// Longest side of a stored site icon (shown at up to 40 pt).
pub const ICON_MAX_SIDE: u32 = 128;

/// A fetched HTML page (the start of its body).
#[derive(Clone, Debug)]
pub struct Page {
    /// Where the redirects ended.
    pub url: Url,
    /// At most `link::MAX_HTML_BYTES`.
    pub body: Vec<u8>,
    /// From the `Content-Type` header.
    pub charset: Option<String>,
}

/// Why a fetch failed. Content-free.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FetchError {
    /// Offline, DNS failure, timeout, 408/429/5xx: worth another try later.
    Transient,
    /// Refused (4xx), not HTML / not an image, too large, too many
    /// redirects, TLS failure: final.
    Permanent,
    /// A redirect or resolved address that Satchel may not follow: final.
    Blocked,
}

/// The network. Implementations must not send cookies or credentials, must
/// check every redirect with `link::redirect_target` and every resolved
/// address with `link::is_public_ip`, and must cap time and size.
pub trait Fetcher: Send + Sync {
    fn page(&self, url: &Url) -> Result<Page, FetchError>;
    /// Raw bytes of an image (already checked to be an allowed format).
    fn image(&self, url: &Url) -> Result<Vec<u8>, FetchError>;
}

/// Turns downloaded image bytes into a PNG no larger than `max_side`.
pub trait ImageDecoder: Send + Sync {
    fn thumbnail(&self, bytes: &[u8], max_side: u32) -> Option<Vec<u8>>;
}

/// ImageIO (macOS) plus Satchel's PNG thumbnailer.
pub struct SystemImages;

impl ImageDecoder for SystemImages {
    fn thumbnail(&self, bytes: &[u8], max_side: u32) -> Option<Vec<u8>> {
        let png = crate::platform::web_image_to_png(bytes)?;
        crate::thumbnail::png_thumbnail(&png, max_side).ok()
    }
}

/// Wakes the workers (a new job was queued or the setting was turned on)
/// and paces the backfill.
#[derive(Default)]
pub struct Wake {
    generation: Mutex<u64>,
    cv: Condvar,
    /// When the backfill may queue its next link; `None` when it has run
    /// out of links (or has not been started).
    backfill_at: Mutex<Option<Millis>>,
}

impl Wake {
    /// (Re)starts the backfill: its first link is queued at `at`.
    pub fn start_backfill(&self, at: Millis) {
        *self.backfill_at.lock().unwrap_or_else(|p| p.into_inner()) = Some(at);
        self.notify();
    }

    fn backfill_at(&self) -> Option<Millis> {
        *self.backfill_at.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn set_backfill_at(&self, at: Option<Millis>) {
        *self.backfill_at.lock().unwrap_or_else(|p| p.into_inner()) = at;
    }

    pub fn notify(&self) {
        *self.generation.lock().unwrap_or_else(|p| p.into_inner()) += 1;
        self.cv.notify_all();
    }

    fn generation(&self) -> u64 {
        *self.generation.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Blocks until `notify` is called after `seen` was read, or `timeout`
    /// passes (`None`: no timeout).
    fn wait(&self, seen: u64, timeout: Option<Duration>) {
        let guard = self.generation.lock().unwrap_or_else(|p| p.into_inner());
        match timeout {
            Some(t) => drop(self.cv.wait_timeout_while(guard, t, |g| *g == seen)),
            None => drop(self.cv.wait_while(guard, |g| *g == seen)),
        }
    }
}

fn trace(what: &str) {
    if crate::shelf::tracing() {
        // Content-free: never the link, the title or the host.
        eprintln!("[satchel] link-preview {what}");
    }
}

/// Called after a clip is stored or bumped by a copy: queues a link clip
/// for a preview when the setting is on. Cheap (one indexed insert); the
/// fetch happens on a worker.
pub fn on_copied(ctx: &Ctx, id: ClipId, kind: ClipKind) {
    if kind != ClipKind::Link || !ctx.settings().link_previews {
        return;
    }
    let queued = ctx.store.lock().queue_link_preview(id, ctx.clock.now()).unwrap_or(false);
    if queued {
        ctx.link_wake.notify();
    }
}

/// What one worker step did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// The setting is off: nothing was claimed.
    Off,
    /// No job is due; the next one (or the next backfill) is due at this
    /// time, if any.
    Idle(Option<Millis>),
    /// The backfill queued this older link clip.
    Backfilled(ClipId),
    /// A job ran. `ready` is true when it produced a preview.
    Ran { id: ClipId, ready: bool },
    /// The store failed.
    Failed,
}

/// Claims and runs one due job.
pub fn step(ctx: &Ctx, fetcher: &dyn Fetcher, images: &dyn ImageDecoder) -> Step {
    if !ctx.settings().link_previews {
        return Step::Off;
    }
    let now = ctx.clock.now();
    let claimed = ctx.store.lock().claim_link_preview(now, LEASE_MS);
    let job = match claimed {
        Ok(Some(job)) => job,
        Ok(None) => return backfill_or_idle(ctx, now),
        Err(_) => return Step::Failed,
    };
    let id = job.clip_id;
    let outcome = run(ctx, fetcher, images, &job);
    if !ctx.settings().link_previews {
        // Turned off meanwhile: the previews were cleared; drop the result.
        return Step::Ran { id, ready: false };
    }
    trace(match &outcome {
        LinkPreviewOutcome::Ready(_) => "ready",
        LinkPreviewOutcome::Retry { .. } => "retry later",
        LinkPreviewOutcome::Failed => "failed",
        LinkPreviewOutcome::Skipped => "skipped",
    });
    let ready = ctx.store.lock().finish_link_preview(id, outcome, ctx.clock.now()).unwrap_or(false);
    if ready {
        ctx.events.clips_changed(ChangeReason::Updated, Some(id));
    }
    Step::Ran { id, ready }
}

/// With nothing due: queues one older link if the backfill's turn has
/// come, else reports when the next job or backfill is due.
fn backfill_or_idle(ctx: &Ctx, now: Millis) -> Step {
    let wake = &ctx.link_wake;
    if let Some(at) = wake.backfill_at()
        && at <= now
    {
        // Claim the turn first, so two idle workers do not both backfill.
        wake.set_backfill_at(Some(now.saturating_add(BACKFILL_EVERY_MS)));
        match ctx.store.lock().queue_link_backfill(now) {
            Ok(Some(id)) => {
                trace("backfill queued");
                return Step::Backfilled(id);
            }
            Ok(None) => wake.set_backfill_at(None),
            Err(_) => return Step::Failed,
        }
    }
    let next_job = ctx.store.lock().next_link_preview_at().unwrap_or(None);
    let next = match (next_job, wake.backfill_at()) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    Step::Idle(next)
}

fn still_on(ctx: &Ctx) -> bool {
    ctx.settings().link_previews
}

/// Decides whether the job may be fetched, then fetches it.
fn run(ctx: &Ctx, fetcher: &dyn Fetcher, images: &dyn ImageDecoder, job: &LinkPreviewJob) -> LinkPreviewOutcome {
    let settings = ctx.settings();
    if job.source_app_id.as_deref().is_some_and(|app| privacy::is_excluded_app(app, &settings)) {
        trace("skip: excluded app");
        return LinkPreviewOutcome::Skipped;
    }
    let target = match link::fetch_target(&job.url) {
        Ok(url) => url,
        Err(reason) => {
            trace(&format!("skip: {reason:?}"));
            return LinkPreviewOutcome::Skipped;
        }
    };
    let page = match fetcher.page(&target) {
        Ok(page) => page,
        Err(FetchError::Transient) => {
            let wait = RETRY_BASE_MS.saturating_mul(4i64.saturating_pow(job.attempts.min(8)));
            return LinkPreviewOutcome::Retry { retry_at: ctx.clock.now().saturating_add(wait) };
        }
        Err(e) => {
            trace(&format!("page: {e:?}"));
            return LinkPreviewOutcome::Failed;
        }
    };
    let meta = link::parse_html(&page.body, page.charset.as_deref(), &page.url);
    let site = meta.site_name.clone().unwrap_or_else(|| link::display_host(&page.url));
    let image = if still_on(ctx) { preview_image(fetcher, images, &meta, &page.url, || still_on(ctx)) } else { None };
    if meta.title.is_none() && image.is_none() {
        // Nothing to show beyond the link itself.
        return LinkPreviewOutcome::Failed;
    }
    LinkPreviewOutcome::Ready(FetchedPreview { title: meta.title, site, image })
}

/// The page's preview image, else its icon, else `/favicon.ico`; at most
/// one successful download, and none once `go_on` turns false.
fn preview_image(
    fetcher: &dyn Fetcher,
    images: &dyn ImageDecoder,
    meta: &PageMeta,
    page: &Url,
    go_on: impl Fn() -> bool,
) -> Option<(LinkImageKind, Vec<u8>)> {
    let mut candidates: Vec<(LinkImageKind, Url, u32)> = Vec::new();
    if let Some(u) = &meta.image {
        candidates.push((LinkImageKind::Photo, u.clone(), PHOTO_MAX_SIDE));
    }
    match &meta.icon {
        Some(u) => candidates.push((LinkImageKind::Icon, u.clone(), ICON_MAX_SIDE)),
        None => {
            if let Some(u) = link::favicon_url(page) {
                candidates.push((LinkImageKind::Icon, u, ICON_MAX_SIDE));
            }
        }
    }
    for (kind, url, side) in candidates {
        if !go_on() {
            return None;
        }
        let Ok(bytes) = fetcher.image(&url) else { continue };
        if let Some(png) = images.thumbnail(&bytes, side) {
            return Some((kind, png));
        }
    }
    None
}

/// Starts the workers. They sleep on `ctx.link_wake` until a job is queued
/// or due, so they cost nothing while idle or while the setting is off.
pub fn spawn(ctx: Arc<Ctx>, fetcher: Arc<dyn Fetcher>, images: Arc<dyn ImageDecoder>) -> std::io::Result<()> {
    ctx.link_wake.start_backfill(ctx.clock.now().saturating_add(BACKFILL_EVERY_MS));
    for n in 0..WORKERS {
        let (ctx, fetcher, images) = (ctx.clone(), fetcher.clone(), images.clone());
        std::thread::Builder::new()
            .name(format!("satchel-link-preview-{n}"))
            .spawn(move || worker(&ctx, fetcher.as_ref(), images.as_ref()))?;
    }
    Ok(())
}

fn worker(ctx: &Ctx, fetcher: &dyn Fetcher, images: &dyn ImageDecoder) {
    loop {
        let seen = ctx.link_wake.generation();
        match step(ctx, fetcher, images) {
            Step::Ran { .. } | Step::Backfilled(_) => {}
            Step::Off | Step::Idle(None) => ctx.link_wake.wait(seen, None),
            Step::Idle(Some(due)) => {
                let wait = (due - ctx.clock.now()).clamp(1000, 24 * 60 * 60 * 1000);
                ctx.link_wake.wait(seen, Some(Duration::from_millis(wait as u64)));
            }
            Step::Failed => ctx.link_wake.wait(seen, Some(Duration::from_secs(60))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Harness, Log, sample_settings};
    use satchel_core::{NewClip, InsertOutcome, Settings, SourceApp};
    use std::sync::atomic::Ordering;

    /// Serves canned pages and images by URL and logs every request.
    pub struct FakeFetcher {
        pub log: Arc<Log>,
        pages: Vec<(String, Result<Page, FetchError>)>,
        images: Vec<(String, Vec<u8>)>,
    }

    impl FakeFetcher {
        fn new(log: Arc<Log>) -> Self {
            Self { log, pages: Vec::new(), images: Vec::new() }
        }
        fn page(mut self, url: &str, html: &str) -> Self {
            let page = Page { url: Url::parse(url).unwrap(), body: html.as_bytes().to_vec(), charset: None };
            self.pages.push((url.into(), Ok(page)));
            self
        }
        fn failing(mut self, url: &str, e: FetchError) -> Self {
            self.pages.push((url.into(), Err(e)));
            self
        }
        fn image(mut self, url: &str, bytes: &[u8]) -> Self {
            self.images.push((url.into(), bytes.to_vec()));
            self
        }
    }

    impl Fetcher for FakeFetcher {
        fn page(&self, url: &Url) -> Result<Page, FetchError> {
            self.log.push(format!("GET page {url}"));
            self.pages.iter().find(|(u, _)| u == url.as_str()).map(|(_, r)| r.clone()).unwrap_or(Err(FetchError::Permanent))
        }
        fn image(&self, url: &Url) -> Result<Vec<u8>, FetchError> {
            self.log.push(format!("GET image {url}"));
            self.images.iter().find(|(u, _)| u == url.as_str()).map(|(_, b)| b.clone()).ok_or(FetchError::Permanent)
        }
    }

    /// "Decodes" bytes that start with `img` into `png:<side>:<rest>`.
    struct FakeImages;
    impl ImageDecoder for FakeImages {
        fn thumbnail(&self, bytes: &[u8], max_side: u32) -> Option<Vec<u8>> {
            let rest = bytes.strip_prefix(b"img")?;
            Some([format!("png:{max_side}:").as_bytes(), rest].concat())
        }
    }

    fn on(h: &Harness) {
        h.ctx.set_settings(Settings { link_previews: true, ..sample_settings() });
    }

    fn requests(log: &Log) -> Vec<String> {
        log.entries().into_iter().filter(|e| e.starts_with("GET ")).collect()
    }

    const ARTICLE: &str = r#"<head><title>Plain</title><meta property="og:title" content="An Article">
        <meta property="og:site_name" content="The Paper"><meta property="og:image" content="/cover.jpg"></head>"#;

    fn copy_link(h: &Harness, url: &str) -> ClipId {
        let id = h.seed(ClipKind::Link, url);
        on_copied(&h.ctx, id, ClipKind::Link);
        id
    }

    #[test]
    fn nothing_is_queued_or_fetched_while_the_setting_is_off() {
        let h = Harness::with_real_core();
        assert!(!h.ctx.settings().link_previews, "the harness starts with it off");
        h.ctx.link_wake.start_backfill(0);
        let fetcher = FakeFetcher::new(h.log.clone()).page("https://www.example.com/a", ARTICLE);
        let id = copy_link(&h, "https://www.example.com/a");
        assert_eq!(step(&h.ctx, &fetcher, &FakeImages), Step::Off);
        // Even a job queued while it was on is not claimed once it is off.
        on(&h);
        on_copied(&h.ctx, id, ClipKind::Link);
        h.ctx.set_settings(sample_settings());
        assert_eq!(step(&h.ctx, &fetcher, &FakeImages), Step::Off);
        assert!(requests(&h.log).is_empty(), "{:?}", h.log.entries());
        assert!(h.ctx.store.lock().list(&Default::default()).unwrap()[0].link_preview.is_none());
    }

    #[test]
    fn a_copied_link_gets_title_site_and_image_once() {
        let h = Harness::with_real_core();
        on(&h);
        let fetcher = FakeFetcher::new(h.log.clone())
            .page("https://www.example.com/a", ARTICLE)
            .image("https://www.example.com/cover.jpg", b"imgcover");
        let id = copy_link(&h, "https://www.example.com/a?utm_source=news");
        h.log.clear();
        assert_eq!(step(&h.ctx, &fetcher, &FakeImages), Step::Ran { id, ready: true });
        assert_eq!(requests(&h.log), vec!["GET page https://www.example.com/a", "GET image https://www.example.com/cover.jpg"]);
        assert!(h.log.entries().contains(&format!("event clips-changed updated {id}")));
        let p = h.ctx.store.lock().list(&Default::default()).unwrap()[0].link_preview.clone().unwrap();
        assert_eq!(p.title.as_deref(), Some("An Article"));
        assert_eq!(p.site, "The Paper");
        assert_eq!(p.image, Some(LinkImageKind::Photo));
        assert_eq!(h.ctx.store.lock().link_preview_image(id).unwrap(), Some(b"png:480:cover".to_vec()));
        // Copying it again does not fetch again.
        h.log.clear();
        on_copied(&h.ctx, id, ClipKind::Link);
        assert_eq!(step(&h.ctx, &fetcher, &FakeImages), Step::Idle(None));
        assert!(requests(&h.log).is_empty());
    }

    #[test]
    fn falls_back_to_icon_then_favicon_and_host() {
        let h = Harness::with_real_core();
        on(&h);
        let fetcher = FakeFetcher::new(h.log.clone())
            .page("https://www.example.com/b", r#"<title>B</title><meta property="og:image" content="/broken.jpg"><link rel=icon href=/i.png>"#)
            .image("https://www.example.com/broken.jpg", b"not an image")
            .image("https://www.example.com/i.png", b"imgicon")
            .page("https://news.example.org/c", "<title>C</title>")
            .image("https://news.example.org/favicon.ico", b"imgfav");
        let b = copy_link(&h, "https://www.example.com/b");
        let c = copy_link(&h, "https://news.example.org/c");
        step(&h.ctx, &fetcher, &FakeImages);
        step(&h.ctx, &fetcher, &FakeImages);
        let store = h.ctx.store.lock();
        let list = store.list(&Default::default()).unwrap();
        let pb = list.iter().find(|s| s.id == b).unwrap().link_preview.clone().unwrap();
        assert_eq!((pb.site.as_str(), pb.image), ("example.com", Some(LinkImageKind::Icon)));
        assert_eq!(store.link_preview_image(b).unwrap(), Some(b"png:128:icon".to_vec()));
        let pc = list.iter().find(|s| s.id == c).unwrap().link_preview.clone().unwrap();
        assert_eq!((pc.site.as_str(), pc.image), ("news.example.org", Some(LinkImageKind::Icon)));
    }

    #[test]
    fn risky_links_and_excluded_apps_are_never_fetched() {
        let h = Harness::with_real_core();
        on(&h);
        let fetcher = FakeFetcher::new(h.log.clone());
        for url in [
            "https://example.com/reset-password?token=abc123",
            "https://192.168.1.1/admin",
            "https://localhost/x",
            "https://user:pw@example.com/",
            "https://bucket.s3.amazonaws.com/f?X-Amz-Signature=abc",
            "https://example.com/magic-link/a1b2c3d4e5f6g7h8i9",
            "http://printer.local/",
        ] {
            copy_link(&h, url);
        }
        // A link copied from an app excluded since.
        let clip = NewClip {
            kind: ClipKind::Link,
            text: Some("https://www.example.com/fine".into()),
            html: None,
            rtf: None,
            image_png: None,
            thumbnail_png: None,
            file_paths: vec![],
            source_app: Some(SourceApp { id: "com.apple.TextEdit".into(), name: "TextEdit".into() }),
        };
        let (InsertOutcome::Inserted(c) | InsertOutcome::Bumped(c)) = h.ctx.store.lock().insert(clip, 5).unwrap();
        on_copied(&h.ctx, c.id, ClipKind::Link);
        h.ctx.set_settings(Settings {
            link_previews: true,
            excluded_app_ids: vec!["com.apple.TextEdit".into()],
            ..sample_settings()
        });
        for _ in 0..8 {
            assert!(matches!(step(&h.ctx, &fetcher, &FakeImages), Step::Ran { ready: false, .. }));
        }
        assert_eq!(step(&h.ctx, &fetcher, &FakeImages), Step::Idle(None));
        assert!(requests(&h.log).is_empty(), "{:?}", requests(&h.log));
    }

    #[test]
    fn older_links_are_backfilled_slowly_newest_first() {
        let h = Harness::with_real_core();
        // In the history before previews were on.
        let old = h.seed(ClipKind::Link, "https://www.example.com/old");
        let new = h.seed(ClipKind::Link, "https://www.example.com/new");
        let fetcher = FakeFetcher::new(h.log.clone())
            .page("https://www.example.com/old", "<title>Old</title>")
            .page("https://www.example.com/new", "<title>New</title>");
        on(&h);
        let start = h.ctx.clock.now();
        h.ctx.link_wake.start_backfill(start + BACKFILL_EVERY_MS);
        assert_eq!(step(&h.ctx, &fetcher, &FakeImages), Step::Idle(Some(start + BACKFILL_EVERY_MS)), "not at launch");
        h.clock.now.fetch_add(BACKFILL_EVERY_MS, Ordering::SeqCst);
        assert_eq!(step(&h.ctx, &fetcher, &FakeImages), Step::Backfilled(new));
        assert_eq!(step(&h.ctx, &fetcher, &FakeImages), Step::Ran { id: new, ready: true });
        let now = h.ctx.clock.now();
        assert_eq!(step(&h.ctx, &fetcher, &FakeImages), Step::Idle(Some(now + BACKFILL_EVERY_MS)), "paced");
        assert_eq!(requests(&h.log).iter().filter(|r| r.starts_with("GET page")).count(), 1);
        h.clock.now.fetch_add(BACKFILL_EVERY_MS, Ordering::SeqCst);
        assert_eq!(step(&h.ctx, &fetcher, &FakeImages), Step::Backfilled(old));
        assert_eq!(step(&h.ctx, &fetcher, &FakeImages), Step::Ran { id: old, ready: true });
        h.clock.now.fetch_add(BACKFILL_EVERY_MS, Ordering::SeqCst);
        assert_eq!(step(&h.ctx, &fetcher, &FakeImages), Step::Idle(None), "backfill done; sleeps until woken");
        // Off: the backfill does nothing either.
        h.ctx.set_settings(sample_settings());
        h.ctx.link_wake.start_backfill(0);
        assert_eq!(step(&h.ctx, &fetcher, &FakeImages), Step::Off);
    }

    #[test]
    fn transient_failures_back_off_and_permanent_ones_stop() {
        let h = Harness::with_real_core();
        on(&h);
        let fetcher = FakeFetcher::new(h.log.clone())
            .failing("https://www.example.com/offline", FetchError::Transient)
            .failing("https://www.example.com/gone", FetchError::Permanent);
        let offline = copy_link(&h, "https://www.example.com/offline");
        copy_link(&h, "https://www.example.com/gone");
        step(&h.ctx, &fetcher, &FakeImages);
        step(&h.ctx, &fetcher, &FakeImages);
        let now = h.ctx.clock.now();
        assert_eq!(step(&h.ctx, &fetcher, &FakeImages), Step::Idle(Some(now + RETRY_BASE_MS)));
        h.clock.now.fetch_add(RETRY_BASE_MS, Ordering::SeqCst);
        assert_eq!(step(&h.ctx, &fetcher, &FakeImages), Step::Ran { id: offline, ready: false });
        assert_eq!(step(&h.ctx, &fetcher, &FakeImages), Step::Idle(Some(now + RETRY_BASE_MS + 4 * RETRY_BASE_MS)));
        h.clock.now.fetch_add(4 * RETRY_BASE_MS, Ordering::SeqCst);
        step(&h.ctx, &fetcher, &FakeImages);
        assert_eq!(step(&h.ctx, &fetcher, &FakeImages), Step::Idle(None), "gave up after three attempts");
        let pages: Vec<_> = requests(&h.log);
        assert_eq!(pages.iter().filter(|r| r.ends_with("/offline")).count(), 3);
        assert_eq!(pages.iter().filter(|r| r.ends_with("/gone")).count(), 1);
    }

    #[test]
    fn a_page_with_nothing_to_show_is_not_a_preview() {
        let h = Harness::with_real_core();
        on(&h);
        let fetcher = FakeFetcher::new(h.log.clone()).page("https://www.example.com/empty", "<p>no head</p>");
        let id = copy_link(&h, "https://www.example.com/empty");
        assert_eq!(step(&h.ctx, &fetcher, &FakeImages), Step::Ran { id, ready: false });
        assert!(h.ctx.store.lock().list(&Default::default()).unwrap()[0].link_preview.is_none());
    }

    #[test]
    fn other_kinds_are_not_queued() {
        let h = Harness::with_real_core();
        on(&h);
        let id = h.seed(ClipKind::Text, "https://www.example.com/ but with words");
        on_copied(&h.ctx, id, ClipKind::Text);
        assert_eq!(step(&h.ctx, &FakeFetcher::new(h.log.clone()), &FakeImages), Step::Idle(None));
    }

    #[test]
    fn system_images_scale_allowed_formats_and_refuse_others() {
        let big = crate::thumbnail::tests::png(1200, 600);
        let out = SystemImages.thumbnail(&big, PHOTO_MAX_SIDE).unwrap();
        assert_eq!(crate::thumbnail::png_dimensions(&out), Some((480, 240)));
        let small = crate::thumbnail::tests::png(32, 32);
        let out = SystemImages.thumbnail(&small, ICON_MAX_SIDE).unwrap();
        assert_eq!(crate::thumbnail::png_dimensions(&out), Some((32, 32)), "never upscaled");
        assert_eq!(SystemImages.thumbnail(b"<svg xmlns='http://www.w3.org/2000/svg'/>", 128), None);
        assert_eq!(SystemImages.thumbnail(b"II*\0 tiff", 128), None);
        assert_eq!(SystemImages.thumbnail(b"%PDF-1.7", 128), None);
        assert_eq!(SystemImages.thumbnail(b"\x89PNG\r\n\x1a\ntruncated", 128), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn system_images_read_jpeg_via_imageio() {
        use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep};
        use objc2_foundation::{NSData, NSDictionary};
        let png = crate::thumbnail::tests::png(64, 40);
        let rep = NSBitmapImageRep::imageRepWithData(&NSData::with_bytes(&png)).unwrap();
        let props = NSDictionary::new();
        // SAFETY: an empty dictionary satisfies the element types.
        let jpeg = unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::JPEG, &props) }.unwrap().to_vec();
        let out = SystemImages.thumbnail(&jpeg, 128).unwrap();
        assert_eq!(crate::thumbnail::png_dimensions(&out), Some((64, 40)));
    }

    #[test]
    fn wake_does_not_lose_a_notify_between_check_and_wait() {
        let w = Arc::new(Wake::default());
        let seen = w.generation();
        w.notify();
        // Returns at once: the generation moved after `seen` was read.
        let started = std::time::Instant::now();
        w.wait(seen, Some(Duration::from_secs(5)));
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
