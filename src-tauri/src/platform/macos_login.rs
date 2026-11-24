//! Open at login: `SMAppService.mainAppService` (ServiceManagement, macOS
//! 13+), which lists this app bundle under System Settings › General ›
//! Login Items › Open at Login. macOS launches it at login through Launch
//! Services, like opening it from Finder.
//!
//! Called by message, not through a generated binding crate: four methods
//! on one class, looked up at run time, so macOS 11 and 12 (no
//! `SMAppService`) simply report it unavailable. Nothing else in the crate
//! links ServiceManagement.

use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{AnyClass, AnyObject};
use objc2::msg_send;
use objc2_foundation::{NSBundle, NSError};

use crate::login_item::Status;

#[link(name = "ServiceManagement", kind = "framework")]
unsafe extern "C" {}

/// `SMAppService.mainAppService`, or `None` before macOS 13.
fn main_app() -> Option<Retained<AnyObject>> {
    let class = AnyClass::get(c"SMAppService")?;
    // SAFETY: class method of SMAppService taking no arguments, returning an
    // autoreleased SMAppService.
    unsafe { msg_send![class, mainAppService] }
}

/// Whether this macOS has `SMAppService` (13+).
pub fn supported() -> bool {
    AnyClass::get(c"SMAppService").is_some()
}

/// Where the login item stands for this bundle.
pub fn status() -> Option<Status> {
    autoreleasepool(|_| {
        let service = main_app()?;
        // SAFETY: `status` is an NSInteger property (SMAppServiceStatus).
        let raw: isize = unsafe { msg_send![&*service, status] };
        Some(match raw {
            1 => Status::Enabled,
            2 => Status::RequiresApproval,
            3 => Status::NotFound,
            _ => Status::NotRegistered,
        })
    })
}

/// Registers (on) or unregisters (off) this bundle as a login item.
pub fn set(on: bool) -> Result<(), ()> {
    autoreleasepool(|_| {
        let service = main_app().ok_or(())?;
        // SAFETY: both methods take an `NSError **` and return BOOL; objc2
        // turns that into a Result.
        let result: Result<(), Retained<NSError>> = if on {
            unsafe { msg_send![&*service, registerAndReturnError: _] }
        } else {
            unsafe { msg_send![&*service, unregisterAndReturnError: _] }
        };
        result.map_err(|_| ())
    })
}

/// The path of the running app bundle (`/Applications/Satchel.app`).
pub fn bundle_path() -> Option<String> {
    autoreleasepool(|_| {
        let path = NSBundle::mainBundle().bundlePath().to_string();
        path.ends_with(".app").then_some(path)
    })
}
