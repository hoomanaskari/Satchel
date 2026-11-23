//! Copy and paste sequencing: write the clipboard (recording the write so
//! the watcher ignores it), hide the shelf, give focus back to the app that
//! was in front, synthesize the paste keystroke, bump the clip.

use std::time::Duration;

use satchel_core::ClipId;

use crate::clipboard::{WriteRequest, write_request};
use crate::ctx::Ctx;
use crate::error::{self, CmdResult};
use crate::events::ChangeReason;

/// How long to wait for the previous app to come back to the front.
pub const FOCUS_TIMEOUT: Duration = Duration::from_millis(500);
pub const FOCUS_POLL: Duration = Duration::from_millis(10);
/// Pause between focus returning and the keystroke, so the target's key
/// window is ready to receive it.
pub const SETTLE: Duration = Duration::from_millis(40);
/// Pause between two pastes of a multi-paste, so the target has read the
/// clipboard before it is overwritten.
pub const BETWEEN_PASTES: Duration = Duration::from_millis(150);

fn load(ctx: &Ctx, ids: &[ClipId], plain: bool) -> CmdResult<Vec<(ClipId, WriteRequest)>> {
    let store = ctx.store.lock();
    ids.iter()
        .map(|&id| {
            let content = store
                .content(id)
                .map_err(error::store_error)?
                .ok_or_else(|| error::NOT_FOUND.to_string())?;
            Ok((id, write_request(&content, plain)?))
        })
        .collect()
}

fn write(ctx: &Ctx, request: &WriteRequest) -> CmdResult<()> {
    ctx.self_writes
        .write_with(|| ctx.clipboard.write(request))
        .map(|_| ())
        .map_err(|_| error::CLIPBOARD.to_string())
}

fn bump(ctx: &Ctx, id: ClipId) {
    let now = ctx.clock.now();
    if ctx.store.lock().touch(id, now).is_ok() {
        ctx.events.clips_changed(ChangeReason::Bumped, Some(id));
    }
}

/// What a copy of several clips puts on the clipboard: text clips joined by
/// newlines, otherwise the first clip.
pub fn combine_for_copy(requests: &[(ClipId, WriteRequest)]) -> Option<WriteRequest> {
    let all_text = requests
        .iter()
        .all(|(_, r)| r.text.is_some() && r.png.is_none() && r.files.is_empty());
    if requests.len() > 1 && all_text {
        let text = requests
            .iter()
            .filter_map(|(_, r)| r.text.as_deref())
            .collect::<Vec<_>>()
            .join("\n");
        return Some(WriteRequest { text: Some(text), ..WriteRequest::default() });
    }
    requests.first().map(|(_, r)| r.clone())
}

/// Copies one clip without pasting.
pub fn copy_clip(ctx: &Ctx, id: ClipId, plain: bool) -> CmdResult<()> {
    let requests = load(ctx, &[id], plain)?;
    write(ctx, &requests[0].1)?;
    bump(ctx, id);
    Ok(())
}

/// Brings `pid` back to the front, waiting until it is (or giving up).
pub fn restore_focus(ctx: &Ctx, pid: i32) -> bool {
    let is_front = || ctx.desktop.frontmost_app().is_some_and(|a| a.pid == pid);
    if is_front() {
        return true;
    }
    if !ctx.desktop.activate_app(pid) {
        return false;
    }
    let mut waited = Duration::ZERO;
    while waited < FOCUS_TIMEOUT {
        if is_front() {
            return true;
        }
        ctx.clock.sleep(FOCUS_POLL);
        waited += FOCUS_POLL;
    }
    is_front()
}

/// Hides the shelf and hands focus back to the previous app (Escape).
pub fn dismiss_shelf(ctx: &Ctx) {
    ctx.shelf.hide();
    if let Some(pid) = ctx.previous_app()
        && !ctx.desktop.frontmost_app().is_some_and(|a| a.pid == pid)
    {
        ctx.desktop.activate_app(pid);
    }
}

/// Pastes the clips, in order, into the app that was in front when the shelf
/// opened. Without the keystroke permission it copies instead and returns
/// the `ACCESSIBILITY` error so the UI can explain.
pub fn paste_clips(ctx: &Ctx, ids: &[ClipId], plain: bool) -> CmdResult<()> {
    let _one_at_a_time = ctx.paste_lock.lock().unwrap_or_else(|p| p.into_inner());
    // Everything is loaded before anything visible happens, so a bad id
    // leaves the shelf and the clipboard untouched.
    let requests = load(ctx, ids, plain)?;
    let target = ctx.previous_app();
    ctx.shelf.hide();

    let allowed = ctx.desktop.can_post_keystrokes();
    if crate::shelf::tracing() {
        let front = ctx.desktop.frontmost_app().map(|a| (a.pid, a.name));
        eprintln!("satchel: paste: keystrokes allowed={allowed} target={target:?} frontmost={front:?}");
    }
    if !allowed {
        if let Some(request) = combine_for_copy(&requests) {
            write(ctx, &request)?;
            for (id, _) in &requests {
                bump(ctx, *id);
            }
        }
        if let Some(pid) = target {
            restore_focus(ctx, pid);
        }
        if ctx.take_accessibility_prompt() {
            ctx.desktop.request_keystroke_permission();
        }
        return Err(error::ACCESSIBILITY.to_string());
    }

    if let Some(pid) = target {
        let restored = restore_focus(ctx, pid);
        if crate::shelf::tracing() {
            eprintln!("satchel: paste: focus restored={restored}");
        }
    }
    for (index, (id, request)) in requests.iter().enumerate() {
        if index > 0 {
            ctx.clock.sleep(BETWEEN_PASTES);
        }
        write(ctx, request)?;
        ctx.clock.sleep(SETTLE);
        ctx.desktop
            .post_paste_keystroke()
            .map_err(|_| "could not send the paste keystroke".to_string())?;
        bump(ctx, *id);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use satchel_core::ClipKind;

    use super::*;
    use crate::platform::FrontApp;
    use crate::testing::Harness;

    fn set_front(h: &Harness, pid: i32) {
        *h.desktop.front.lock().unwrap() =
            Some(FrontApp { pid, id: format!("app.{pid}"), name: format!("App {pid}") });
    }

    #[test]
    fn paste_writes_hides_restores_focus_then_keystrokes_and_bumps() {
        let h = Harness::new();
        let id = h.seed(ClipKind::Text, "hello");
        h.ctx.remember_previous_app(Some(7));
        set_front(&h, 99);
        h.log.clear();

        paste_clips(&h.ctx, &[id], false).unwrap();
        assert_eq!(
            h.log.entries(),
            vec![
                "hide shelf".to_string(),
                "activate 7".to_string(),
                "write text:hello".to_string(),
                format!("sleep {}", SETTLE.as_millis()),
                "keystroke paste".to_string(),
                format!("event clips-changed bumped {id}"),
            ]
        );
    }

    #[test]
    fn no_activation_when_target_is_already_frontmost() {
        // The non-activating panel case: the previous app never lost focus.
        let h = Harness::new();
        let id = h.seed(ClipKind::Text, "x");
        h.ctx.remember_previous_app(Some(42));
        h.log.clear();
        paste_clips(&h.ctx, &[id], false).unwrap();
        assert!(!h.log.entries().iter().any(|e| e.starts_with("activate")));
    }

    #[test]
    fn waits_for_slow_activation_but_gives_up_after_timeout() {
        let h = Harness::new();
        let id = h.seed(ClipKind::Text, "x");
        h.desktop.activation_works.store(false, Ordering::SeqCst);
        h.ctx.remember_previous_app(Some(7));
        h.log.clear();
        paste_clips(&h.ctx, &[id], false).unwrap();
        let polls = h.log.entries().iter().filter(|e| *e == "sleep 10").count();
        assert_eq!(polls as u128, FOCUS_TIMEOUT.as_millis() / FOCUS_POLL.as_millis());
        assert!(h.log.entries().contains(&"keystroke paste".to_string()));
    }

    #[test]
    fn focus_arriving_late_stops_the_wait() {
        let h = Harness::new();
        h.desktop.activation_works.store(false, Ordering::SeqCst);
        set_front(&h, 99);
        let desktop = h.desktop.clone();
        h.clock.on_sleep.lock().unwrap().extend([
            Box::new(|| {}) as Box<dyn FnOnce() + Send>,
            Box::new(move || {
                *desktop.front.lock().unwrap() =
                    Some(FrontApp { pid: 7, id: "a".into(), name: "A".into() });
            }),
        ]);
        assert!(restore_focus(&h.ctx, 7));
        assert_eq!(h.log.entries().iter().filter(|e| *e == "sleep 10").count(), 2);
    }

    #[test]
    fn multiple_clips_paste_in_order_with_pauses() {
        let h = Harness::new();
        let a = h.seed(ClipKind::Text, "first");
        let b = h.seed(ClipKind::Text, "second");
        h.log.clear();
        paste_clips(&h.ctx, &[b, a], true).unwrap();
        let entries: Vec<String> = h
            .log
            .entries()
            .into_iter()
            .filter(|e| e.starts_with("write") || e.starts_with("keystroke") || e.starts_with("sleep 150"))
            .collect();
        assert_eq!(
            entries,
            vec!["write text:second", "keystroke paste", "sleep 150", "write text:first", "keystroke paste"]
        );
    }

    #[test]
    fn plain_paste_drops_html() {
        let h = Harness::new();
        let id = h.seed(ClipKind::Text, "t");
        assert!(paste_clips(&h.ctx, &[id], true).is_ok());
    }

    #[test]
    fn unknown_id_fails_before_anything_visible_happens() {
        let h = Harness::new();
        let id = h.seed(ClipKind::Text, "x");
        h.log.clear();
        assert_eq!(paste_clips(&h.ctx, &[id, 999], false), Err(error::NOT_FOUND.to_string()));
        assert!(h.log.entries().is_empty());
    }

    #[test]
    fn without_accessibility_it_copies_explains_and_prompts_once() {
        let h = Harness::new();
        let a = h.seed(ClipKind::Text, "one");
        let b = h.seed(ClipKind::Text, "two");
        h.desktop.trusted.store(false, Ordering::SeqCst);
        h.log.clear();

        let err = paste_clips(&h.ctx, &[a, b], false).unwrap_err();
        assert!(err.starts_with("accessibility-permission-required"));
        let entries = h.log.entries();
        assert!(entries.contains(&"write text:one\ntwo".to_string()));
        assert!(!entries.contains(&"keystroke paste".to_string()));
        assert_eq!(entries.iter().filter(|e| *e == "prompt accessibility").count(), 1);

        h.log.clear();
        assert!(paste_clips(&h.ctx, &[a], false).is_err());
        assert!(!h.log.entries().contains(&"prompt accessibility".to_string()));
    }

    #[test]
    fn errors_never_contain_clip_text() {
        let h = Harness::new();
        let id = h.seed(ClipKind::Text, "top secret");
        h.board.fail_writes.store(true, Ordering::SeqCst);
        let err = paste_clips(&h.ctx, &[id], false).unwrap_err();
        assert!(!err.contains("secret"));
        assert_eq!(err, error::CLIPBOARD);
    }

    #[test]
    fn own_paste_is_not_recaptured() {
        use crate::capture::{Capturer, Outcome, Skip};
        let h = Harness::new();
        let id = h.seed(ClipKind::Text, "x");
        let mut cap = Capturer::new(h.source(), 0);
        paste_clips(&h.ctx, &[id], false).unwrap();
        assert_eq!(cap.poll(&h.ctx), Outcome::Skipped(Skip::OwnWrite));
    }

    #[test]
    fn copy_writes_and_bumps_without_hiding_or_keystroke() {
        let h = Harness::new();
        let id = h.seed(ClipKind::Link, "https://a.b");
        h.log.clear();
        copy_clip(&h.ctx, id, false).unwrap();
        assert_eq!(
            h.log.entries(),
            vec!["write text:https://a.b".to_string(), format!("event clips-changed bumped {id}")]
        );
    }

    #[test]
    fn dismiss_restores_focus_only_when_needed() {
        let h = Harness::new();
        h.ctx.remember_previous_app(Some(42));
        dismiss_shelf(&h.ctx);
        assert_eq!(h.log.entries(), vec!["hide shelf"]);
        h.log.clear();
        h.ctx.remember_previous_app(Some(7));
        dismiss_shelf(&h.ctx);
        assert_eq!(h.log.entries(), vec!["hide shelf", "activate 7"]);
    }

    #[test]
    fn combine_for_copy_rules() {
        let t = |s: &str| WriteRequest { text: Some(s.into()), ..WriteRequest::default() };
        let img = WriteRequest { png: Some(vec![1]), ..WriteRequest::default() };
        assert_eq!(combine_for_copy(&[]), None);
        assert_eq!(combine_for_copy(&[(1, t("a"))]), Some(t("a")));
        assert_eq!(combine_for_copy(&[(1, t("a")), (2, t("b"))]), Some(t("a\nb")));
        assert_eq!(combine_for_copy(&[(1, img.clone()), (2, t("b"))]), Some(img));
    }
}
