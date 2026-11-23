//! End-to-end tests of the app logic against the real `satchel_core`
//! (SQLite in memory, real privacy gate and classifier) with fake OS seams.

use satchel_core::{ClipKind, Query, Settings, Store};

use crate::capture::{Capturer, Outcome, Skip};
use crate::clipboard::Payload;
use crate::commands::api;
use crate::events::ChangeReason;
use crate::testing::Harness;
use crate::thumbnail;

const TEXT: &str = "public.utf8-plain-text";

fn copy_text(h: &Harness, cap: &mut Capturer<crate::testing::FakeSource>, text: &str) -> Outcome {
    h.clock.now.fetch_add(1000, std::sync::atomic::Ordering::SeqCst);
    h.board.external_copy(&[TEXT], Payload::Text { text: text.into(), html: None, rtf: None });
    cap.poll(&h.ctx)
}

#[test]
fn copy_capture_list_paste_round_trip() {
    let h = Harness::with_real_core();
    let mut cap = Capturer::new(h.source(), 0);

    let Outcome::Stored { reason: ChangeReason::Inserted, id } =
        copy_text(&h, &mut cap, "https://example.com/a")
    else {
        panic!("not stored")
    };
    let list = api::list_clips(&h.ctx, Query::default()).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, id);
    assert_eq!(list[0].kind, ClipKind::Link);
    assert_eq!(list[0].source_app.as_ref().unwrap().id, "com.apple.TextEdit");

    h.ctx.remember_previous_app(Some(42));
    api::paste_clips(&h.ctx, &[id], false).unwrap();
    assert!(h.log.entries().contains(&"keystroke paste".to_string()));
    // Our own write is not captured again.
    assert_eq!(cap.poll(&h.ctx), Outcome::Skipped(Skip::OwnWrite));
    assert_eq!(api::list_clips(&h.ctx, Query::default()).unwrap().len(), 1);
}

#[test]
fn real_gate_refuses_concealed_and_excluded_sources() {
    let h = Harness::with_real_core();
    let mut cap = Capturer::new(h.source(), 0);

    h.board.external_copy(
        &["org.nspasteboard.ConcealedType", TEXT],
        Payload::Text { text: "s3cret".into(), html: None, rtf: None },
    );
    assert_eq!(cap.poll(&h.ctx), Outcome::Skipped(Skip::Refused));

    let mut settings = h.ctx.settings();
    settings.excluded_app_ids = vec!["com.apple.TextEdit".into()];
    h.ctx.set_settings(settings);
    assert_eq!(copy_text(&h, &mut cap, "from an excluded app"), Outcome::Skipped(Skip::Refused));

    assert!(api::list_clips(&h.ctx, Query::default()).unwrap().is_empty());
    assert!(!h.log.entries().iter().any(|e| e.starts_with("read content")));
}

#[test]
fn duplicates_bump_in_the_real_store() {
    let h = Harness::with_real_core();
    let mut cap = Capturer::new(h.source(), 0);
    let Outcome::Stored { id, .. } = copy_text(&h, &mut cap, "same") else { panic!() };
    copy_text(&h, &mut cap, "other");
    assert_eq!(copy_text(&h, &mut cap, "same"), Outcome::Stored { reason: ChangeReason::Bumped, id });
    let list = api::list_clips(&h.ctx, Query::default()).unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].id, id);
}

#[test]
fn image_thumbnail_is_served_as_png_bytes() {
    let h = Harness::with_real_core();
    let mut cap = Capturer::new(h.source(), 0);
    h.board.external_copy(&["public.png"], Payload::Image { png: thumbnail::tests::png(1000, 500) });
    let Outcome::Stored { id, .. } = cap.poll(&h.ctx) else { panic!("image not stored") };
    let bytes = api::get_clip_thumbnail(&h.ctx, id).unwrap();
    assert_eq!(thumbnail::tests::dimensions(&bytes), (320, 160));
    let list = api::list_clips(&h.ctx, Query { kind: Some(ClipKind::Image), ..Query::default() }).unwrap();
    assert!(list[0].has_thumbnail);
}

#[test]
fn settings_and_pinboards_persist_in_the_real_store() {
    let h = Harness::with_real_core();
    let new = Settings { max_items: 500, shelf_shortcut: "Alt+Space".into(), ..Settings::default() };
    api::save_settings(&h.ctx, new.clone()).unwrap();
    assert_eq!(h.ctx.store.lock().settings().unwrap(), new);

    let mut cap = Capturer::new(h.source(), 0);
    let Outcome::Stored { id, .. } = copy_text(&h, &mut cap, "pin me") else { panic!() };
    let board = api::create_pinboard(&h.ctx, "Work", "#0A84FF").unwrap();
    api::pin_clip(&h.ctx, id, board.id).unwrap();
    assert_eq!(api::clear_history(&h.ctx).unwrap(), 0);
    let pinned = api::list_clips(&h.ctx, Query { pinboard_id: Some(board.id), ..Query::default() }).unwrap();
    assert_eq!(pinned.len(), 1);
}

#[test]
fn database_file_is_user_only() {
    let dir = std::env::temp_dir().join(format!("satchel-it-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("t.sqlite3");
    drop(Store::open(&path).unwrap());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn every_kind_is_captured_filtered_searched_and_pasted_back() {
    let h = Harness::with_real_core();
    let mut cap = Capturer::new(h.source(), 0);
    let mut copy = |types: &[&str], payload: Payload| {
        h.clock.now.fetch_add(1000, std::sync::atomic::Ordering::SeqCst);
        h.board.external_copy(types, payload);
        match cap.poll(&h.ctx) {
            Outcome::Stored { reason: ChangeReason::Inserted, id } => id,
            other => panic!("{other:?}"),
        }
    };
    let text = |t: &str| Payload::Text { text: t.into(), html: None, rtf: None };
    let rtf = b"{\\rtf1 \\b Quarterly}".to_vec();
    let png = thumbnail::tests::png(900, 300);
    let cases = [
        (ClipKind::Text, copy(&[TEXT], text("meeting notes for monday")), "monday"),
        (
            ClipKind::Text,
            copy(
                &[TEXT, "public.rtf", "public.html"],
                Payload::Text {
                    text: "Quarterly".into(),
                    html: Some("<b>Quarterly</b>".into()),
                    rtf: Some(rtf.clone()),
                },
            ),
            "quarterly",
        ),
        (ClipKind::Link, copy(&[TEXT], text("https://example.org/docs")), "example"),
        (ClipKind::Code, copy(&[TEXT], text("fn main() {\n    println!(\"hi\");\n}\n")), "println"),
        (ClipKind::Color, copy(&[TEXT], text("#0A84FF")), "0a84ff"),
        (ClipKind::Image, copy(&["public.png"], Payload::Image { png: png.clone() }), "image"),
        (
            ClipKind::File,
            copy(&["public.file-url"], Payload::Files(vec!["/Users/me/report.pdf".into(), "/Users/me/Photos".into()])),
            "report",
        ),
    ];
    for (kind, id, word) in &cases {
        let filtered = api::list_clips(&h.ctx, Query { kind: Some(*kind), ..Query::default() }).unwrap();
        assert!(filtered.iter().any(|c| c.id == *id), "{kind:?} not under its filter");
        let found = api::list_clips(
            &h.ctx,
            Query { kind: Some(*kind), text: Some((*word).into()), ..Query::default() },
        )
        .unwrap();
        assert_eq!(found.iter().map(|c| c.id).collect::<Vec<_>>(), vec![*id], "{kind:?} search {word}");
    }

    let written = |id, plain| {
        api::copy_clip(&h.ctx, id, plain).unwrap();
        h.board.written.lock().unwrap().clone().unwrap()
    };
    // Rich text: "Paste" carries the formatting, "Paste as Plain Text" not.
    let rich = written(cases[1].1, false);
    assert_eq!((rich.html.as_deref(), rich.rtf.as_deref()), (Some("<b>Quarterly</b>"), Some(&rtf[..])));
    let plain = written(cases[1].1, true);
    assert_eq!((plain.text.as_deref(), plain.html, plain.rtf), (Some("Quarterly"), None, None));
    // Links and colors paste as their text.
    assert_eq!(written(cases[2].1, false).text.as_deref(), Some("https://example.org/docs"));
    assert_eq!(written(cases[4].1, false).text.as_deref(), Some("#0A84FF"));
    // Images paste at full size, never the thumbnail.
    let img = written(cases[5].1, false);
    assert_eq!(img.png.as_deref(), Some(&png[..]));
    // Files paste as files, or as their paths when plain.
    assert_eq!(written(cases[6].1, false).files, vec!["/Users/me/report.pdf", "/Users/me/Photos"]);
    assert_eq!(written(cases[6].1, true).text.as_deref(), Some("/Users/me/report.pdf\n/Users/me/Photos"));
    // None of those writes came back as new clips.
    assert_eq!(cap.poll(&h.ctx), Outcome::Skipped(Skip::OwnWrite));
    assert_eq!(api::list_clips(&h.ctx, Query::default()).unwrap().len(), cases.len());
}

#[test]
fn oversized_copies_are_announced_not_stored() {
    let h = Harness::with_real_core();
    let mut cap = Capturer::new(h.source(), 0);
    h.board.external_copy(
        &["public.png"],
        Payload::TooLarge { kind: ClipKind::Image, limit_bytes: satchel_core::MAX_IMAGE_BYTES },
    );
    assert_eq!(cap.poll(&h.ctx), Outcome::Skipped(Skip::TooLarge));
    assert!(
        h.log.entries().contains(&format!("event capture-skipped image {}", satchel_core::MAX_IMAGE_BYTES))
    );
    assert!(api::list_clips(&h.ctx, Query::default()).unwrap().is_empty());
}

#[test]
fn password_manager_copies_are_skipped_whatever_their_kind() {
    let h = Harness::with_real_core();
    let mut cap = Capturer::new(h.source(), 0);
    for marker in ["org.nspasteboard.ConcealedType", "org.nspasteboard.TransientType", "com.agilebits.onepassword"] {
        h.board.external_copy(&[marker, "public.png"], Payload::Image { png: thumbnail::tests::png(4, 4) });
        assert_eq!(cap.poll(&h.ctx), Outcome::Skipped(Skip::Refused), "{marker}");
    }
    assert!(api::list_clips(&h.ctx, Query::default()).unwrap().is_empty());
}

/// Manual check of the real macOS paste path (NSPasteboard write, focus
/// restore, CGEvent Cmd+V) with the real store. Run with a text field app
/// frontmost and its pid in `SATCHEL_LIVE_PASTE_PID`:
/// `cargo test -p satchel --target-dir target/platform -- --ignored live_paste`
/// It overwrites the clipboard. Skips unless the pid is frontmost.
#[cfg(target_os = "macos")]
#[test]
#[ignore]
fn live_paste_into_frontmost_app() {
    use std::sync::Arc;

    use crate::platform::{Desktop, SystemDesktop};

    let Some(pid) = std::env::var("SATCHEL_LIVE_PASTE_PID").ok().and_then(|p| p.parse::<i32>().ok()) else {
        eprintln!("SATCHEL_LIVE_PASTE_PID not set; skipping");
        return;
    };
    let desktop = SystemDesktop::new();
    assert_eq!(desktop.frontmost_app().map(|a| a.pid), Some(pid), "target must be frontmost");
    assert!(desktop.can_post_keystrokes(), "no Accessibility permission for this process");

    let h = Harness::with_real_core();
    let id = h.seed(ClipKind::Text, "satchel live paste ok");
    // Swap the fakes for the real OS seams.
    let ctx = crate::ctx::Ctx {
        store: crate::storage::SharedStore::new(Box::new(Store::open_in_memory().unwrap())),
        settings: std::sync::RwLock::new(h.ctx.settings()),
        self_writes: Default::default(),
        events: h.ctx.events.clone(),
        desktop: Arc::new(desktop),
        clipboard: Arc::new(crate::clipboard::SystemClipboardSink),
        policy: h.ctx.policy.clone(),
        shelf: h.ctx.shelf.clone(),
        settings_window: h.ctx.settings_window.clone(),
        shortcuts: h.ctx.shortcuts.clone(),
        clock: Arc::new(crate::platform::SystemClock),
        apps: h.ctx.apps.clone(),
        app_icons: Default::default(),
        previous_app: std::sync::Mutex::new(Some(pid)),
        paste_lock: Default::default(),
        accessibility_prompted: Default::default(),
    };
    let content = h.ctx.store.lock().content(id).unwrap().unwrap();
    let new = satchel_core::NewClip {
        kind: content.kind,
        text: content.text,
        html: None,
        rtf: None,
        image_png: None,
        thumbnail_png: None,
        file_paths: vec![],
        source_app: None,
    };
    let satchel_core::InsertOutcome::Inserted(s) = ctx.store.lock().insert(new, 1).unwrap() else { panic!() };
    let started = std::time::Instant::now();
    crate::paste::paste_clips(&ctx, &[s.id], false).unwrap();
    eprintln!("paste sequence took {} ms", started.elapsed().as_millis());
}
