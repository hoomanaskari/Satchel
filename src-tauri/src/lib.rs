//! Satchel's Tauri application layer: clipboard watcher, paste, global
//! shortcut, tray, the shelf and settings windows and the IPC commands.

mod app_identity;
mod apps;
mod capture;
mod clipboard;
mod commands;
mod ctx;
mod error;
mod events;
mod instance;
mod link_preview;
mod login_item;
mod notices;
mod paste;
mod platform;
mod settings_window;
mod shelf;
mod shortcut;
mod storage;
mod thumbnail;
mod tray;
mod tray_anim;
mod updates;
mod validate;
mod watcher;
mod window_frame;

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
        if let Some(updates) = self.app.try_state::<Arc<updates::Updates>>() {
            updates.set_auto(settings.check_for_updates);
        }
    }
    fn capture_skipped(&self, kind: ClipKind, limit_bytes: u64) {
        let _ = self.app.emit(events::CAPTURE_SKIPPED, CaptureSkipped { kind, limit_bytes });
    }
    fn clip_saved(&self) {
        tray::clip_saved(&self.app);
    }
}

/// Sends update status changes to every window and to the tray.
struct TauriUpdateEvents<R: Runtime> {
    app: AppHandle<R>,
}

impl<R: Runtime> updates::UpdateEvents for TauriUpdateEvents<R> {
    fn status_changed(&self, status: &updates::UpdateStatus) {
        let _ = self.app.emit(updates::UPDATE_STATUS, status);
        tray::update_status(&self.app, status);
    }
}

/// Whether this build updates itself: the installed Satchel, with an update
/// feed in its config. Satchel Dev (`tauri.dev.conf.json` empties the feed)
/// never does, and the updater plugin is not even registered for it.
fn updates_supported(config: &tauri::Config) -> bool {
    let has_feed = config
        .plugins
        .0
        .get("updater")
        .and_then(|u| u.get("endpoints"))
        .and_then(|e| e.as_array())
        .is_some_and(|e| !e.is_empty());
    has_feed && !app_identity::is_dev_id(&config.identifier)
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
        .and_then(|dir| {
            let path = dir.join(DB_FILE);
            let new = !path.exists();
            Store::open(&path).map(|store| (store, new)).map_err(|_| ())
        });
    match opened {
        Ok((mut store, new)) => {
            if new {
                start_with_variant_defaults(&mut store);
            }
            Box::new(store)
        }
        Err(()) => {
            // Never fall back to a world-readable location: keep the history
            // in memory for this run instead.
            eprintln!("satchel: cannot open the history database; history is kept in memory only");
            Box::new(Store::open_in_memory().expect("in-memory SQLite is always available"))
        }
    }
}

/// A new history starts with this Satchel's own defaults: Satchel Dev's
/// shelf shortcut differs from the installed Satchel's (see `app_identity`).
fn start_with_variant_defaults(store: &mut Store) {
    let shortcut = app_identity::default_shortcut();
    if let Ok(mut settings) = store.settings()
        && settings.shelf_shortcut != shortcut
    {
        settings.shelf_shortcut = shortcut;
        let _ = store.save_settings(&settings);
    }
}

fn setup<R: Runtime>(app: &AppHandle<R>) -> Result<(), Box<dyn std::error::Error>> {
    // Before anything else: a later copy hands over to the running one and
    // leaves without a tray icon, a shortcut or the store.
    if !instance::claim(&app.config().identifier) {
        std::process::exit(0);
    }
    app_identity::init(&app.config().identifier, &app.package_info().name);

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
        link_wake: Default::default(),
        login_items: Arc::new(login_item::SystemLoginItems),
    });
    app.manage(ctx.clone());

    let shelf = Shelf::create(app)?;
    if !shelf.is_panel() {
        eprintln!("satchel: shelf uses a regular window (non-activating panel unavailable)");
    }
    app.manage(shelf);
    Shelf::install_blur_handler(app);
    instance::listen(app);

    let supported = updates_supported(app.config());
    let updates = updates::Updates::new(
        supported,
        app.package_info().version.to_string(),
        settings.check_for_updates,
        Arc::new(updates::PluginSource { app: app.clone(), found: Mutex::new(None), ctx: ctx.clone() }),
        Arc::new(TauriUpdateEvents { app: app.clone() }),
        Arc::new(SystemClock),
    );
    app.manage(updates.clone());

    tray::create(app, settings.capture_paused, supported)?;

    if shortcuts.register(&settings.shelf_shortcut).is_err() {
        let fallback = app_identity::default_shortcut();
        eprintln!("satchel: the shelf shortcut is unavailable; trying the default");
        let _ = shortcuts.register(&fallback);
    }

    // Idle (blocked on a condition variable) unless "Show link previews" is
    // on and a link is queued; they are the only code that uses the network.
    link_preview::spawn(
        ctx.clone(),
        Arc::new(link_preview::http::HttpFetcher::new()),
        Arc::new(link_preview::SystemImages),
    )?;
    // Registers once when undecided, otherwise adopts the system's state.
    // Off the main thread: ServiceManagement can take a moment.
    let login_ctx = ctx.clone();
    std::thread::Builder::new()
        .name("satchel-login-item".into())
        .spawn(move || login_item::launch(&login_ctx))?;
    watcher::spawn(ctx)?;
    // Installed Satchel only; asleep between checks, and until turned on
    // while "Check for updates automatically" is off.
    updates.spawn_scheduler()?;
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
    let context = tauri::generate_context!();
    let mut builder = tauri::Builder::default();
    if updates_supported(context.config()) {
        builder = builder.plugin(tauri_plugin_updater::Builder::new().build());
    }
    #[allow(unused_mut)]
    let mut app = builder
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
            commands::get_link_image,
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
            commands::get_login_item,
            commands::set_open_at_login,
            commands::hide_shelf,
            commands::open_settings,
            commands::close_settings,
            commands::list_apps,
            commands::resolve_apps,
            commands::get_app_icon,
            commands::choose_app,
            commands::get_third_party_notices,
            commands::get_update_status,
            commands::check_for_updates,
            commands::install_update,
        ])
        .setup(|app| setup(app.handle()))
        .build(context)
        .expect("error while building the app");
    // Satchel is an agent app: no Dock icon, not in the app switcher.
    // `LSUIElement` in the bundle's Info.plist (`src-tauri/Info.plist`) makes
    // macOS launch it as one, and keeps it one when it is opened again while
    // running. tao still applies its own activation policy (Regular unless
    // told otherwise) when launching finishes, before `setup` runs, so it is
    // told here, before the event loop starts. This alone also covers the
    // unbundled binary `pnpm tauri dev` runs.
    #[cfg(target_os = "macos")]
    app.set_activation_policy(tauri::ActivationPolicy::Accessory);
    app.run(|app, event| match event {
        // An accessory app with no visible window must keep running.
        tauri::RunEvent::ExitRequested { code: None, api, .. } => api.prevent_exit(),
        // Opened again while running (Finder, Spotlight, `open -a`).
        #[cfg(target_os = "macos")]
        tauri::RunEvent::Reopen { .. } => instance::show_shelf(app),
        _ => {}
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_installed_satchel_with_a_feed_updates() {
        let read = |path: &str| -> serde_json::Value {
            let text = std::fs::read_to_string(format!("{}/{path}", env!("CARGO_MANIFEST_DIR"))).unwrap();
            serde_json::from_str(&text).unwrap()
        };
        let config = |value: serde_json::Value| -> tauri::Config { serde_json::from_value(value).unwrap() };
        let mut prod = read("tauri.conf.json");
        assert!(updates_supported(&config(prod.clone())));
        // Satchel Dev: the overlay empties the feed and changes the identifier.
        let dev = read("tauri.dev.conf.json");
        let mut merged = prod.clone();
        merged["identifier"] = dev["identifier"].clone();
        merged["plugins"]["updater"]["endpoints"] = dev["plugins"]["updater"]["endpoints"].clone();
        assert!(!updates_supported(&config(merged.clone())));
        merged["plugins"]["updater"]["endpoints"] = prod["plugins"]["updater"]["endpoints"].clone();
        assert!(!updates_supported(&config(merged)), "a Dev identifier never updates");
        prod["plugins"]["updater"]["endpoints"] = serde_json::json!([]);
        assert!(!updates_supported(&config(prod)));
    }

    #[test]
    fn both_variants_are_agent_apps() {
        // Satchel and Satchel Dev (whose overlay does not touch `bundle`)
        // merge `Info.plist` into their bundles; it must say LSUIElement.
        let dir = env!("CARGO_MANIFEST_DIR");
        let config: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(format!("{dir}/tauri.conf.json")).unwrap()).unwrap();
        let name = config["bundle"]["macOS"]["infoPlist"].as_str().expect("bundle.macOS.infoPlist is set");
        let dev: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(format!("{dir}/tauri.dev.conf.json")).unwrap()).unwrap();
        assert!(dev["bundle"].get("macOS").is_none(), "Satchel Dev keeps the same Info.plist");
        let plist = std::fs::read_to_string(format!("{dir}/{name}")).unwrap();
        let compact: String = plist.split_whitespace().collect();
        assert!(compact.contains("<key>LSUIElement</key><true/>"), "LSUIElement must be true");
    }

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
