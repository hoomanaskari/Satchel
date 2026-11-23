//! Compiling fallback for platforms whose integration is not written yet.
//!
//! Windows: `GetForegroundWindow` + `GetWindowThreadProcessId` for the
//! frontmost app, `SetForegroundWindow` to restore it, `SendInput` with
//! Ctrl+V for the keystroke.
//! Linux/X11: `_NET_ACTIVE_WINDOW` + `_NET_WM_PID`, `XTest` for Ctrl+V.
//! Until then pasting falls back to copy-only.

use super::{Desktop, FrontApp};

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
    fn post_paste_keystroke(&self) -> Result<(), ()> {
        Err(())
    }
}
