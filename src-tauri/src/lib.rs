//! Satchel's Tauri application layer: clipboard watcher, paste, global
//! shortcut, tray, the shelf and settings windows and the IPC commands.

mod apps;
mod capture;
mod clipboard;
mod commands;
mod ctx;
mod error;
mod events;
mod paste;
mod platform;
mod settings_window;
mod shelf;
mod shortcut;
mod storage;
mod thumbnail;
mod tray;
mod validate;
mod watcher;

#[cfg(test)]
mod integration_tests;
#[cfg(test)]
mod testing;

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, RwLock};

use tauri::{AppHandle, Emitter, Manager, Runtime};
use tauri_plugin_global_shortcut::ShortcutState;

use satchel_core::{ClipId, ClipKind, Settings, Store};

use crate::clipboard::{SelfWrites, SystemClipboardSink};
use crate::ctx::{CorePolicy, Ctx};
use crate::events::{CaptureSkipped, ChangeReason, ClipsChanged, EventSink};
use crate::platform::{SystemApps, SystemClock, SystemDesktop};
use crate::settings_window::TauriSettingsWindow;
use crate::shelf::{Shelf, TauriShelf};
use crate::shortcut::{ShortcutRegistrar, TauriShortcuts};
use crate::storage::{ClipStore, SharedStore};

const DB_FILE: &str = "satchel.sqlite3";

/// Emits IPC events and keeps the tray's pause check mark in sync.
struct TauriEvents<R: Runtime> {
    app: AppHandle<R>,
}

impl<R: Runtime> EventSink for TauriEvents<R> {
    fn clips_changed(&self, reason: ChangeReason, id: Option<ClipId>) {
        let _ = self.app.emit(events::CLIPS_CHANGED, ClipsChanged { reason, id });
    }
    fn pinboards_changed(&self) {
        let _ = self.app.emit(events::PINBOARDS_CHANGED, ());
    }
    fn settings_changed(&self, settings: &Settings) {
        let _ = self.app.emit(events::SETTINGS_CHANGED, settings);
        if let Some(item) = self.app.try_state::<tray::PauseItem<R>>() {
            let _ = item.0.set_checked(settings.capture_paused);
        }
    }
    fn capture_skipped(&self, kind: ClipKind, limit_bytes: u64) {
        let _ = self.app.emit(events::CAPTURE_SKIPPED, CaptureSkipped { kind, limit_bytes });
    }
}

/// Creates the data directory readable by the current user only.
fn ensure_private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn data_dir<R: Runtime>(app: &AppHandle<R>) -> Option<std::path::PathBuf> {
    // Debug builds only: lets manual testing use a throwaway history.
    #[cfg(debug_assertions)]
    if let Some(dir) = std::env::var_os("SATCHEL_DATA_DIR") {
        return Some(dir.into());
    }
    app.path().app_data_dir().ok()
}

fn open_store<R: Runtime>(app: &AppHandle<R>) -> Box<dyn ClipStore> {
    let opened = data_dir(app)
        .ok_or(())
        .and_then(|dir| ensure_private_dir(&dir).map(|_| dir).map_err(|_| ()))
        .and_then(|dir| Store::open(&dir.join(DB_FILE)).map_err(|_| ()));
    match opened {
        Ok(store) => Box::new(store),
        Err(()) => {
            // Never fall back to a world-readable location: keep the history
            // in memory for this run instead.
            eprintln!("satchel: cannot open the history database; history is kept in memory only");
            Box::new(Store::open_in_memory().expect("in-memory SQLite is always available"))
        }
    }
}

fn setup<R: Runtime>(app: &AppHandle<R>) -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(target_os = "macos")]
    app.set_activation_policy(tauri::ActivationPolicy::Accessory)?;

    let store = open_store(app);
    let settings = store.settings().unwrap_or_default();
    let shortcuts: Arc<dyn ShortcutRegistrar> = Arc::new(TauriShortcuts { app: app.clone() });
    let main_app = app.clone();
    let apps = SystemApps::new(Box::new(move |f| main_app.run_on_main_thread(f).is_ok()));
    let ctx = Arc::new(Ctx {
        store: SharedStore::new(store),
        settings: RwLock::new(settings.clone()),
        self_writes: SelfWrites::default(),
        events: Arc::new(TauriEvents { app: app.clone() }),
        desktop: Arc::new(SystemDesktop::new()),
        clipboard: Arc::new(SystemClipboardSink),
        policy: Arc::new(CorePolicy),
        shelf: Arc::new(TauriShelf { app: app.clone() }),
        settings_window: Arc::new(TauriSettingsWindow::new(app.clone())),
        shortcuts: shortcuts.clone(),
        clock: Arc::new(SystemClock),
        apps: Arc::new(apps),
        app_icons: Default::default(),
        previous_app: Mutex::new(None),
        paste_lock: Mutex::new(()),
        accessibility_prompted: AtomicBool::new(false),
    });
    app.manage(ctx.clone());

    let shelf = Shelf::create(app)?;
    if !shelf.is_panel() {
        eprintln!("satchel: shelf uses a regular window (non-activating panel unavailable)");
    }
    app.manage(shelf);
    Shelf::install_blur_handler(app);

    tray::create(app, settings.capture_paused)?;

    if shortcuts.register(&settings.shelf_shortcut).is_err() {
        let fallback = Settings::default().shelf_shortcut;
        eprintln!("satchel: the shelf shortcut is unavailable; trying the default");
        let _ = shortcuts.register(&fallback);
    }

    watcher::spawn(ctx)?;
    Ok(())
}

fn on_shortcut<R: Runtime>(app: &AppHandle<R>) {
    let (Some(shelf), Some(ctx)) = (app.try_state::<Shelf<R>>(), app.try_state::<Arc<Ctx>>()) else {
        return;
    };
    shelf.toggle(app, &ctx);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    if event.state == ShortcutState::Pressed {
                        on_shortcut(app);
                    }
                })
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            commands::list_clips,
            commands::get_clip_text,
            commands::get_clip_thumbnail,
            commands::paste_clip,
            commands::paste_clips,
            commands::open_accessibility_settings,
            commands::copy_clip,
            commands::delete_clip,
            commands::set_clip_title,
            commands::clear_history,
            commands::list_pinboards,
            commands::create_pinboard,
            commands::rename_pinboard,
            commands::delete_pinboard,
            commands::pin_clip,
            commands::unpin_clip,
            commands::get_settings,
            commands::save_settings,
            commands::set_capture_paused,
            commands::hide_shelf,
            commands::open_settings,
            commands::close_settings,
            commands::list_apps,
            commands::resolve_apps,
            commands::get_app_icon,
            commands::choose_app,
        ])
        .setup(|app| setup(app.handle()))
        .build(tauri::generate_context!())
        .expect("error while building Satchel")
        .run(|_app, event| {
            // An accessory app with no visible window must keep running.
            if let tauri::RunEvent::ExitRequested { code: None, api, .. } = event {
                api.prevent_exit();
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_dir_is_private() {
        let dir = std::env::temp_dir().join(format!("satchel-test-{}", std::process::id()));
        let nested = dir.join("a/b");
        ensure_private_dir(&nested).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&nested).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
