//! Desktop integration other than the clipboard: which app is frontmost,
//! re-activating it, and synthesizing the paste keystroke.

use std::time::Duration;

use satchel_core::Millis;

#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(not(target_os = "macos"))]
mod unsupported;

#[cfg(target_os = "macos")]
pub use macos::MacDesktop as SystemDesktop;
#[cfg(not(target_os = "macos"))]
pub use unsupported::UnsupportedDesktop as SystemDesktop;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrontApp {
    pub pid: i32,
    /// Bundle id on macOS, executable name elsewhere.
    pub id: String,
    pub name: String,
}

pub trait Desktop: Send + Sync {
    /// The frontmost application, or `None` when it is Satchel itself or
    /// unknown.
    fn frontmost_app(&self) -> Option<FrontApp>;
    /// Brings the app with this pid to the front. Returns false when it no
    /// longer exists.
    fn activate_app(&self, pid: i32) -> bool;
    /// Whether synthesized key events will be delivered (macOS: the
    /// Accessibility permission).
    fn can_post_keystrokes(&self) -> bool;
    /// Shows the OS prompt that leads the user to grant that permission.
    fn request_keystroke_permission(&self);
    /// Sends the platform paste shortcut (Cmd+V / Ctrl+V) to the frontmost
    /// app.
    fn post_paste_keystroke(&self) -> Result<(), ()>;
}

/// Time source, injectable so paste sequencing is tested without sleeping.
pub trait Clock: Send + Sync {
    fn now(&self) -> Millis;
    fn sleep(&self, duration: Duration);
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Millis {
        crate::storage::now_millis()
    }
    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}
