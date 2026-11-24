//! Menu bar / tray icon: Show Satchel, Settings, Check for Updates, Pause
//! Capturing, Clear History, Quit. While an update is on offer the icon
//! carries a dot and "Update to Satchel <version>…" heads the menu.
//! Satchel Dev says its own name, tints the icon blue and has no update
//! items.

use std::sync::{Arc, Mutex};

use tauri::image::Image;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, Runtime};

use crate::commands::api;
use crate::ctx::Ctx;
use crate::shelf::Shelf;
use crate::tray_anim::PostGate;
use crate::updates::{Phase, UpdateStatus, Updates};

pub const ID_SHOW: &str = "show";
pub const ID_SETTINGS: &str = "settings";
pub const ID_PAUSE: &str = "pause";
pub const ID_CLEAR: &str = "clear";
pub const ID_QUIT: &str = "quit";
pub const ID_CHECK_UPDATES: &str = "check-updates";
pub const ID_UPDATE: &str = "update";

/// Icon side in pixels: 18 pt at 2x, the menu bar size on Retina screens.
pub const ICON_SIZE: u32 = 36;

/// Pause item, kept so settings changes from the UI update its check mark.
pub struct PauseItem<R: Runtime>(pub CheckMenuItem<R>);

/// The resting satchel glyph as a black-on-transparent RGBA template image
/// (drawn in `tray_anim`, which also draws the animation frames).
pub fn icon_rgba(size: u32) -> Vec<u8> {
    crate::tray_anim::render(size, &crate::tray_anim::Pose::REST)
}

/// The update items: "Update to …" (with its separator) is in the menu
/// only while an update is on offer; "Check for Updates…" always is.
pub struct UpdateMenu<R: Runtime> {
    menu: Menu<R>,
    update: MenuItem<R>,
    separator: PredefinedMenuItem<R>,
    check: MenuItem<R>,
    shown: Mutex<bool>,
}

/// The "Update to …" item for `phase`: its text and whether it can be
/// chosen; `None` when there is no update to offer.
pub fn update_item(name: &str, phase: &Phase) -> Option<(String, bool)> {
    Some(match phase {
        Phase::Idle => return None,
        Phase::Available { version } | Phase::Failed { version, .. } => (format!("Update to {name} {version}…"), true),
        Phase::Downloading { version, downloaded, total } => {
            let pct = total.filter(|t| *t > 0).map(|t| format!(" {}%", (downloaded.saturating_mul(100) / t).min(100)));
            (format!("Downloading {name} {version}…{}", pct.unwrap_or_default()), false)
        }
        Phase::Installing { version } => (format!("Installing {name} {version}…"), false),
    })
}

/// The "Check for Updates…" item: its text and whether it can be chosen.
pub fn check_item(status: &UpdateStatus) -> (&'static str, bool) {
    if status.checking {
        ("Checking for Updates…", false)
    } else {
        ("Check for Updates…", !matches!(status.phase, Phase::Downloading { .. } | Phase::Installing { .. }))
    }
}

/// A popup the tray shows after something the user asked for.
#[derive(Debug, PartialEq, Eq)]
pub struct Popup {
    pub title: String,
    pub message: String,
    pub buttons: &'static [&'static str],
    /// The first button installs the update on offer.
    pub installs: bool,
}

impl Popup {
    fn ok(title: String, message: String) -> Self {
        Self { title, message, buttons: &["OK"], installs: false }
    }
}

/// What "Check for Updates…" says once its check is done.
pub fn check_popup(name: &str, status: &UpdateStatus) -> Popup {
    use crate::updates::CheckOutcome;
    let current = &status.current_version;
    match (&status.phase, status.last_check.map(|c| c.outcome)) {
        (_, Some(CheckOutcome::Failed)) => Popup::ok(
            "Couldn’t check for updates".into(),
            format!("{name} couldn’t reach GitHub. Check your connection and try again."),
        ),
        (Phase::Available { version } | Phase::Failed { version, .. }, _) => Popup {
            title: format!("{name} {version} is available"),
            message: format!(
                "You have {name} {current}. {name} downloads the update, checks its signature, installs it and reopens."
            ),
            buttons: &["Update", "Later"],
            installs: true,
        },
        (Phase::Downloading { version, .. } | Phase::Installing { version }, _) => {
            Popup::ok(format!("{name} {version} is being installed"), format!("{name} reopens when it is done."))
        }
        (Phase::Idle, _) => Popup::ok(format!("{name} is up to date"), format!("{name} {current} is the newest version.")),
    }
}

/// What a failed update the user started from the menu says. Content-free.
pub fn failure_popup(name: &str, version: &str, during: crate::updates::Failure) -> Popup {
    use crate::updates::Failure;
    let message = match during {
        Failure::Download => "It couldn’t be downloaded. Check your connection and try again.".to_string(),
        Failure::Verify => "It couldn’t be verified, so it was not installed.".to_string(),
        Failure::Install => format!("It couldn’t be installed. Try again, or download {name} from GitHub."),
    };
    Popup::ok(format!("{name} {version} was not installed"), message)
}

pub fn create<R: Runtime>(app: &AppHandle<R>, paused: bool, updates: bool) -> tauri::Result<()> {
    let name = crate::app_identity::name();
    let show = MenuItem::with_id(app, ID_SHOW, format!("Show {name}"), true, None::<&str>)?;
    let settings = MenuItem::with_id(app, ID_SETTINGS, "Settings…", true, None::<&str>)?;
    let check = MenuItem::with_id(app, ID_CHECK_UPDATES, "Check for Updates…", true, None::<&str>)?;
    let pause = CheckMenuItem::with_id(app, ID_PAUSE, "Pause Capturing", true, paused, None::<&str>)?;
    let clear = MenuItem::with_id(app, ID_CLEAR, "Clear History…", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, ID_QUIT, format!("Quit {name}"), true, Some("CmdOrCtrl+Q"))?;
    let (sep1, sep2, sep3) =
        (PredefinedMenuItem::separator(app)?, PredefinedMenuItem::separator(app)?, PredefinedMenuItem::separator(app)?);
    let mut items: Vec<&dyn tauri::menu::IsMenuItem<R>> = vec![&show, &settings];
    if updates {
        items.push(&check);
    }
    items.extend([&sep1 as &dyn tauri::menu::IsMenuItem<R>, &pause, &sep2, &clear, &sep3, &quit]);
    let menu = Menu::with_items(app, &items)?;
    app.manage(PauseItem(pause));
    if updates {
        app.manage(UpdateMenu {
            menu: menu.clone(),
            update: MenuItem::with_id(app, ID_UPDATE, "", true, None::<&str>)?,
            separator: PredefinedMenuItem::separator(app)?,
            check,
            shown: Mutex::new(false),
        });
    }

    let tray = TrayIconBuilder::with_id("satchel")
        .icon(Image::new_owned(icon_rgba(ICON_SIZE), ICON_SIZE, ICON_SIZE))
        .icon_as_template(true)
        .tooltip(name)
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| handle_menu(app, event.id().as_ref()))
        .build(app)?;
    // The animation drives the native status item directly (see
    // `platform::macos_tray`). Setup runs on the main thread.
    #[cfg(target_os = "macos")]
    let _ = tray.with_inner_tray_icon(|inner| {
        if let (Some(item), Some(mtm)) = (inner.ns_status_item(), objc2::MainThreadMarker::new()) {
            crate::platform::macos_tray::install(mtm, &item);
        }
    });
    #[cfg(not(target_os = "macos"))]
    let _ = tray;
    Ok(())
}

/// One "clip saved" hop to the main thread at a time.
static CLIP_SAVED: PostGate = PostGate::new();

/// Called by capture (on the watcher thread) when a clip is stored, new or
/// re-copied: queues the menu bar animation on the main thread and returns
/// at once.
/// Clips that arrive while a hop is queued ride along with it.
pub fn clip_saved<R: Runtime>(app: &AppHandle<R>) {
    if !CLIP_SAVED.claim() {
        return;
    }
    let queued = app
        .run_on_main_thread(|| {
            CLIP_SAVED.release();
            crate::platform::tray_clip_saved();
        })
        .is_ok();
    if !queued {
        CLIP_SAVED.release();
    }
}

/// The update status changed: badge the icon and update the menu, on the
/// main thread. Never opens a window or takes focus.
pub fn update_status<R: Runtime>(app: &AppHandle<R>, status: &UpdateStatus) {
    let status = status.clone();
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        crate::platform::tray_set_badge(status.phase.version().is_some());
        let Some(m) = handle.try_state::<UpdateMenu<R>>() else { return };
        let (text, enabled) = check_item(&status);
        let _ = m.check.set_text(text);
        let _ = m.check.set_enabled(enabled);
        let mut shown = m.shown.lock().unwrap_or_else(|p| p.into_inner());
        match update_item(crate::app_identity::name(), &status.phase) {
            Some((text, enabled)) => {
                let _ = m.update.set_text(text);
                let _ = m.update.set_enabled(enabled);
                if !*shown {
                    *shown = m.menu.insert(&m.update, 0).is_ok();
                    let _ = m.menu.insert(&m.separator, 1);
                }
            }
            None if *shown => {
                let _ = m.menu.remove(&m.update);
                let _ = m.menu.remove(&m.separator);
                *shown = false;
            }
            None => {}
        }
    });
}

fn handle_menu<R: Runtime>(app: &AppHandle<R>, id: &str) {
    let Some(ctx) = app.try_state::<Arc<Ctx>>() else { return };
    let ctx = ctx.inner().clone();
    match id {
        ID_SHOW => {
            if let Some(shelf) = app.try_state::<Shelf<R>>()
                && !shelf.is_shown()
            {
                shelf.show(app, &ctx);
            }
        }
        ID_SETTINGS => api::open_settings(&ctx),
        // Answered with a popup, not the settings window. Progress shows in
        // this menu's item, the shelf and Settings (when open).
        ID_CHECK_UPDATES => {
            if let Some(updates) = app.try_state::<Arc<Updates>>() {
                check_and_tell(app.clone(), updates.inner().clone());
            }
        }
        ID_UPDATE => {
            if let Some(updates) = app.try_state::<Arc<Updates>>() {
                install_and_tell(app.clone(), updates.inner().clone());
            }
        }
        ID_PAUSE => {
            let paused = app
                .try_state::<PauseItem<R>>()
                .and_then(|item| item.0.is_checked().ok())
                .unwrap_or(!ctx.capture_paused());
            let _ = api::set_capture_paused(&ctx, paused);
        }
        ID_CLEAR => {
            if confirm_clear() {
                // The store call can take a moment; keep the main thread free.
                std::thread::spawn(move || {
                    let _ = api::clear_history(&ctx);
                });
            }
        }
        ID_QUIT => app.exit(0),
        _ => {}
    }
}

/// Checks now (off the main thread), then says what it found in a popup.
fn check_and_tell<R: Runtime>(app: AppHandle<R>, updates: Arc<Updates>) {
    std::thread::spawn(move || {
        updates.run_check();
        // A check already in flight (the automatic one) finishes first.
        let mut status = updates.status();
        for _ in 0..400 {
            if !status.checking {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
            status = updates.status();
        }
        let popup = check_popup(crate::app_identity::name(), &status);
        let then_app = app.clone();
        show_popup(&app, popup, move |install| {
            if install {
                install_and_tell(then_app, updates);
            }
        });
    });
}

/// Starts the update the user chose and, if it fails, says so. On success
/// Satchel relaunches into the new version, which says it all.
fn install_and_tell<R: Runtime>(app: AppHandle<R>, updates: Arc<Updates>) {
    if updates.install().is_err() {
        return;
    }
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(std::time::Duration::from_millis(200));
            let status = updates.status();
            match status.phase {
                Phase::Downloading { .. } | Phase::Installing { .. } => {}
                Phase::Failed { version, during } => {
                    show_popup(&app, failure_popup(crate::app_identity::name(), &version, during), |_| {});
                    return;
                }
                _ => return,
            }
        }
    });
}

fn show_popup<R: Runtime>(app: &AppHandle<R>, popup: Popup, then: impl FnOnce(bool) + Send + 'static) {
    let _ = app.run_on_main_thread(move || {
        #[cfg(target_os = "macos")]
        let pressed = objc2::MainThreadMarker::new().map(|mtm| {
            let buttons: Vec<&str> = popup.buttons.to_vec();
            crate::platform::macos::alert(mtm, &popup.title, &popup.message, &buttons)
        });
        // TODO(windows, linux): a native dialog. Until then nothing is shown.
        #[cfg(not(target_os = "macos"))]
        let pressed: Option<usize> = None;
        let install = popup.installs && pressed == Some(0);
        // Leave the main thread before doing anything slow.
        std::thread::spawn(move || then(install));
    });
}

#[cfg(target_os = "macos")]
fn confirm_clear() -> bool {
    let Some(mtm) = objc2::MainThreadMarker::new() else { return false };
    crate::platform::macos::confirm(
        mtm,
        "Clear clipboard history?",
        "All clips that are not on a pinboard will be deleted. This cannot be undone.",
        "Clear History",
    )
}

/// TODO(windows, linux): native confirmation (MessageBoxW / GTK dialog).
/// Until then the destructive action is refused rather than run unasked.
#[cfg(not(target_os = "macos"))]
fn confirm_clear() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_items_follow_the_phase() {
        use crate::updates::Failure;
        let v = || "0.2.0".to_string();
        assert_eq!(update_item("Satchel", &Phase::Idle), None);
        assert_eq!(update_item("Satchel", &Phase::Available { version: v() }), Some(("Update to Satchel 0.2.0…".into(), true)));
        assert_eq!(
            update_item("Satchel", &Phase::Failed { version: v(), during: Failure::Download }),
            Some(("Update to Satchel 0.2.0…".into(), true))
        );
        assert_eq!(
            update_item("Satchel", &Phase::Downloading { version: v(), downloaded: 42, total: Some(100) }),
            Some(("Downloading Satchel 0.2.0… 42%".into(), false))
        );
        assert_eq!(
            update_item("Satchel", &Phase::Downloading { version: v(), downloaded: 42, total: None }),
            Some(("Downloading Satchel 0.2.0…".into(), false))
        );
        assert_eq!(update_item("Satchel", &Phase::Installing { version: v() }), Some(("Installing Satchel 0.2.0…".into(), false)));
        let mut s = UpdateStatus { supported: true, current_version: "0.1.0".into(), checking: true, phase: Phase::Idle, last_check: None };
        assert_eq!(check_item(&s), ("Checking for Updates…", false));
        s.checking = false;
        assert_eq!(check_item(&s), ("Check for Updates…", true));
        s.phase = Phase::Installing { version: v() };
        assert_eq!(check_item(&s), ("Check for Updates…", false));
    }

    #[test]
    fn check_for_updates_answers_in_a_popup() {
        use crate::updates::{CheckOutcome, Failure, LastCheck};
        let status = |phase: Phase, outcome: CheckOutcome| UpdateStatus {
            supported: true,
            current_version: "0.1.1".into(),
            checking: false,
            phase,
            last_check: Some(LastCheck { at: 1, outcome }),
        };
        let up = check_popup("Satchel", &status(Phase::Idle, CheckOutcome::UpToDate));
        assert_eq!(up.title, "Satchel is up to date");
        assert_eq!(up.message, "Satchel 0.1.1 is the newest version.");
        assert_eq!((up.buttons, up.installs), (&["OK"][..], false));
        let offer = check_popup("Satchel", &status(Phase::Available { version: "0.2.0".into() }, CheckOutcome::Available));
        assert_eq!(offer.title, "Satchel 0.2.0 is available");
        assert!(offer.message.starts_with("You have Satchel 0.1.1."));
        assert_eq!((offer.buttons, offer.installs), (&["Update", "Later"][..], true));
        let failed = check_popup("Satchel", &status(Phase::Available { version: "0.2.0".into() }, CheckOutcome::Failed));
        assert_eq!(failed.title, "Couldn’t check for updates");
        assert!(!failed.installs);
        let busy = check_popup("Satchel", &status(Phase::Installing { version: "0.2.0".into() }, CheckOutcome::Available));
        assert!(!busy.installs);
        let f = failure_popup("Satchel", "0.2.0", Failure::Verify);
        assert_eq!(f.title, "Satchel 0.2.0 was not installed");
        assert_eq!(f.message, "It couldn’t be verified, so it was not installed.");
    }

    #[test]
    fn icon_has_the_right_size_and_is_a_template() {
        let rgba = icon_rgba(ICON_SIZE);
        assert_eq!(rgba.len(), (ICON_SIZE * ICON_SIZE * 4) as usize);
        assert!(rgba.chunks(4).all(|p| p[0] == 0 && p[1] == 0 && p[2] == 0));
    }

    #[test]
    fn icon_has_ink_in_the_body_and_clear_corners() {
        let rgba = icon_rgba(ICON_SIZE);
        let alpha = |x: u32, y: u32| rgba[((y * ICON_SIZE + x) * 4 + 3) as usize];
        assert_eq!(alpha(0, 0), 0);
        assert_eq!(alpha(ICON_SIZE - 1, 0), 0);
        assert_eq!(alpha(ICON_SIZE / 2, ICON_SIZE * 3 / 4), 255);
        let ink = rgba.chunks(4).filter(|p| p[3] > 0).count();
        let total = (ICON_SIZE * ICON_SIZE) as usize;
        assert!(ink > total / 5 && ink < total * 3 / 4, "{ink}");
    }
}
