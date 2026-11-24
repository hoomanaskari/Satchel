//! Desktop integration other than the clipboard: which app is frontmost,
//! re-activating it, synthesizing the paste keystroke, and the catalog of
//! applications the settings window picks excluded apps from.

use std::time::Duration;

use satchel_core::Millis;

use serde::Serialize;

#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "macos")]
mod macos_apps;
#[cfg(target_os = "macos")]
mod macos_images;
#[cfg(target_os = "macos")]
pub mod macos_tray;
#[cfg(not(target_os = "macos"))]
mod unsupported;

#[cfg(target_os = "macos")]
pub use macos::MacDesktop as SystemDesktop;
#[cfg(target_os = "macos")]
pub use macos_apps::MacApps as SystemApps;
#[cfg(not(target_os = "macos"))]
pub use unsupported::UnsupportedApps as SystemApps;
#[cfg(not(target_os = "macos"))]
pub use unsupported::UnsupportedDesktop as SystemDesktop;

/// Image formats accepted from the web for link previews, by their first
/// bytes: PNG, JPEG, GIF, WebP and ICO. Anything else (SVG, which can carry
/// scripts; TIFF, HEIC, PDF and the other formats ImageIO also reads) is
/// refused before a decoder sees it.
pub fn is_web_image(bytes: &[u8]) -> bool {
    bytes.starts_with(b"\x89PNG\r\n\x1a\n")
        || bytes.starts_with(&[0xFF, 0xD8, 0xFF])
        || bytes.starts_with(b"GIF87a")
        || bytes.starts_with(b"GIF89a")
        || (bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP")
        || bytes.starts_with(&[0, 0, 1, 0])
}

/// A downloaded image as PNG (first frame), decoded by the OS. Other
/// platforms accept PNG only for now.
pub fn web_image_to_png(bytes: &[u8]) -> Option<Vec<u8>> {
    if !is_web_image(bytes) {
        return None;
    }
    #[cfg(target_os = "macos")]
    {
        macos_images::to_png(bytes)
    }
    #[cfg(not(target_os = "macos"))]
    {
        bytes.starts_with(b"\x89PNG").then(|| bytes.to_vec())
    }
}

/// A clip was saved: plays the menu bar icon's animation. Main thread only;
/// a no-op elsewhere and on other platforms for now.
pub fn tray_clip_saved() {
    #[cfg(target_os = "macos")]
    if let Some(mtm) = objc2::MainThreadMarker::new() {
        macos_tray::clip_saved(mtm);
    }
}

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
    /// Opens the OS settings page where that permission is granted (macOS:
    /// Privacy & Security > Accessibility). Unlike the prompt, this works
    /// every time, including when a stale grant keeps the prompt from
    /// showing.
    fn open_keystroke_permission_settings(&self);
    /// Sends the platform paste shortcut (Cmd+V / Ctrl+V) to the frontmost
    /// app.
    fn post_paste_keystroke(&self) -> Result<(), ()>;
}

/// An application the user can exclude from capture. `id` is what the
/// excluded-app list stores and what capture matches (bundle id on macOS).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub id: String,
    /// Localized display name, as Finder shows it.
    pub name: String,
    /// Running right now (listed even if it is not in an Applications folder).
    pub running: bool,
}

/// Runs a closure on the main (UI) thread; false if it could not be queued.
pub type MainThreadRunner = Box<dyn Fn(Box<dyn FnOnce() + Send>) -> bool + Send + Sync>;

/// Installed and running applications, their names and icons. Every method
/// may block (file system, Launch Services, drawing): call it off the main
/// thread.
pub trait AppCatalog: Send + Sync {
    /// App bundles in the usual Applications folders. May contain the same
    /// id more than once (copies in several folders); see `apps::merge`.
    fn installed_apps(&self) -> Vec<AppInfo>;
    /// Regular (Dock) apps running now.
    fn running_apps(&self) -> Vec<AppInfo>;
    /// The installed app with this id, wherever it is, or `None`.
    fn find_app(&self, id: &str) -> Option<AppInfo>;
    /// The app's icon as PNG, `px` pixels square, or `None` when the app is
    /// not installed.
    fn icon_png(&self, id: &str, px: u32) -> Option<Vec<u8>>;
    /// Shows the native "choose an application" dialog and waits for it.
    /// `None` when cancelled, when the choice has no bundle id, or when the
    /// platform has no such dialog.
    fn choose_app(&self) -> Option<AppInfo>;
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
