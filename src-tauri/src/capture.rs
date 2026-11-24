//! What happens when the clipboard changes: self-write suppression, the
//! privacy gate (before any content is read), reading, classification,
//! thumbnailing, storing, announcing and periodic pruning.

use satchel_core::{
    ClipId, ClipKind, InsertOutcome, MAX_IMAGE_BYTES, MAX_TEXT_BYTES, Millis, NewClip, SourceApp,
    privacy::CaptureContext,
};

use crate::clipboard::{self, ClipboardSource, MAX_FILES, MAX_PATH_BYTES, Payload, ReadLimits};
use crate::ctx::{Ctx, Policy};
use crate::events::ChangeReason;
use crate::thumbnail;

pub const THUMBNAIL_MAX_SIDE: u32 = 320;
/// Prune after this many new clips, or after `PRUNE_INTERVAL_MS`.
pub const PRUNE_EVERY_INSERTS: u32 = 25;
pub const PRUNE_INTERVAL_MS: Millis = 60 * 60 * 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Skip {
    /// Satchel wrote this change itself.
    OwnWrite,
    /// Paused, excluded app, concealed/transient marker...
    Refused,
    /// Nothing Satchel keeps (unknown types, empty text, undecodable image).
    Nothing,
    /// Over a size cap: not stored, and announced (`capture-skipped`) so the
    /// shelf can say so.
    TooLarge,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Unchanged,
    Skipped(Skip),
    Stored { reason: ChangeReason, id: ClipId },
    /// The store refused or failed; nothing was announced.
    Failed,
}

pub struct Capturer<S: ClipboardSource> {
    source: S,
    last_seen: i64,
    inserts_since_prune: u32,
    last_prune: Millis,
}

impl<S: ClipboardSource> Capturer<S> {
    /// Starts from the current clipboard state: whatever is already on the
    /// clipboard at launch is not captured, because its source app cannot be
    /// known any more (when in doubt, do not capture).
    pub fn new(mut source: S, now: Millis) -> Self {
        let last_seen = source.change_count();
        Self { source, last_seen, inserts_since_prune: 0, last_prune: now }
    }

    /// One watcher tick. Allocation-free when nothing changed.
    pub fn poll(&mut self, ctx: &Ctx) -> Outcome {
        let count = self.source.change_count();
        if count == self.last_seen {
            return Outcome::Unchanged;
        }
        self.last_seen = count;
        let outcome = self.handle_change(ctx, count);
        if crate::shelf::tracing() {
            // Content-free: the change count and the outcome only.
            eprintln!("[satchel] capture change={count} outcome={outcome:?}");
        }
        if matches!(outcome, Outcome::Stored { reason: ChangeReason::Inserted, .. }) {
            self.inserts_since_prune += 1;
        }
        let now = ctx.clock.now();
        if self.inserts_since_prune >= PRUNE_EVERY_INSERTS
            || now - self.last_prune >= PRUNE_INTERVAL_MS
        {
            self.prune(ctx);
        }
        outcome
    }

    /// Applies retention now. Returns the number of clips removed.
    pub fn prune(&mut self, ctx: &Ctx) -> u64 {
        let now = ctx.clock.now();
        self.inserts_since_prune = 0;
        self.last_prune = now;
        let settings = ctx.settings();
        let removed = ctx.store.lock().prune(&settings, now).unwrap_or(0);
        if removed > 0 {
            ctx.events.clips_changed(ChangeReason::Deleted, None);
        }
        removed
    }

    fn handle_change(&mut self, ctx: &Ctx, count: i64) -> Outcome {
        // Cheap refusals first; they can only ever prevent a capture.
        if ctx.self_writes.is_self(count) {
            return Outcome::Skipped(Skip::OwnWrite);
        }
        let settings = ctx.settings();
        if settings.capture_paused {
            return Outcome::Skipped(Skip::Refused);
        }

        let formats = self.source.format_names();
        if crate::shelf::tracing() {
            // Type names only (never data), to see what a source app wrote.
            eprintln!("[satchel] capture types={}", formats.join(","));
        }
        let source_app = ctx.desktop.frontmost_app();
        let format_refs: Vec<&str> = formats.iter().map(String::as_str).collect();
        let gate = CaptureContext {
            source_app_id: source_app.as_ref().map(|a| a.id.as_str()),
            format_names: &format_refs,
            written_by_self: clipboard::has_self_marker(&formats),
        };
        if !ctx.policy.should_capture(&gate, &settings) {
            return Outcome::Skipped(Skip::Refused);
        }

        let limits = ReadLimits {
            max_text_bytes: MAX_TEXT_BYTES,
            max_image_bytes: MAX_IMAGE_BYTES,
            images: settings.capture_images,
        };
        let Some(payload) = self.source.read(&limits) else {
            return Outcome::Skipped(Skip::Nothing);
        };
        if let Payload::TooLarge { kind, limit_bytes } = payload {
            ctx.events.capture_skipped(kind, limit_bytes);
            return Outcome::Skipped(Skip::TooLarge);
        }
        let app = source_app.map(|a| SourceApp { id: a.id, name: a.name });
        let Some(clip) = build_clip(payload, app, ctx.policy.as_ref(), &limits) else {
            return Outcome::Skipped(Skip::Nothing);
        };

        let now = ctx.clock.now();
        let result = ctx.store.lock().insert(clip, now);
        match result {
            Ok(InsertOutcome::Inserted(s)) => {
                ctx.events.clips_changed(ChangeReason::Inserted, Some(s.id));
                ctx.events.clip_saved();
                crate::link_preview::on_copied(ctx, s.id, s.kind);
                Outcome::Stored { reason: ChangeReason::Inserted, id: s.id }
            }
            Ok(InsertOutcome::Bumped(s)) => {
                ctx.events.clips_changed(ChangeReason::Bumped, Some(s.id));
                ctx.events.clip_saved();
                crate::link_preview::on_copied(ctx, s.id, s.kind);
                Outcome::Stored { reason: ChangeReason::Bumped, id: s.id }
            }
            Err(_) => Outcome::Failed,
        }
    }
}

/// Turns a payload into a `NewClip`, re-checking the caps (the reader
/// should already have honored them) and making the thumbnail.
pub fn build_clip(
    payload: Payload,
    source_app: Option<SourceApp>,
    policy: &dyn Policy,
    limits: &ReadLimits,
) -> Option<NewClip> {
    let empty = NewClip {
        kind: ClipKind::Text,
        text: None,
        html: None,
        rtf: None,
        image_png: None,
        thumbnail_png: None,
        file_paths: Vec::new(),
        source_app,
    };
    match payload {
        Payload::Files(paths) => {
            let file_paths: Vec<String> = paths
                .into_iter()
                .filter(|p| !p.is_empty() && p.len() <= MAX_PATH_BYTES)
                .take(MAX_FILES)
                .collect();
            if file_paths.is_empty() {
                return None;
            }
            Some(NewClip { kind: ClipKind::File, file_paths, ..empty })
        }
        Payload::Text { text, html, rtf } => {
            if text.trim().is_empty() || text.len() as u64 > limits.max_text_bytes {
                return None;
            }
            let html = html.filter(|h| !h.is_empty() && h.len() as u64 <= limits.max_text_bytes);
            let rtf = rtf.filter(|r| !r.is_empty() && r.len() as u64 <= limits.max_text_bytes);
            let kind = policy.classify_text(&text);
            Some(NewClip { kind, text: Some(text), html, rtf, ..empty })
        }
        Payload::Image { png } => {
            if !limits.images || png.is_empty() || png.len() as u64 > limits.max_image_bytes {
                return None;
            }
            // A PNG that cannot be decoded is not stored at all. One too big
            // to thumbnail (an interlaced image whose full frame is over the
            // thumbnailer's budget) is kept without a thumbnail: the card
            // shows an image icon and it still pastes.
            let thumb = match thumbnail::png_thumbnail(&png, THUMBNAIL_MAX_SIDE) {
                Ok(t) => Some(t),
                Err(thumbnail::ThumbnailError::TooLarge) => None,
                Err(_) => return None,
            };
            Some(NewClip {
                kind: ClipKind::Image,
                image_png: Some(png),
                thumbnail_png: thumb,
                ..empty
            })
        }
        Payload::TooLarge { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use super::*;
    use crate::clipboard::WriteRequest;
    use crate::platform::Clock;
    use crate::testing::{FakeStore, Harness};

    const TEXT: &str = "public.utf8-plain-text";

    fn text(s: &str) -> Payload {
        Payload::Text { text: s.into(), html: None, rtf: None }
    }

    #[test]
    fn unchanged_clipboard_does_nothing() {
        let h = Harness::new();
        let mut cap = Capturer::new(h.source(), 0);
        assert_eq!(cap.poll(&h.ctx), Outcome::Unchanged);
        assert_eq!(cap.poll(&h.ctx), Outcome::Unchanged);
        assert!(h.log.entries().is_empty());
    }

    #[test]
    fn content_already_on_the_clipboard_at_start_is_not_captured() {
        let h = Harness::new();
        h.board.external_copy(&[TEXT], text("before launch"));
        let mut cap = Capturer::new(h.source(), 0);
        assert_eq!(cap.poll(&h.ctx), Outcome::Unchanged);
    }

    #[test]
    fn copied_text_is_gated_then_read_then_stored_and_announced() {
        let h = Harness::new();
        let mut cap = Capturer::new(h.source(), 0);
        h.board.external_copy(&[TEXT], text("hello"));

        let outcome = cap.poll(&h.ctx);
        let Outcome::Stored { reason: ChangeReason::Inserted, id } = outcome else {
            panic!("{outcome:?}")
        };
        assert_eq!(
            h.log.entries(),
            vec![
                "read formats".to_string(),
                format!("gate app=com.apple.TextEdit formats={TEXT} self=false"),
                "read content images=true".to_string(),
                format!("event clips-changed inserted {id}"),
            ]
        );
        let content = h.ctx.store.lock().content(id).unwrap().unwrap();
        assert_eq!(content.text.as_deref(), Some("hello"));
        assert_eq!(content.kind, ClipKind::Text);
        assert_eq!(cap.poll(&h.ctx), Outcome::Unchanged);
    }

    #[test]
    fn source_app_is_recorded() {
        let h = Harness::new();
        let mut cap = Capturer::new(h.source(), 0);
        h.board.external_copy(&[TEXT], text("https://example.com"));
        cap.poll(&h.ctx);
        let list = h.ctx.store.lock().list(&Default::default()).unwrap();
        assert_eq!(list[0].kind, ClipKind::Link);
        assert_eq!(list[0].source_app.as_ref().unwrap().id, "com.apple.TextEdit");
        assert_eq!(list[0].source_app.as_ref().unwrap().name, "TextEdit");
    }

    #[test]
    fn copied_links_are_queued_for_a_preview_only_with_the_setting_on() {
        let h = Harness::with_real_core();
        let mut cap = Capturer::new(h.source(), 0);
        h.board.external_copy(&[TEXT], text("https://www.example.com/off"));
        cap.poll(&h.ctx);
        assert_eq!(h.ctx.store.lock().next_link_preview_at().unwrap(), None, "off: nothing queued");
        h.ctx.set_settings(satchel_core::Settings { link_previews: true, ..crate::testing::sample_settings() });
        h.board.external_copy(&[TEXT], text("just words, not a link"));
        cap.poll(&h.ctx);
        assert_eq!(h.ctx.store.lock().next_link_preview_at().unwrap(), None, "not a link");
        h.board.external_copy(&[TEXT], text("https://www.example.com/on"));
        cap.poll(&h.ctx);
        assert!(h.ctx.store.lock().next_link_preview_at().unwrap().is_some(), "on: queued");
        // Re-copying the link copied while off queues it now.
        h.board.external_copy(&[TEXT], text("https://www.example.com/off"));
        cap.poll(&h.ctx);
        let job = h.ctx.store.lock().claim_link_preview(i64::MAX - 10, 0).unwrap().unwrap();
        let job2 = h.ctx.store.lock().claim_link_preview(i64::MAX - 10, 0).unwrap().unwrap();
        let mut urls = vec![job.url, job2.url];
        urls.sort();
        assert_eq!(urls, vec!["https://www.example.com/off", "https://www.example.com/on"]);
    }

    #[test]
    fn refused_changes_are_never_read() {
        let h = Harness::new();
        h.policy.allow.store(false, Ordering::SeqCst);
        let mut cap = Capturer::new(h.source(), 0);
        h.board.external_copy(&["org.nspasteboard.ConcealedType", TEXT], text("secret"));
        assert_eq!(cap.poll(&h.ctx), Outcome::Skipped(Skip::Refused));
        assert!(!h.log.entries().iter().any(|e| e.starts_with("read content")));
        assert!(h.ctx.store.lock().list(&Default::default()).unwrap().is_empty());
    }

    #[test]
    fn paused_capture_reads_nothing_at_all() {
        let h = Harness::new();
        let mut s = h.ctx.settings();
        s.capture_paused = true;
        h.ctx.set_settings(s);
        let mut cap = Capturer::new(h.source(), 0);
        h.board.external_copy(&[TEXT], text("while paused"));
        assert_eq!(cap.poll(&h.ctx), Outcome::Skipped(Skip::Refused));
        assert!(h.log.entries().is_empty());
        // Unpausing later does not resurrect the change.
        let mut s = h.ctx.settings();
        s.capture_paused = false;
        h.ctx.set_settings(s);
        assert_eq!(cap.poll(&h.ctx), Outcome::Unchanged);
    }

    #[test]
    fn own_writes_are_ignored_by_change_count() {
        let h = Harness::new();
        let mut cap = Capturer::new(h.source(), 0);
        h.ctx
            .self_writes
            .write_with(|| {
                h.ctx.clipboard.write(&WriteRequest { text: Some("mine".into()), ..Default::default() })
            })
            .unwrap();
        assert_eq!(cap.poll(&h.ctx), Outcome::Skipped(Skip::OwnWrite));
        assert!(!h.log.entries().iter().any(|e| e.starts_with("read")));
    }

    #[test]
    fn own_marker_is_passed_to_the_gate() {
        let h = Harness::new();
        let mut cap = Capturer::new(h.source(), 0);
        // Written by us but not recorded (e.g. by another Satchel process).
        h.board.external_copy(&[TEXT, clipboard::SELF_MARKER_TYPE], text("mine"));
        assert_eq!(cap.poll(&h.ctx), Outcome::Skipped(Skip::Refused));
        assert!(h.log.entries().iter().any(|e| e.ends_with("self=true")));
    }

    #[test]
    fn same_text_twice_bumps() {
        let h = Harness::new();
        let mut cap = Capturer::new(h.source(), 0);
        h.board.external_copy(&[TEXT], text("again"));
        let Outcome::Stored { id, .. } = cap.poll(&h.ctx) else { panic!() };
        h.board.external_copy(&[TEXT], text("again"));
        assert_eq!(cap.poll(&h.ctx), Outcome::Stored { reason: ChangeReason::Bumped, id });
        assert!(h.log.entries().contains(&format!("event clips-changed bumped {id}")));
    }

    #[test]
    fn a_stored_or_recopied_clip_animates_the_menu_bar_icon() {
        let saved = |h: &Harness| h.clips_saved.load(Ordering::SeqCst);
        let h = Harness::new();
        let mut cap = Capturer::new(h.source(), 0);

        h.board.external_copy(&[TEXT], text("new"));
        assert!(matches!(cap.poll(&h.ctx), Outcome::Stored { reason: ChangeReason::Inserted, .. }));
        assert_eq!(saved(&h), 1);

        // A re-copy of a clip already in history moves it to the top and
        // animates too.
        h.board.external_copy(&[TEXT], text("new"));
        assert!(matches!(cap.poll(&h.ctx), Outcome::Stored { reason: ChangeReason::Bumped, .. }));
        assert_eq!(saved(&h), 2);

        // Nothing changed, empty text, a copy over the cap, Satchel's own
        // write, an excluded or concealed copy: none of these animate.
        assert_eq!(cap.poll(&h.ctx), Outcome::Unchanged);
        h.board.external_copy(&[TEXT], text("  "));
        assert_eq!(cap.poll(&h.ctx), Outcome::Skipped(Skip::Nothing));
        h.board.external_copy(&[TEXT], Payload::TooLarge { kind: ClipKind::Text, limit_bytes: 1 });
        assert_eq!(cap.poll(&h.ctx), Outcome::Skipped(Skip::TooLarge));
        h.ctx
            .self_writes
            .write_with(|| {
                h.ctx.clipboard.write(&WriteRequest { text: Some("mine".into()), ..Default::default() })
            })
            .unwrap();
        assert_eq!(cap.poll(&h.ctx), Outcome::Skipped(Skip::OwnWrite));
        h.policy.allow.store(false, Ordering::SeqCst);
        h.board.external_copy(&[TEXT], text("from an excluded app"));
        assert_eq!(cap.poll(&h.ctx), Outcome::Skipped(Skip::Refused));
        h.board.external_copy(&[TEXT], text("new"));
        assert_eq!(cap.poll(&h.ctx), Outcome::Skipped(Skip::Refused));
        h.policy.allow.store(true, Ordering::SeqCst);
        assert_eq!(saved(&h), 2);

        // Paused: nothing, neither for new content nor for a re-copy.
        let mut s = h.ctx.settings();
        s.capture_paused = true;
        h.ctx.set_settings(s.clone());
        h.board.external_copy(&[TEXT], text("while paused"));
        assert_eq!(cap.poll(&h.ctx), Outcome::Skipped(Skip::Refused));
        h.board.external_copy(&[TEXT], text("new"));
        assert_eq!(cap.poll(&h.ctx), Outcome::Skipped(Skip::Refused));
        assert_eq!(saved(&h), 2);

        // Resumed: the next new clip animates again.
        s.capture_paused = false;
        h.ctx.set_settings(s);
        h.board.external_copy(&[TEXT], text("after resuming"));
        cap.poll(&h.ctx);
        assert_eq!(saved(&h), 3);
    }

    #[test]
    fn a_failed_store_does_not_animate() {
        let h = Harness::with_store(FakeStore::failing());
        let mut cap = Capturer::new(h.source(), 0);
        h.board.external_copy(&[TEXT], text("x"));
        assert_eq!(cap.poll(&h.ctx), Outcome::Failed);
        assert_eq!(h.clips_saved.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn whitespace_only_text_is_not_stored() {
        let h = Harness::new();
        let mut cap = Capturer::new(h.source(), 0);
        h.board.external_copy(&[TEXT], text(" \n\t "));
        assert_eq!(cap.poll(&h.ctx), Outcome::Skipped(Skip::Nothing));
    }

    #[test]
    fn images_respect_the_setting() {
        let h = Harness::new();
        let mut s = h.ctx.settings();
        s.capture_images = false;
        h.ctx.set_settings(s);
        let mut cap = Capturer::new(h.source(), 0);
        h.board.external_copy(&["public.png"], Payload::Image { png: thumbnail::tests::png(4, 4) });
        assert_eq!(cap.poll(&h.ctx), Outcome::Skipped(Skip::Nothing));
        assert!(h.log.entries().contains(&"read content images=false".to_string()));
    }

    #[test]
    fn images_get_a_thumbnail() {
        let h = Harness::new();
        let mut cap = Capturer::new(h.source(), 0);
        h.board.external_copy(&["public.png"], Payload::Image { png: thumbnail::tests::png(640, 320) });
        let Outcome::Stored { id, .. } = cap.poll(&h.ctx) else { panic!() };
        let thumb = h.ctx.store.lock().thumbnail(id).unwrap().unwrap();
        assert_eq!(thumbnail::tests::dimensions(&thumb), (320, 160));
    }

    #[test]
    fn undecodable_images_are_dropped() {
        let h = Harness::new();
        let mut cap = Capturer::new(h.source(), 0);
        h.board.external_copy(&["public.png"], Payload::Image { png: b"not a png".to_vec() });
        assert_eq!(cap.poll(&h.ctx), Outcome::Skipped(Skip::Nothing));
    }

    #[test]
    fn store_failure_is_reported_without_events() {
        let h = Harness::with_store(FakeStore::failing());
        let mut cap = Capturer::new(h.source(), 0);
        h.board.external_copy(&[TEXT], text("x"));
        assert_eq!(cap.poll(&h.ctx), Outcome::Failed);
        assert!(!h.log.entries().iter().any(|e| e.starts_with("event")));
    }

    #[test]
    fn prunes_after_enough_inserts_and_announces_deletions() {
        let h = Harness::new();
        let mut s = h.ctx.settings();
        s.max_items = 10;
        h.ctx.set_settings(s);
        let mut cap = Capturer::new(h.source(), h.clock.now());
        for i in 0..PRUNE_EVERY_INSERTS {
            h.board.external_copy(&[TEXT], text(&format!("clip {i}")));
            h.clock.now.fetch_add(1, Ordering::SeqCst);
            cap.poll(&h.ctx);
        }
        assert_eq!(h.ctx.store.lock().list(&Default::default()).unwrap().len(), 10);
        assert_eq!(h.log.entries().last().unwrap(), "event clips-changed deleted");
    }

    #[test]
    fn prunes_hourly_even_without_inserts() {
        let h = Harness::new();
        let mut cap = Capturer::new(h.source(), h.clock.now());
        h.clock.now.fetch_add(PRUNE_INTERVAL_MS, Ordering::SeqCst);
        h.board.external_copy(&[TEXT], text("x"));
        cap.poll(&h.ctx);
        assert_eq!(cap.inserts_since_prune, 0);
        assert_eq!(cap.last_prune, h.clock.now());
    }

    mod build {
        use super::*;
        use crate::testing::FakePolicy;
        use std::sync::Arc;
        use std::sync::atomic::AtomicBool;

        fn policy() -> FakePolicy {
            FakePolicy { log: Arc::new(Default::default()), allow: AtomicBool::new(true) }
        }

        fn limits() -> ReadLimits {
            ReadLimits { max_text_bytes: 10, max_image_bytes: 100_000, images: true }
        }

        #[test]
        fn text_over_the_cap_is_dropped() {
            assert!(build_clip(text("01234567890"), None, &policy(), &limits()).is_none());
            assert!(build_clip(text("0123456789"), None, &policy(), &limits()).is_some());
        }

        #[test]
        fn oversized_html_is_dropped_but_text_kept() {
            let clip = build_clip(
                Payload::Text { text: "hi".into(), html: Some("<p>far too long</p>".into()), rtf: Some(b"{\\rtf1 far too long}".to_vec()) },
                None,
                &policy(),
                &limits(),
            )
            .unwrap();
            assert_eq!(clip.text.as_deref(), Some("hi"));
            assert_eq!(clip.html, None);
            assert_eq!(clip.rtf, None);
        }

        #[test]
        fn files_are_bounded() {
            let long = "x".repeat(MAX_PATH_BYTES + 1);
            let clip = build_clip(
                Payload::Files(vec!["/a".into(), long, String::new()]),
                None,
                &policy(),
                &limits(),
            )
            .unwrap();
            assert_eq!(clip.kind, ClipKind::File);
            assert_eq!(clip.file_paths, vec!["/a".to_string()]);
            let many: Vec<String> = (0..MAX_FILES + 5).map(|i| format!("/f{i}")).collect();
            let clip = build_clip(Payload::Files(many), None, &policy(), &limits()).unwrap();
            assert_eq!(clip.file_paths.len(), MAX_FILES);
            assert!(build_clip(Payload::Files(vec![]), None, &policy(), &limits()).is_none());
        }

        #[test]
        fn images_too_big_to_thumbnail_are_kept_without_one() {
            // An interlaced PNG whose full frame is over the thumbnailer's
            // budget: pasteable, just no thumbnail.
            let w = 4100;
            let h = 4100;
            let png = thumbnail::tests::interlaced_png(w, h, [1, 2, 3, 255]);
            let lim = ReadLimits { max_image_bytes: 100_000_000, ..limits() };
            let clip = build_clip(Payload::Image { png: png.clone() }, None, &policy(), &lim).unwrap();
            assert_eq!(clip.kind, ClipKind::Image);
            assert_eq!(clip.image_png, Some(png));
            assert_eq!(clip.thumbnail_png, None);
        }

        #[test]
        fn images_over_the_cap_are_dropped() {
            let png = thumbnail::tests::png(8, 8);
            let small = ReadLimits { max_image_bytes: png.len() as u64 - 1, ..limits() };
            assert!(build_clip(Payload::Image { png: png.clone() }, None, &policy(), &small).is_none());
            assert!(build_clip(Payload::Image { png }, None, &policy(), &limits()).is_some());
        }
    }
}
