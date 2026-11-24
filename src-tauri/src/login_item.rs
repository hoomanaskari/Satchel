//! "Open Satchel at login".
//!
//! macOS 13+ only, through `SMAppService.mainAppService`
//! (`platform/macos_login.rs`): the app bundle itself is the login item, as
//! listed in System Settings › General › Login Items, so it follows the app
//! when it is updated and the user can remove it there.
//!
//! On by default: the stored setting starts undecided (`None`), and the
//! first launch that can register does so and stores the result. Settings
//! saved by 0.1.2, which had no such setting, are undecided too, so an
//! upgrade registers once. After that Satchel never registers on its own:
//! at each launch and each time Settings asks, the system's state is read
//! and stored, so turning it off in System Settings sticks. Only the
//! checkbox in Settings registers again.
//!
//! Offered only where it makes sense ([`unavailable`]): never for Satchel
//! Dev or a debug build (they are never login items, and never touch the
//! installed Satchel's, which belongs to another bundle identifier), never
//! before macOS 13, and only for a bundle in `/Applications` or
//! `~/Applications` (not a build in `target/`, a mounted DMG, a translocated
//! copy or a test folder), so the login item never points at a copy that
//! goes away.
//!
//! Launched at login, Satchel does what it does at any launch: nothing
//! visible (no shelf, no window, no Dock icon) and no change of focus.

use serde::Serialize;

use crate::ctx::Ctx;
use crate::error::{CmdResult, invalid};

/// Where the login item stands (`SMAppServiceStatus`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Enabled,
    /// Registered, waiting for the user to allow it in System Settings.
    RequiresApproval,
    NotRegistered,
    NotFound,
}

impl Status {
    /// Whether the user wants it on (a pending approval counts).
    pub fn is_on(self) -> bool {
        matches!(self, Status::Enabled | Status::RequiresApproval)
    }
}

/// Why the option is not offered here.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Unavailable {
    /// Satchel Dev or a debug build.
    Dev,
    /// Before macOS 13 (no `SMAppService`).
    Macos,
    /// Not in /Applications or ~/Applications.
    Location,
}

/// What Settings shows.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LoginItem {
    /// The system's state (false when unavailable).
    pub enabled: bool,
    /// On, but macOS waits for the user to allow it in System Settings.
    pub requires_approval: bool,
    /// Set when the option is not offered; the checkbox is then disabled.
    pub unavailable: Option<Unavailable>,
}

/// The system side, behind a seam for tests.
pub trait LoginItems: Send + Sync {
    /// Why the option is not offered, if it is not.
    fn unavailable(&self) -> Option<Unavailable>;
    /// The system's state; `None` if it cannot be read.
    fn status(&self) -> Option<Status>;
    /// Registers (true) or unregisters (false).
    fn set(&self, on: bool) -> Result<(), ()>;
}

/// A bundle path from which Satchel may register: an `.app` in
/// `/Applications` or `~/Applications` (or a folder inside them), never a
/// translocated copy (Gatekeeper runs quarantined apps opened elsewhere from
/// a random read-only path) or one on a mounted disk image (`/Volumes`).
pub fn is_stable_location(path: &str, home: Option<&str>) -> bool {
    if !path.ends_with(".app") || path.contains("/AppTranslocation/") || path.contains("/../") {
        return false;
    }
    let user_apps = home.filter(|h| h.starts_with('/')).map(|h| format!("{}/Applications/", h.trim_end_matches('/')));
    path.starts_with("/Applications/") || user_apps.is_some_and(|dir| path.starts_with(&dir))
}

/// Why the option is not offered, from what was found (checked in this
/// order: the variant, the OS, the location).
pub fn availability(dev: bool, supported: bool, stable: bool) -> Option<Unavailable> {
    if dev {
        Some(Unavailable::Dev)
    } else if !supported {
        Some(Unavailable::Macos)
    } else if !stable {
        Some(Unavailable::Location)
    } else {
        None
    }
}

/// What to do at launch, where the option is offered: register only while
/// undecided (a new install, or settings from before the option existed);
/// otherwise adopt the system's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AtLaunch {
    Register,
    Adopt(bool),
}

pub fn at_launch(stored: Option<bool>, status: Status) -> AtLaunch {
    match stored {
        None => AtLaunch::Register,
        Some(_) => AtLaunch::Adopt(status.is_on()),
    }
}

/// Stores `on` as the setting if it differs. Not announced: nothing
/// reacts to it, and `settings-changed` would reset unsaved edits in the
/// Settings form, which shows the system's state (`get`) instead.
fn store(ctx: &Ctx, on: bool) {
    // Under the settings lock, as `save_settings` updates it, so neither
    // undoes the other.
    let mut settings = ctx.settings.write().unwrap_or_else(|p| p.into_inner());
    if settings.open_at_login == Some(on) {
        return;
    }
    if ctx.store.lock().save_open_at_login(Some(on)).is_ok() {
        settings.open_at_login = Some(on);
    }
}

/// At launch: registers once if undecided, otherwise takes the system's
/// state. Does nothing where the option is not offered (a copy that is
/// later moved to /Applications registers at its first launch there).
pub fn launch(ctx: &Ctx) {
    if ctx.login_items.unavailable().is_some() {
        return;
    }
    let Some(status) = ctx.login_items.status() else { return };
    match at_launch(ctx.settings().open_at_login, status) {
        AtLaunch::Register => {
            // A failure stays undecided, to try again at the next launch.
            if status.is_on() || ctx.login_items.set(true).is_ok() {
                let now = ctx.login_items.status().is_some_and(Status::is_on);
                store(ctx, now);
            }
        }
        AtLaunch::Adopt(on) => store(ctx, on),
    }
}

/// For Settings, each time it opens: the system's state, which also
/// becomes the stored one.
pub fn get(ctx: &Ctx) -> LoginItem {
    if let Some(why) = ctx.login_items.unavailable() {
        return LoginItem { enabled: false, requires_approval: false, unavailable: Some(why) };
    }
    let status = ctx.login_items.status();
    if let Some(status) = status {
        store(ctx, status.is_on());
    }
    LoginItem {
        enabled: status.is_some_and(Status::is_on),
        requires_approval: status == Some(Status::RequiresApproval),
        unavailable: None,
    }
}

/// The checkbox in Settings: registers or unregisters, then reports where
/// it stands.
pub fn set(ctx: &Ctx, on: bool) -> CmdResult<LoginItem> {
    if ctx.login_items.unavailable().is_some() {
        return Err(invalid("openAtLogin"));
    }
    let failed = ctx.login_items.set(on).is_err();
    let item = get(ctx);
    if failed && item.enabled != on {
        let name = crate::app_identity::name();
        return Err(if on {
            format!("macOS did not add {name} to the login items.")
        } else {
            format!("macOS did not remove {name} from the login items.")
        });
    }
    Ok(item)
}

/// `SMAppService` for this bundle.
#[cfg(target_os = "macos")]
pub struct SystemLoginItems;

#[cfg(target_os = "macos")]
impl LoginItems for SystemLoginItems {
    fn unavailable(&self) -> Option<Unavailable> {
        use crate::platform::macos_login as mac;
        let dev = cfg!(debug_assertions) || crate::app_identity::is_dev();
        let home = std::env::var("HOME").ok();
        let stable = mac::bundle_path().is_some_and(|p| is_stable_location(&p, home.as_deref()));
        availability(dev, mac::supported(), stable)
    }
    fn status(&self) -> Option<Status> {
        crate::platform::macos_login::status()
    }
    fn set(&self, on: bool) -> Result<(), ()> {
        crate::platform::macos_login::set(on)
    }
}

/// TODO(windows, linux): a Run key / an XDG autostart entry.
#[cfg(not(target_os = "macos"))]
pub struct SystemLoginItems;

#[cfg(not(target_os = "macos"))]
impl LoginItems for SystemLoginItems {
    fn unavailable(&self) -> Option<Unavailable> {
        Some(Unavailable::Macos)
    }
    fn status(&self) -> Option<Status> {
        None
    }
    fn set(&self, _on: bool) -> Result<(), ()> {
        Err(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Harness;

    #[test]
    fn only_applications_folders_are_stable() {
        let home = Some("/Users/ana");
        assert!(is_stable_location("/Applications/Satchel.app", home));
        assert!(is_stable_location("/Applications/Utilities/Satchel.app", home));
        assert!(is_stable_location("/Users/ana/Applications/Satchel.app", home));
        assert!(is_stable_location("/Users/ana/Applications/Satchel.app", Some("/Users/ana/")));
        for path in [
            "/Users/ana/Sites/satchel/target/release/bundle/macos/Satchel.app",
            "/Volumes/Satchel/Satchel.app",
            "/private/var/folders/x/AppTranslocation/1234/d/Satchel.app",
            "/Users/ana/Downloads/Satchel.app",
            "/Users/bob/Applications/Satchel.app",
            "/private/tmp/scratch/Satchel.app",
            "/Applications/../tmp/Satchel.app",
            "/Applications/Satchel.app/Contents/MacOS/satchel",
            "/ApplicationsX/Satchel.app",
        ] {
            assert!(!is_stable_location(path, home), "{path}");
        }
        assert!(!is_stable_location("/Users/ana/Applications/Satchel.app", None));
        assert!(!is_stable_location("/Users/ana/Applications/Satchel.app", Some("")));
    }

    #[test]
    fn availability_names_the_first_reason() {
        assert_eq!(availability(false, true, true), None);
        assert_eq!(availability(true, true, true), Some(Unavailable::Dev));
        assert_eq!(availability(true, false, false), Some(Unavailable::Dev));
        assert_eq!(availability(false, false, false), Some(Unavailable::Macos));
        assert_eq!(availability(false, true, false), Some(Unavailable::Location));
    }

    #[test]
    fn registers_only_while_undecided() {
        assert_eq!(at_launch(None, Status::NotRegistered), AtLaunch::Register);
        // Turned off in Settings or in System Settings: never again on its own.
        assert_eq!(at_launch(Some(true), Status::NotRegistered), AtLaunch::Adopt(false));
        assert_eq!(at_launch(Some(false), Status::NotRegistered), AtLaunch::Adopt(false));
        // Turned on in System Settings while stored off: the system wins.
        assert_eq!(at_launch(Some(false), Status::Enabled), AtLaunch::Adopt(true));
        assert_eq!(at_launch(Some(true), Status::RequiresApproval), AtLaunch::Adopt(true));
    }

    fn harness(stored: Option<bool>, status: Status) -> Harness {
        let h = Harness::new();
        let mut settings = h.ctx.settings();
        settings.open_at_login = stored;
        h.ctx.set_settings(settings);
        h.login.set_status(status);
        h
    }

    #[test]
    fn a_new_install_registers_at_first_launch() {
        let h = harness(None, Status::NotRegistered);
        launch(&h.ctx);
        assert_eq!(h.login.status(), Some(Status::Enabled));
        assert_eq!(h.ctx.settings().open_at_login, Some(true));
        assert_eq!(h.login.calls(), vec![true]);
    }

    #[test]
    fn an_install_turned_off_in_system_settings_stays_off() {
        let h = harness(Some(true), Status::NotRegistered);
        launch(&h.ctx);
        assert!(h.login.calls().is_empty(), "never registers again on its own");
        assert_eq!(h.ctx.settings().open_at_login, Some(false));
        assert!(!get(&h.ctx).enabled);
    }

    #[test]
    fn a_failed_first_registration_stays_undecided() {
        let h = harness(None, Status::NotRegistered);
        h.login.fail();
        launch(&h.ctx);
        assert_eq!(h.ctx.settings().open_at_login, None, "tries again next launch");
    }

    #[test]
    fn nothing_happens_where_it_is_not_offered() {
        for why in [Unavailable::Dev, Unavailable::Macos, Unavailable::Location] {
            let h = harness(None, Status::NotRegistered);
            h.login.set_unavailable(Some(why));
            launch(&h.ctx);
            assert!(h.login.calls().is_empty());
            assert_eq!(h.ctx.settings().open_at_login, None, "moved to /Applications later: registers then");
            assert_eq!(get(&h.ctx), LoginItem { enabled: false, requires_approval: false, unavailable: Some(why) });
            assert!(set(&h.ctx, true).is_err());
            assert!(h.login.calls().is_empty());
        }
    }

    #[test]
    fn settings_shows_and_stores_the_system_state() {
        let h = harness(Some(false), Status::RequiresApproval);
        assert_eq!(get(&h.ctx), LoginItem { enabled: true, requires_approval: true, unavailable: None });
        assert_eq!(h.ctx.settings().open_at_login, Some(true));
    }

    #[test]
    fn the_checkbox_registers_and_unregisters() {
        let h = harness(Some(true), Status::Enabled);
        assert!(!set(&h.ctx, false).unwrap().enabled);
        assert_eq!(h.ctx.settings().open_at_login, Some(false));
        assert!(set(&h.ctx, true).unwrap().enabled);
        assert_eq!(h.ctx.settings().open_at_login, Some(true));
        assert_eq!(h.login.calls(), vec![false, true]);
        h.login.fail();
        assert!(set(&h.ctx, false).is_err(), "a refusal is reported");
        assert_eq!(h.ctx.settings().open_at_login, Some(true), "and the state is still the system's");
    }

    #[test]
    fn saving_the_settings_form_never_changes_it() {
        let h = harness(Some(false), Status::NotRegistered);
        let mut draft = h.ctx.settings();
        draft.open_at_login = Some(true);
        crate::commands::api::save_settings(&h.ctx, draft).unwrap();
        assert_eq!(h.ctx.settings().open_at_login, Some(false));
        assert!(h.login.calls().is_empty());
    }
}
