//! macOS side of `instance`: other running copies of this app, the request
//! that asks the first copy to show its shelf, and the app that was active
//! before Satchel.
//!
//! - Other copies: `NSRunningApplication` by bundle identifier. Launch
//!   Services normally keeps one copy per identifier, but `open -n` (which
//!   the updater's relaunch uses) and a second copy of the bundle elsewhere
//!   on disk each start another process.
//! - The request: a distributed notification named after the bundle
//!   identifier, posted with `deliverImmediately` so it also reaches a copy
//!   that is not the active app. It carries nothing; any local process can
//!   post it, and all it can do is what the shortcut does.
//! - The previous app: `NSWorkspaceDidActivateApplicationNotification`. When
//!   the user opens Satchel again from Finder or Spotlight, macOS activates
//!   Satchel before telling it, so "the frontmost app" is Satchel itself;
//!   this remembers the one before, to paste into and to give focus back to.

use std::ptr::NonNull;
use std::sync::atomic::{AtomicI32, Ordering};

use block2::RcBlock;
use objc2::rc::autoreleasepool;
use objc2_app_kit::{NSRunningApplication, NSWorkspace, NSWorkspaceDidActivateApplicationNotification};
use objc2_foundation::{NSDistributedNotificationCenter, NSNotification, NSString};

use crate::instance::Instance;

/// Every other running process with this bundle identifier, with its launch
/// time. Empty for an unbundled binary (no identifier to match).
pub fn other_instances(bundle_id: &str) -> Vec<Instance> {
    let own = std::process::id() as i32;
    autoreleasepool(|_| {
        NSRunningApplication::runningApplicationsWithBundleIdentifier(&NSString::from_str(bundle_id))
            .iter()
            .filter(|app| !app.isTerminated() && app.processIdentifier() != own && app.processIdentifier() > 0)
            .map(|app| Instance {
                pid: app.processIdentifier(),
                launched: app.launchDate().map_or(0.0, |d| d.timeIntervalSince1970()),
            })
            .collect()
    })
}

/// This process, as `other_instances` describes the others.
pub fn this_instance() -> Instance {
    let pid = std::process::id() as i32;
    let launched = autoreleasepool(|_| {
        NSRunningApplication::currentApplication().launchDate().map_or(f64::MAX, |d| d.timeIntervalSince1970())
    });
    Instance { pid, launched }
}

/// Whether the process is still running. Asks Launch Services afresh each
/// time (a kept `NSRunningApplication` only updates on the main run loop,
/// which is blocked while this is polled).
pub fn is_running(pid: i32) -> bool {
    autoreleasepool(|_| {
        NSRunningApplication::runningApplicationWithProcessIdentifier(pid).is_some_and(|app| !app.isTerminated())
    })
}

/// Asks the copy that is already running to show its shelf.
pub fn post_show_request(name: &str) {
    autoreleasepool(|_| {
        let center = NSDistributedNotificationCenter::defaultCenter();
        // SAFETY: no user info.
        unsafe {
            center.postNotificationName_object_userInfo_deliverImmediately(&NSString::from_str(name), None, None, true)
        };
    });
}

/// Calls `on_request` for every show request another copy posts. For the
/// life of the process. Main thread (the observer is called on the thread
/// that registered it).
pub fn observe_show_requests(name: &str, on_request: impl Fn() + 'static) {
    let block = RcBlock::new(move |_: NonNull<NSNotification>| on_request());
    let center = NSDistributedNotificationCenter::defaultCenter();
    // SAFETY: no object filter or queue; the block lives as long as the
    // observer, which is kept for the life of the process.
    let observer = unsafe { center.addObserverForName_object_queue_usingBlock(Some(&NSString::from_str(name)), None, None, &block) };
    std::mem::forget(observer);
}

/// Pid of the last app other than Satchel that became active.
static LAST_OTHER_APP: AtomicI32 = AtomicI32::new(0);

fn note_frontmost(own: i32) {
    let pid = autoreleasepool(|_| NSWorkspace::sharedWorkspace().frontmostApplication().map(|a| a.processIdentifier()));
    if let Some(pid) = pid
        && pid > 0
        && pid != own
    {
        LAST_OTHER_APP.store(pid, Ordering::Relaxed);
    }
}

/// Starts remembering the last app other than Satchel that was active.
/// Main thread, once.
pub fn track_active_apps() {
    let own = std::process::id() as i32;
    note_frontmost(own);
    let block = RcBlock::new(move |_: NonNull<NSNotification>| note_frontmost(own));
    let center = NSWorkspace::sharedWorkspace().notificationCenter();
    // SAFETY: as in `observe_show_requests`.
    let observer = unsafe {
        center.addObserverForName_object_queue_usingBlock(Some(NSWorkspaceDidActivateApplicationNotification), None, None, &block)
    };
    std::mem::forget(observer);
}

/// The last app other than Satchel that was active, if it still runs.
pub fn last_other_active_app() -> Option<i32> {
    let pid = LAST_OTHER_APP.load(Ordering::Relaxed);
    (pid > 0 && is_running(pid)).then_some(pid)
}
