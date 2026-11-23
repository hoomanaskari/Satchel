//! IPC commands. `api` holds the logic (validated, tested with fakes); the
//! `#[tauri::command]` functions below are thin adapters.
//!
//! Store-backed commands run with `async` so they execute on the async
//! runtime's worker threads, never on the main (UI) thread. Paste sequences
//! sleep, so they go to the blocking pool.

use std::sync::Arc;

use tauri::State;
use tauri::ipc::Response;

use satchel_core::{ClipId, ClipKind, ClipSummary, Pinboard, PinboardId, Query, Settings};

use crate::ctx::Ctx;
use crate::error::CmdResult;
use crate::platform::AppInfo;

pub mod api {
    use super::*;
    use crate::error::{self, store_error};
    use crate::events::ChangeReason;
    use crate::{paste, shortcut, validate};

    pub fn list_clips(ctx: &Ctx, query: Query) -> CmdResult<Vec<ClipSummary>> {
        let query = validate::query(query)?;
        ctx.store.lock().list(&query).map_err(store_error)
    }

    pub fn get_clip_text(ctx: &Ctx, id: ClipId) -> CmdResult<Option<String>> {
        let id = validate::clip_id(id)?;
        let content = ctx.store.lock().content(id).map_err(store_error)?;
        let content = content.ok_or_else(|| error::NOT_FOUND.to_string())?;
        Ok(match content.kind {
            ClipKind::Image => None,
            ClipKind::File => Some(content.file_paths.join("\n")),
            _ => content.text,
        })
    }

    pub fn get_clip_thumbnail(ctx: &Ctx, id: ClipId) -> CmdResult<Vec<u8>> {
        let id = validate::clip_id(id)?;
        ctx.store
            .lock()
            .thumbnail(id)
            .map_err(store_error)?
            .ok_or_else(|| error::NOT_FOUND.to_string())
    }

    pub fn paste_clips(ctx: &Ctx, ids: &[ClipId], plain_text: bool) -> CmdResult<()> {
        let ids = validate::clip_ids(ids)?;
        paste::paste_clips(ctx, &ids, plain_text)
    }

    pub fn copy_clip(ctx: &Ctx, id: ClipId, plain_text: bool) -> CmdResult<()> {
        let id = validate::clip_id(id)?;
        paste::copy_clip(ctx, id, plain_text)
    }

    pub fn delete_clip(ctx: &Ctx, id: ClipId) -> CmdResult<bool> {
        let id = validate::clip_id(id)?;
        let deleted = ctx.store.lock().delete(id).map_err(store_error)?;
        if deleted {
            ctx.events.clips_changed(ChangeReason::Deleted, Some(id));
        }
        Ok(deleted)
    }

    pub fn set_clip_title(ctx: &Ctx, id: ClipId, title: Option<String>) -> CmdResult<()> {
        let id = validate::clip_id(id)?;
        let title = validate::title(title)?;
        ctx.store.lock().set_title(id, title.as_deref()).map_err(store_error)?;
        ctx.events.clips_changed(ChangeReason::Updated, Some(id));
        Ok(())
    }

    pub fn clear_history(ctx: &Ctx) -> CmdResult<u64> {
        let removed = ctx.store.lock().clear_history().map_err(store_error)?;
        ctx.events.clips_changed(ChangeReason::Cleared, None);
        Ok(removed)
    }

    pub fn list_pinboards(ctx: &Ctx) -> CmdResult<Vec<Pinboard>> {
        ctx.store.lock().pinboards().map_err(store_error)
    }

    pub fn create_pinboard(ctx: &Ctx, name: &str, color: &str) -> CmdResult<Pinboard> {
        let name = validate::pinboard_name(name)?;
        let color = validate::color(color)?;
        let board = ctx.store.lock().create_pinboard(&name, &color).map_err(store_error)?;
        ctx.events.pinboards_changed();
        Ok(board)
    }

    pub fn rename_pinboard(ctx: &Ctx, id: PinboardId, name: &str) -> CmdResult<()> {
        let id = validate::pinboard_id(id)?;
        let name = validate::pinboard_name(name)?;
        ctx.store.lock().rename_pinboard(id, &name).map_err(store_error)?;
        ctx.events.pinboards_changed();
        Ok(())
    }

    pub fn delete_pinboard(ctx: &Ctx, id: PinboardId) -> CmdResult<()> {
        let id = validate::pinboard_id(id)?;
        ctx.store.lock().delete_pinboard(id).map_err(store_error)?;
        ctx.events.pinboards_changed();
        Ok(())
    }

    pub fn pin_clip(ctx: &Ctx, clip_id: ClipId, pinboard_id: PinboardId) -> CmdResult<()> {
        let clip_id = validate::clip_id(clip_id)?;
        let pinboard_id = validate::pinboard_id(pinboard_id)?;
        ctx.store.lock().pin(clip_id, pinboard_id).map_err(store_error)?;
        ctx.events.pinboards_changed();
        Ok(())
    }

    pub fn unpin_clip(ctx: &Ctx, clip_id: ClipId, pinboard_id: PinboardId) -> CmdResult<()> {
        let clip_id = validate::clip_id(clip_id)?;
        let pinboard_id = validate::pinboard_id(pinboard_id)?;
        ctx.store.lock().unpin(clip_id, pinboard_id).map_err(store_error)?;
        ctx.events.pinboards_changed();
        Ok(())
    }

    pub fn get_settings(ctx: &Ctx) -> Settings {
        ctx.settings()
    }

    /// Validates, moves the global shortcut if it changed (rolling back if the
    /// store then fails), persists, applies retention and announces.
    pub fn save_settings(ctx: &Ctx, settings: Settings) -> CmdResult<()> {
        let new = validate::settings(settings)?;
        let old = ctx.settings();
        shortcut::swap(ctx.shortcuts.as_ref(), &old.shelf_shortcut, &new.shelf_shortcut)?;
        if let Err(e) = ctx.store.lock().save_settings(&new) {
            let _ = shortcut::swap(ctx.shortcuts.as_ref(), &new.shelf_shortcut, &old.shelf_shortcut);
            return Err(store_error(e));
        }
        ctx.set_settings(new.clone());
        ctx.events.settings_changed(&new);
        let removed = ctx.store.lock().prune(&new, ctx.clock.now()).unwrap_or(0);
        if removed > 0 {
            ctx.events.clips_changed(ChangeReason::Deleted, None);
        }
        Ok(())
    }

    pub fn set_capture_paused(ctx: &Ctx, paused: bool) -> CmdResult<()> {
        let mut settings = ctx.settings();
        if settings.capture_paused == paused {
            return Ok(());
        }
        settings.capture_paused = paused;
        ctx.store.lock().save_settings(&settings).map_err(store_error)?;
        ctx.set_settings(settings.clone());
        ctx.events.settings_changed(&settings);
        Ok(())
    }

    pub fn hide_shelf(ctx: &Ctx) {
        paste::dismiss_shelf(ctx);
    }

    /// Hides the shelf (without handing focus back: the settings window
    /// takes it) and opens the settings window. Focus returns to the app
    /// that was in front once the window closes.
    pub fn open_settings(ctx: &Ctx) {
        let return_to = ctx.desktop.frontmost_app().map(|a| a.pid).or_else(|| ctx.previous_app());
        ctx.shelf.hide();
        ctx.settings_window.open(return_to);
    }

    /// Hides the shelf (System Settings takes focus) and opens the page
    /// where the paste permission is granted.
    pub fn open_accessibility_settings(ctx: &Ctx) {
        ctx.shelf.hide();
        ctx.desktop.open_keystroke_permission_settings();
    }

    pub fn close_settings(ctx: &Ctx) {
        ctx.settings_window.close();
    }

    /// Candidates for the excluded-apps picker: installed apps and running
    /// regular apps, one per id, sorted by name.
    pub fn list_apps(ctx: &Ctx) -> Vec<AppInfo> {
        crate::apps::merge(ctx.apps.installed_apps(), ctx.apps.running_apps())
    }

    /// The installed app for each id, in request order (`None`: not on this
    /// machine).
    pub fn resolve_apps(ctx: &Ctx, ids: &[String]) -> CmdResult<Vec<Option<AppInfo>>> {
        let ids = validate::app_ids(ids)?;
        Ok(ids.iter().map(|id| ctx.apps.find_app(id)).collect())
    }

    /// The app's icon as PNG (`ICON_PX` square), cached.
    pub fn get_app_icon(ctx: &Ctx, id: &str) -> CmdResult<Vec<u8>> {
        let id = validate::app_id(id)?;
        ctx.app_icons
            .get_or_render(&id, || ctx.apps.icon_png(&id, crate::apps::ICON_PX))
            .map(|png| png.as_ref().clone())
            .ok_or_else(|| error::NOT_FOUND.to_string())
    }

    /// The native "choose an application" dialog; `None` when cancelled.
    pub fn choose_app(ctx: &Ctx) -> Option<AppInfo> {
        ctx.apps.choose_app()
    }
}

type Ctxs<'a> = State<'a, Arc<Ctx>>;

async fn blocking<T: Send + 'static>(
    ctx: &Arc<Ctx>,
    f: impl FnOnce(&Ctx) -> CmdResult<T> + Send + 'static,
) -> CmdResult<T> {
    let ctx = ctx.clone();
    tauri::async_runtime::spawn_blocking(move || f(&ctx))
        .await
        .map_err(|_| crate::error::INTERNAL.to_string())?
}

#[tauri::command(async)]
pub fn list_clips(ctx: Ctxs<'_>, query: Query) -> CmdResult<Vec<ClipSummary>> {
    api::list_clips(&ctx, query)
}

#[tauri::command(async)]
pub fn get_clip_text(ctx: Ctxs<'_>, id: ClipId) -> CmdResult<Option<String>> {
    api::get_clip_text(&ctx, id)
}

/// Raw PNG bytes (an `ArrayBuffer` on the JS side), no JSON/base64 detour.
#[tauri::command(async)]
pub fn get_clip_thumbnail(ctx: Ctxs<'_>, id: ClipId) -> CmdResult<Response> {
    api::get_clip_thumbnail(&ctx, id).map(Response::new)
}

#[tauri::command]
pub async fn paste_clip(ctx: Ctxs<'_>, id: ClipId, plain_text: bool) -> CmdResult<()> {
    blocking(&ctx, move |c| api::paste_clips(c, &[id], plain_text)).await
}

#[tauri::command]
pub async fn paste_clips(ctx: Ctxs<'_>, ids: Vec<ClipId>, plain_text: bool) -> CmdResult<()> {
    blocking(&ctx, move |c| api::paste_clips(c, &ids, plain_text)).await
}

#[tauri::command(async)]
pub fn copy_clip(ctx: Ctxs<'_>, id: ClipId, plain_text: bool) -> CmdResult<()> {
    api::copy_clip(&ctx, id, plain_text)
}

#[tauri::command(async)]
pub fn delete_clip(ctx: Ctxs<'_>, id: ClipId) -> CmdResult<bool> {
    api::delete_clip(&ctx, id)
}

#[tauri::command(async)]
pub fn set_clip_title(ctx: Ctxs<'_>, id: ClipId, title: Option<String>) -> CmdResult<()> {
    api::set_clip_title(&ctx, id, title)
}

#[tauri::command(async)]
pub fn clear_history(ctx: Ctxs<'_>) -> CmdResult<u64> {
    api::clear_history(&ctx)
}

#[tauri::command(async)]
pub fn list_pinboards(ctx: Ctxs<'_>) -> CmdResult<Vec<Pinboard>> {
    api::list_pinboards(&ctx)
}

#[tauri::command(async)]
pub fn create_pinboard(ctx: Ctxs<'_>, name: String, color: String) -> CmdResult<Pinboard> {
    api::create_pinboard(&ctx, &name, &color)
}

#[tauri::command(async)]
pub fn rename_pinboard(ctx: Ctxs<'_>, id: PinboardId, name: String) -> CmdResult<()> {
    api::rename_pinboard(&ctx, id, &name)
}

#[tauri::command(async)]
pub fn delete_pinboard(ctx: Ctxs<'_>, id: PinboardId) -> CmdResult<()> {
    api::delete_pinboard(&ctx, id)
}

#[tauri::command(async)]
pub fn pin_clip(ctx: Ctxs<'_>, clip_id: ClipId, pinboard_id: PinboardId) -> CmdResult<()> {
    api::pin_clip(&ctx, clip_id, pinboard_id)
}

#[tauri::command(async)]
pub fn unpin_clip(ctx: Ctxs<'_>, clip_id: ClipId, pinboard_id: PinboardId) -> CmdResult<()> {
    api::unpin_clip(&ctx, clip_id, pinboard_id)
}

#[tauri::command(async)]
pub fn get_settings(ctx: Ctxs<'_>) -> Settings {
    api::get_settings(&ctx)
}

#[tauri::command(async)]
pub fn save_settings(ctx: Ctxs<'_>, settings: Settings) -> CmdResult<()> {
    api::save_settings(&ctx, settings)
}

#[tauri::command(async)]
pub fn set_capture_paused(ctx: Ctxs<'_>, paused: bool) -> CmdResult<()> {
    api::set_capture_paused(&ctx, paused)
}

#[tauri::command]
pub async fn hide_shelf(ctx: Ctxs<'_>) -> CmdResult<()> {
    blocking(&ctx, |c| {
        api::hide_shelf(c);
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn open_settings(ctx: Ctxs<'_>) -> CmdResult<()> {
    blocking(&ctx, |c| {
        api::open_settings(c);
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn open_accessibility_settings(ctx: Ctxs<'_>) -> CmdResult<()> {
    blocking(&ctx, |c| {
        api::open_accessibility_settings(c);
        Ok(())
    })
    .await
}

#[tauri::command(async)]
pub fn close_settings(ctx: Ctxs<'_>) {
    api::close_settings(&ctx);
}

#[tauri::command]
pub async fn list_apps(ctx: Ctxs<'_>) -> CmdResult<Vec<AppInfo>> {
    blocking(&ctx, |c| Ok(api::list_apps(c))).await
}

#[tauri::command]
pub async fn resolve_apps(ctx: Ctxs<'_>, ids: Vec<String>) -> CmdResult<Vec<Option<AppInfo>>> {
    blocking(&ctx, move |c| api::resolve_apps(c, &ids)).await
}

/// Raw PNG bytes, like `get_clip_thumbnail`.
#[tauri::command]
pub async fn get_app_icon(ctx: Ctxs<'_>, id: String) -> CmdResult<Response> {
    blocking(&ctx, move |c| api::get_app_icon(c, &id)).await.map(Response::new)
}

/// Waits (off the main thread) for the open panel, which runs on it.
#[tauri::command]
pub async fn choose_app(ctx: Ctxs<'_>) -> CmdResult<Option<AppInfo>> {
    blocking(&ctx, |c| Ok(api::choose_app(c))).await
}

#[cfg(test)]
mod tests {
    use super::api;
    use super::*;
    use crate::error;
    use crate::testing::{FakeStore, Harness, sample_settings};

    #[test]
    fn list_validates_before_touching_the_store() {
        let h = Harness::new();
        h.seed(ClipKind::Text, "alpha");
        h.seed(ClipKind::Text, "beta");
        assert_eq!(api::list_clips(&h.ctx, Query::default()).unwrap().len(), 2);
        let q = Query { text: Some(" alp ".into()), ..Query::default() };
        assert_eq!(api::list_clips(&h.ctx, q).unwrap().len(), 1);
        assert!(api::list_clips(&h.ctx, Query { limit: 5000, ..Query::default() }).is_err());
    }

    #[test]
    fn get_clip_text_by_kind() {
        let h = Harness::new();
        let id = h.seed(ClipKind::Code, "fn main() {}");
        assert_eq!(api::get_clip_text(&h.ctx, id).unwrap().as_deref(), Some("fn main() {}"));
        assert_eq!(api::get_clip_text(&h.ctx, 999), Err(error::NOT_FOUND.into()));
        assert!(api::get_clip_text(&h.ctx, -1).is_err());
    }

    #[test]
    fn thumbnail_missing_is_not_found() {
        let h = Harness::new();
        let id = h.seed(ClipKind::Text, "no image");
        assert_eq!(api::get_clip_thumbnail(&h.ctx, id), Err(error::NOT_FOUND.into()));
    }

    #[test]
    fn delete_announces_only_real_deletions() {
        let h = Harness::new();
        let id = h.seed(ClipKind::Text, "x");
        h.log.clear();
        assert_eq!(api::delete_clip(&h.ctx, id), Ok(true));
        assert_eq!(api::delete_clip(&h.ctx, id), Ok(false));
        assert_eq!(h.log.entries(), vec![format!("event clips-changed deleted {id}")]);
    }

    #[test]
    fn titles_are_validated_and_announced() {
        let h = Harness::new();
        let id = h.seed(ClipKind::Text, "x");
        h.log.clear();
        api::set_clip_title(&h.ctx, id, Some("  Name ".into())).unwrap();
        assert_eq!(h.log.entries(), vec![format!("event clips-changed updated {id}")]);
        assert!(api::set_clip_title(&h.ctx, id, Some("x".repeat(500))).is_err());
        assert!(api::set_clip_title(&h.ctx, 999, None).is_err());
    }

    #[test]
    fn clear_history_counts_and_announces() {
        let h = Harness::new();
        h.seed(ClipKind::Text, "a");
        h.seed(ClipKind::Text, "b");
        h.log.clear();
        assert_eq!(api::clear_history(&h.ctx), Ok(2));
        assert_eq!(h.log.entries(), vec!["event clips-changed cleared"]);
    }

    #[test]
    fn pinboard_lifecycle() {
        let h = Harness::new();
        let clip = h.seed(ClipKind::Text, "a");
        h.log.clear();
        let board = api::create_pinboard(&h.ctx, " Work ", "#ff9f0a").unwrap();
        assert_eq!(board.name, "Work");
        assert_eq!(board.color, "#FF9F0A");
        api::rename_pinboard(&h.ctx, board.id, "Home").unwrap();
        api::pin_clip(&h.ctx, clip, board.id).unwrap();
        api::unpin_clip(&h.ctx, clip, board.id).unwrap();
        api::delete_pinboard(&h.ctx, board.id).unwrap();
        assert_eq!(h.log.entries().len(), 5);
        assert!(h.log.entries().iter().all(|e| e == "event pinboards-changed"));
        assert!(api::list_pinboards(&h.ctx).unwrap().is_empty());
    }

    #[test]
    fn pinboard_input_is_validated() {
        let h = Harness::new();
        assert!(api::create_pinboard(&h.ctx, "", "#FFFFFF").is_err());
        assert!(api::create_pinboard(&h.ctx, "ok", "red").is_err());
        assert!(api::rename_pinboard(&h.ctx, 0, "x").is_err());
        assert!(api::pin_clip(&h.ctx, 1, -2).is_err());
        assert!(h.log.entries().is_empty());
    }

    #[test]
    fn store_errors_are_content_free() {
        let h = Harness::with_store(FakeStore::failing());
        let err = api::list_clips(&h.ctx, Query::default()).unwrap_err();
        assert_eq!(err, error::INVALID_INPUT);
        assert!(!err.contains("secret"));
    }

    #[test]
    fn save_settings_validates_swaps_shortcut_persists_and_announces() {
        let h = Harness::new();
        let new = Settings { shelf_shortcut: "Alt+Space".into(), max_items: 50, ..sample_settings() };
        api::save_settings(&h.ctx, new.clone()).unwrap();
        assert_eq!(h.ctx.settings(), new);
        assert_eq!(h.ctx.store.lock().settings().unwrap(), new);
        assert_eq!(
            h.log.entries(),
            vec![
                "register Alt+Space",
                "unregister CommandOrControl+Shift+V",
                "event settings-changed paused=false",
            ]
        );
    }

    #[test]
    fn save_settings_with_taken_shortcut_changes_nothing() {
        let h = Harness::new();
        *h.shortcuts.refuse.lock().unwrap() = Some("Alt+Space".into());
        let new = Settings { shelf_shortcut: "Alt+Space".into(), ..sample_settings() };
        assert!(api::save_settings(&h.ctx, new).is_err());
        assert_eq!(h.ctx.settings(), sample_settings());
    }

    #[test]
    fn save_settings_rejects_invalid_values() {
        let h = Harness::new();
        assert!(api::save_settings(&h.ctx, Settings { max_items: 1, ..sample_settings() }).is_err());
        assert!(h.log.entries().is_empty());
    }

    #[test]
    fn save_settings_rolls_back_shortcut_when_store_fails() {
        let h = Harness::with_store(FakeStore::failing());
        let new = Settings { shelf_shortcut: "Alt+Space".into(), ..sample_settings() };
        assert!(api::save_settings(&h.ctx, new).is_err());
        assert_eq!(
            h.log.entries(),
            vec![
                "register Alt+Space",
                "unregister CommandOrControl+Shift+V",
                "register CommandOrControl+Shift+V",
                "unregister Alt+Space",
            ]
        );
        assert_eq!(h.ctx.settings(), sample_settings());
    }

    #[test]
    fn save_settings_prunes_to_the_new_limit() {
        let h = Harness::new();
        for i in 0..12 {
            h.seed(ClipKind::Text, &format!("clip {i}"));
        }
        api::save_settings(&h.ctx, Settings { max_items: 10, ..sample_settings() }).unwrap();
        assert_eq!(api::list_clips(&h.ctx, Query::default()).unwrap().len(), 10);
        assert_eq!(h.log.entries().last().unwrap(), "event clips-changed deleted");
    }

    #[test]
    fn pause_toggles_persist_and_announce_once() {
        let h = Harness::new();
        api::set_capture_paused(&h.ctx, true).unwrap();
        api::set_capture_paused(&h.ctx, true).unwrap();
        assert!(h.ctx.capture_paused());
        assert!(h.ctx.store.lock().settings().unwrap().capture_paused);
        assert_eq!(h.log.entries(), vec!["event settings-changed paused=true"]);
    }

    #[test]
    fn open_settings_hides_the_shelf_then_opens_the_window() {
        let h = Harness::new();
        api::open_settings(&h.ctx);
        // The previous app keeps focus until the window takes it: no
        // "activate" in between, so it cannot flash to the front.
        assert_eq!(h.log.entries(), vec!["hide shelf", "open settings (return to 42)"]);
    }

    #[test]
    fn open_accessibility_settings_hides_the_shelf_then_opens_the_page() {
        let h = Harness::new();
        api::open_accessibility_settings(&h.ctx);
        assert_eq!(h.log.entries(), vec!["hide shelf", "open accessibility settings"]);
    }

    #[test]
    fn open_settings_falls_back_to_the_app_the_shelf_remembered() {
        // Satchel itself is in front (e.g. the shelf as a regular window).
        let h = Harness::new();
        *h.desktop.front.lock().unwrap() = None;
        h.ctx.remember_previous_app(Some(7));
        api::open_settings(&h.ctx);
        assert_eq!(h.log.entries(), vec!["hide shelf", "open settings (return to 7)"]);
    }

    #[test]
    fn open_settings_without_any_known_app() {
        let h = Harness::new();
        *h.desktop.front.lock().unwrap() = None;
        api::open_settings(&h.ctx);
        assert_eq!(h.log.entries(), vec!["hide shelf", "open settings"]);
    }

    #[test]
    fn close_settings_closes_the_window_only() {
        let h = Harness::new();
        api::close_settings(&h.ctx);
        assert_eq!(h.log.entries(), vec!["close settings"]);
    }

    #[test]
    fn list_apps_merges_installed_and_running_apps() {
        let h = Harness::new();
        let apps = api::list_apps(&h.ctx);
        let summary: Vec<_> = apps.iter().map(|a| (a.name.as_str(), a.running)).collect();
        assert_eq!(summary, vec![("Notes", true), ("TextEdit", false), ("Vault", false)]);
    }

    #[test]
    fn resolve_apps_answers_in_request_order() {
        let h = Harness::new();
        let ids = vec!["com.example.vault".to_string(), "1Password.exe".into(), "com.apple.Notes".into()];
        let resolved = api::resolve_apps(&h.ctx, &ids).unwrap();
        let names: Vec<_> = resolved.iter().map(|a| a.as_ref().map(|a| a.name.as_str())).collect();
        assert_eq!(names, vec![Some("Vault"), None, Some("Notes")]);
        assert!(api::resolve_apps(&h.ctx, &["bad\u{7}".into()]).is_err());
    }

    #[test]
    fn app_icons_are_cached_and_missing_ones_are_not_found() {
        let h = Harness::new();
        assert_eq!(api::get_app_icon(&h.ctx, " com.apple.TextEdit ").unwrap(), b"png:com.apple.TextEdit".to_vec());
        assert_eq!(api::get_app_icon(&h.ctx, "com.apple.textedit").unwrap(), b"png:com.apple.TextEdit".to_vec());
        assert_eq!(api::get_app_icon(&h.ctx, "1Password.exe").unwrap_err(), error::NOT_FOUND);
        assert!(api::get_app_icon(&h.ctx, "").is_err());
        let renders: Vec<_> = h.log.entries().into_iter().filter(|e| e.starts_with("icon ")).collect();
        assert_eq!(renders, vec!["icon com.apple.TextEdit 64", "icon 1Password.exe 64"]);
    }

    #[test]
    fn choose_app_passes_the_dialog_result_through() {
        let h = Harness::new();
        assert_eq!(api::choose_app(&h.ctx).map(|a| a.id), Some("com.example.chosen".into()));
    }

    #[test]
    fn paste_validates_ids() {
        let h = Harness::new();
        assert!(api::paste_clips(&h.ctx, &[], false).is_err());
        assert!(api::paste_clips(&h.ctx, &[0], false).is_err());
        assert!(api::copy_clip(&h.ctx, -3, false).is_err());
        assert!(h.log.entries().is_empty());
    }
}
