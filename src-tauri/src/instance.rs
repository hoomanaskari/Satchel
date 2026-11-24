//! One Satchel at a time, and what opening it again does.
//!
//! Satchel is an agent app (`LSUIElement` in `src-tauri/Info.plist`): no
//! Dock icon and no entry in the app switcher, ever. Opening it again while
//! it runs (Finder, Spotlight, `open -a`) does what the shelf shortcut does:
//! macOS sends the running copy a reopen event (`RunEvent::Reopen`) and it
//! shows the shelf on the display under the pointer.
//!
//! Launch Services keeps one process per bundle identifier, except with
//! `open -n` (the updater's relaunch) or a second copy of the bundle
//! elsewhere on disk. So at startup the newer process looks for an older
//! one with the same identifier. It waits a moment for it to exit (the
//! updater's relaunch: the old copy exits as soon as the new one is
//! launched); if the old copy stays, the new one asks it to show its shelf
//! and exits before it has a tray icon, a shortcut or the store. Only the
//! newer process ever steps aside, so two copies started together never
//! both leave. Debug builds (`pnpm tauri dev`) skip this, so they can run
//! next to a built Satchel Dev.

use std::time::Duration;

use tauri::{AppHandle, Manager, Runtime};

/// A running copy of this app.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Instance {
    pub pid: i32,
    /// Launch time, seconds since 1970.
    pub launched: f64,
}

/// Whether `this` started after `other`: the later launch steps aside; on
/// an equal launch time, the higher pid.
fn is_newer(this: &Instance, other: &Instance) -> bool {
    match this.launched.partial_cmp(&other.launched) {
        Some(std::cmp::Ordering::Greater) => true,
        Some(std::cmp::Ordering::Less) => false,
        _ => this.pid > other.pid,
    }
}

/// The older copies this one must wait for (empty: start normally).
pub fn older_copies(this: &Instance, others: &[Instance]) -> Vec<i32> {
    others.iter().filter(|other| other.pid != this.pid && is_newer(this, other)).map(|other| other.pid).collect()
}

/// How long the newer copy waits for an older one to exit, and how often it
/// looks.
const WAIT: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(50);

/// Waits until none of `older` is running; false if one still is after
/// `polls` looks.
fn older_copies_exit(older: &[i32], is_running: impl Fn(i32) -> bool, sleep: impl Fn(), polls: u32) -> bool {
    for poll in 0..=polls {
        if !older.iter().any(|&pid| is_running(pid)) {
            return true;
        }
        if poll < polls {
            sleep();
        }
    }
    false
}

/// Name of the "show your shelf" request between copies of one app.
pub fn request_name(bundle_id: &str) -> String {
    format!("{bundle_id}.show-shelf")
}

/// Called first thing at startup. False when another copy of this app is
/// running and stays: it has been asked to show its shelf, and this process
/// must exit.
pub fn claim(bundle_id: &str) -> bool {
    if cfg!(debug_assertions) {
        return true;
    }
    #[cfg(target_os = "macos")]
    {
        use crate::platform::macos_instance as mac;
        let older = older_copies(&mac::this_instance(), &mac::other_instances(bundle_id));
        if older.is_empty() {
            return true;
        }
        let polls = (WAIT.as_millis() / POLL.as_millis()) as u32;
        if older_copies_exit(&older, mac::is_running, || std::thread::sleep(POLL), polls) {
            return true;
        }
        mac::post_show_request(&request_name(bundle_id));
        false
    }
    // TODO(windows, linux): a named mutex / lock file.
    #[cfg(not(target_os = "macos"))]
    {
        let _ = bundle_id;
        true
    }
}

/// Listens for later copies' requests and starts remembering the app that
/// was active before Satchel. Main thread, once, after the shelf exists.
pub fn listen<R: Runtime>(app: &AppHandle<R>) {
    #[cfg(target_os = "macos")]
    {
        use crate::platform::macos_instance as mac;
        mac::track_active_apps();
        let handle = app.clone();
        mac::observe_show_requests(&request_name(&app.config().identifier), move || {
            let inner = handle.clone();
            let _ = handle.run_on_main_thread(move || show_shelf(&inner));
        });
    }
    #[cfg(not(target_os = "macos"))]
    let _ = app;
}

/// Satchel was opened again (Finder, Spotlight, `open -a`, a later copy):
/// shows the shelf as the shortcut does, or leaves it up. Main thread.
pub fn show_shelf<R: Runtime>(app: &AppHandle<R>) {
    use std::sync::Arc;
    let (Some(shelf), Some(ctx)) = (app.try_state::<crate::shelf::Shelf<R>>(), app.try_state::<Arc<crate::ctx::Ctx>>())
    else {
        return;
    };
    if shelf.is_shown() {
        return;
    }
    shelf.show(app, &ctx);
    // Opening an app activates it, so Satchel is the frontmost app now and
    // the shelf found no app to paste into or give focus back to: use the
    // one that was active before. Not while Settings is open: then Satchel
    // was the app in use, as when the shortcut is pressed over Settings.
    #[cfg(target_os = "macos")]
    if ctx.previous_app().is_none() && app.get_webview_window(crate::settings_window::LABEL).is_none() {
        ctx.remember_previous_app(crate::platform::macos_instance::last_other_active_app());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn at(pid: i32, launched: f64) -> Instance {
        Instance { pid, launched }
    }

    #[test]
    fn the_only_copy_starts() {
        assert!(older_copies(&at(10, 100.0), &[]).is_empty());
    }

    #[test]
    fn a_later_copy_waits_for_the_earlier_one() {
        assert_eq!(older_copies(&at(20, 200.0), &[at(10, 100.0)]), vec![10]);
    }

    #[test]
    fn the_earlier_copy_never_steps_aside() {
        assert!(older_copies(&at(10, 100.0), &[at(20, 200.0)]).is_empty());
    }

    #[test]
    fn copies_launched_together_agree_on_one() {
        // Same launch time: the lower pid stays, so exactly one leaves.
        let (a, b) = (at(10, 100.0), at(11, 100.0));
        assert!(older_copies(&a, &[b]).is_empty());
        assert_eq!(older_copies(&b, &[a]), vec![10]);
        // An unknown launch time (NaN) also falls back to the pid.
        let (c, d) = (at(10, f64::NAN), at(11, 100.0));
        assert!(older_copies(&c, &[d]).is_empty());
        assert_eq!(older_copies(&d, &[c]), vec![10]);
    }

    #[test]
    fn own_pid_is_ignored() {
        assert!(older_copies(&at(10, 200.0), &[at(10, 100.0)]).is_empty());
    }

    #[test]
    fn the_relaunch_goes_on_once_the_old_copy_exits() {
        // The updater's relaunch: the old copy is gone after a few looks.
        let looks = Cell::new(0);
        let running = |_pid: i32| {
            looks.set(looks.get() + 1);
            looks.get() <= 3
        };
        let sleeps = Cell::new(0);
        assert!(older_copies_exit(&[10], running, || sleeps.set(sleeps.get() + 1), 100));
        assert_eq!(sleeps.get(), 3);
    }

    #[test]
    fn a_copy_that_stays_gets_the_request_instead() {
        let sleeps = Cell::new(0);
        assert!(!older_copies_exit(&[10], |_| true, || sleeps.set(sleeps.get() + 1), 5));
        assert_eq!(sleeps.get(), 5, "waits the whole time, then gives up without a last sleep");
    }

    #[test]
    fn waits_for_every_older_copy() {
        let looks = Cell::new(0);
        let running = |pid: i32| {
            looks.set(looks.get() + 1);
            pid == 11 && looks.get() < 6
        };
        assert!(older_copies_exit(&[10, 11], running, || {}, 100));
    }

    #[test]
    fn requests_are_per_app() {
        assert_eq!(request_name("app.satchel.desktop"), "app.satchel.desktop.show-shelf");
        assert_ne!(request_name("app.satchel.desktop"), request_name("app.satchel.desktop.dev"));
    }
}
