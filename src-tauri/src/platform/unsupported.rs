//! Compiling fallback for platforms whose integration is not written yet.
//!
//! Windows: `GetForegroundWindow` + `GetWindowThreadProcessId` for the
//! frontmost app, `SetForegroundWindow` to restore it, `SendInput` with
//! Ctrl+V for the keystroke.
//! Linux/X11: `_NET_ACTIVE_WINDOW` + `_NET_WM_PID`, `XTest` for Ctrl+V.
//! Until then pasting falls back to copy-only.

use super::{AppCatalog, AppInfo, Desktop, FrontApp, MainThreadRunner};

pub struct UnsupportedDesktop;

impl UnsupportedDesktop {
    pub fn new() -> Self {
        Self
    }
}

impl Desktop for UnsupportedDesktop {
    fn frontmost_app(&self) -> Option<FrontApp> {
        None
    }
    fn activate_app(&self, _pid: i32) -> bool {
        false
    }
    fn can_post_keystrokes(&self) -> bool {
        false
    }
    fn request_keystroke_permission(&self) {}
    fn open_keystroke_permission_settings(&self) {}
    fn post_paste_keystroke(&self) -> Result<(), ()> {
        Err(())
    }
}

/// No application catalog yet. Windows: the Start menu's AppsFolder or the
/// Uninstall registry keys plus `EnumProcesses`; Linux: `.desktop` files in
/// the XDG data dirs. Until then the settings window falls back to adding
/// apps by identifier.
pub struct UnsupportedApps;

impl UnsupportedApps {
    pub fn new(_run_on_main: MainThreadRunner) -> Self {
        Self
    }
}

impl AppCatalog for UnsupportedApps {
    fn installed_apps(&self) -> Vec<AppInfo> {
        Vec::new()
    }
    fn running_apps(&self) -> Vec<AppInfo> {
        Vec::new()
    }
    fn find_app(&self, _id: &str) -> Option<AppInfo> {
        None
    }
    fn icon_png(&self, _id: &str, _px: u32) -> Option<Vec<u8>> {
        None
    }
    fn choose_app(&self) -> Option<AppInfo> {
        None
    }
}
