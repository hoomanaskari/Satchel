//! macOS integration: NSWorkspace for the frontmost app, CGEvent for the
//! paste keystroke (gated by the Accessibility permission), a
//! non-activating NSPanel for the shelf, its Liquid Glass background
//! (macOS 26+) and an NSAlert for confirmations.

use objc2::rc::autoreleasepool;
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{ClassType, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSAutoresizingMaskOptions, NSAlertStyle, NSApplication, NSApplicationActivationOptions,
    NSGlassEffectView, NSGlassEffectViewStyle, NSPanel, NSResponder, NSRunningApplication, NSStatusWindowLevel, NSView,
    NSWindow, NSWindowCollectionBehavior, NSWindowOrderingMode, NSWindowStyleMask, NSWorkspace,
};
use objc2_core_graphics::{
    CGEvent, CGEventFlags, CGEventSource, CGEventSourceStateID, CGEventTapLocation,
    CGPreflightPostEventAccess, CGRequestPostEventAccess,
};
use objc2_foundation::{NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString, NSURL};

use super::{Desktop, FrontApp};

/// `kVK_ANSI_V`. Layout-independent position of the V key on ANSI/ISO
/// keyboards (see the report: non-QWERTY layouts are a known gap).
const KEY_V: u16 = 9;

/// System Settings > Privacy & Security > Accessibility.
const ACCESSIBILITY_SETTINGS_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility";

pub struct MacDesktop {
    own_pid: i32,
}

impl MacDesktop {
    pub fn new() -> Self {
        Self { own_pid: std::process::id() as i32 }
    }
}

impl Desktop for MacDesktop {
    fn frontmost_app(&self) -> Option<FrontApp> {
        autoreleasepool(|_| {
            let app = NSWorkspace::sharedWorkspace().frontmostApplication()?;
            let pid = app.processIdentifier();
            if pid <= 0 || pid == self.own_pid {
                return None;
            }
            let name = app.localizedName().map(|n| n.to_string()).unwrap_or_default();
            let id = app.bundleIdentifier().map(|b| b.to_string()).unwrap_or_else(|| name.clone());
            Some(FrontApp { pid, id, name })
        })
    }

    fn activate_app(&self, pid: i32) -> bool {
        autoreleasepool(|_| {
            NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
                .is_some_and(|app| app.activateWithOptions(NSApplicationActivationOptions::empty()))
        })
    }

    fn can_post_keystrokes(&self) -> bool {
        // SAFETY: AXIsProcessTrusted takes no arguments, returns a Boolean
        // and is safe to call from any thread.
        let accessibility = unsafe { AXIsProcessTrusted() } != 0;
        keystrokes_allowed(accessibility, || CGPreflightPostEventAccess())
    }

    fn request_keystroke_permission(&self) {
        // Shows the system prompt that links to Privacy & Security >
        // Accessibility. It returns immediately.
        let _ = CGRequestPostEventAccess();
    }

    fn open_keystroke_permission_settings(&self) {
        autoreleasepool(|_| {
            if let Some(url) = NSURL::URLWithString(&NSString::from_str(ACCESSIBILITY_SETTINGS_URL)) {
                NSWorkspace::sharedWorkspace().openURL(&url);
            }
        });
    }

    fn post_paste_keystroke(&self) -> Result<(), ()> {
        let source = CGEventSource::new(CGEventSourceStateID::CombinedSessionState);
        let down = CGEvent::new_keyboard_event(source.as_deref(), KEY_V, true).ok_or(())?;
        let up = CGEvent::new_keyboard_event(source.as_deref(), KEY_V, false).ok_or(())?;
        // Exactly Command: a Shift still held from Shift+Return must not turn
        // this into Cmd+Shift+V.
        CGEvent::set_flags(Some(&down), CGEventFlags::MaskCommand);
        CGEvent::set_flags(Some(&up), CGEventFlags::MaskCommand);
        CGEvent::post(CGEventTapLocation::SessionEventTap, Some(&down));
        CGEvent::post(CGEventTapLocation::SessionEventTap, Some(&up));
        Ok(())
    }
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    /// Whether this process holds the Accessibility permission. Unlike
    /// `CGPreflightPostEventAccess`, it reflects a grant made while the
    /// process is running.
    fn AXIsProcessTrusted() -> u8;
}

/// Synthesized keystrokes are delivered when the process holds the
/// Accessibility permission. The post-event check is consulted only when
/// that one says no: its answer is fixed at launch, so on its own it keeps
/// reporting "denied" after the user grants access.
fn keystrokes_allowed(accessibility: bool, post_event_access: impl FnOnce() -> bool) -> bool {
    accessibility || post_event_access()
}

define_class!(
    /// The shelf's window class after conversion: an NSPanel that may become
    /// key (to receive typing) without activating Satchel.
    #[unsafe(super(NSPanel, NSWindow, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "SatchelShelfPanel"]
    struct ShelfPanel;

    impl ShelfPanel {
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key_window(&self) -> bool {
            true
        }

        #[unsafe(method(canBecomeMainWindow))]
        fn can_become_main_window(&self) -> bool {
            false
        }
    }
);

fn descends_from(class: &AnyClass, ancestor: &AnyClass) -> bool {
    let mut current = Some(class);
    while let Some(c) = current {
        if c == ancestor {
            return true;
        }
        current = c.superclass();
    }
    false
}

/// Turns the shelf's NSWindow into a non-activating floating panel that joins
/// every Space and shows over full-screen apps. Returns false (leaving the
/// window untouched) when the conversion is not known to be safe.
pub fn make_shelf_panel(_mtm: MainThreadMarker, ns_window: *mut std::ffi::c_void) -> bool {
    if ns_window.is_null() {
        return false;
    }
    let object = ns_window.cast::<AnyObject>();
    // SAFETY: `ns_window` is the live NSWindow of a Tauri window, obtained on
    // the main thread; Tauri keeps it alive for the life of the app.
    let current: &AnyClass = unsafe { (*object).class() };
    if !descends_from(current, NSWindow::class()) {
        return false;
    }
    // Swapping the isa pointer is only sound when the new class does not need
    // more instance storage than NSWindow already reserves: NSPanel must add
    // no ivars, and ShelfPanel adds none. Verified at run time, not assumed.
    if NSPanel::class().instance_size() != NSWindow::class().instance_size()
        || ShelfPanel::class().instance_size() != NSPanel::class().instance_size()
    {
        return false;
    }
    // SAFETY: same instance size (checked above) and ShelfPanel descends from
    // NSWindow, so every NSWindow method and tao's ivars (which live after
    // NSWindow's) stay where they were. tauri-nspanel uses the same technique.
    unsafe {
        objc2::ffi::object_setClass(object, ShelfPanel::class());
    }
    // SAFETY: the object is now a ShelfPanel (an NSPanel) and we are on the
    // main thread.
    let panel: &NSPanel = unsafe { &*object.cast::<NSPanel>() };
    panel.setStyleMask(panel.styleMask() | NSWindowStyleMask::NonactivatingPanel);
    // AppKit only tells the window server about "non-activating" when the
    // style is given at creation; adding it afterwards leaves clicks on the
    // panel activating Satchel, which takes focus from the app being pasted
    // into. This is the setter AppKit itself uses at creation.
    let prevents_activation = objc2::sel!(_setPreventsActivation:);
    if panel.respondsToSelector(prevents_activation) {
        // SAFETY: the selector exists on this object (checked above) and
        // takes one BOOL; we are on the main thread.
        let _: () = unsafe { msg_send![panel, _setPreventsActivation: true] };
    }
    panel.setFloatingPanel(true);
    panel.setBecomesKeyOnlyIfNeeded(false);
    panel.setHidesOnDeactivate(false);
    panel.setLevel(NSStatusWindowLevel);
    panel.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::Stationary
            | NSWindowCollectionBehavior::IgnoresCycle,
    );
    true
}

/// Gives keyboard focus to the app `pid` when Satchel is the active app
/// (the shelf was clicked on a system where the panel still activates, or a
/// Satchel window was in front). macOS 14+ refuses a plain activation of
/// another app unless the active app yields to it first.
pub fn hand_focus_to(mtm: MainThreadMarker, pid: i32) {
    let app = NSApplication::sharedApplication(mtm);
    if !app.isActive() {
        return;
    }
    let Some(target) = NSRunningApplication::runningApplicationWithProcessIdentifier(pid) else {
        return;
    };
    app.yieldActivationToApplication(&target);
    target.activateWithOptions(NSApplicationActivationOptions::empty());
}

/// Shows the panel and makes it key without activating Satchel, then gives
/// keyboard focus to the web view.
pub fn show_panel(_mtm: MainThreadMarker, ns_window: *mut std::ffi::c_void, webview: *mut std::ffi::c_void) {
    if ns_window.is_null() {
        return;
    }
    // SAFETY: called on the main thread with the shelf's live NSWindow
    // pointer (see `make_shelf_panel`).
    let window: &NSWindow = unsafe { &*ns_window.cast::<NSWindow>() };
    window.orderFrontRegardless();
    window.makeKeyWindow();
    if !webview.is_null() {
        // SAFETY: `webview` is the shelf's live WKWebView (an NSResponder),
        // owned by the window above.
        let _: bool = unsafe { msg_send![window, makeFirstResponder: webview.cast::<AnyObject>()] };
    }
}

/// Tag window-vibrancy (used by Tauri's window effects) gives its
/// NSVisualEffectView (`NS_VIEW_TAG_BLUR_VIEW` in window-vibrancy 0.8).
const VIBRANCY_VIEW_TAG: isize = 91_376_254;

/// Frame of the UI's panel inside a content view of `bounds`: `inset`
/// points from the left, right and bottom edges and `top` points from the
/// top (`shelf::PANEL_INSET` / `PANEL_TOP`, the CSS panel's margins).
fn panel_frame(bounds: NSRect, flipped: bool, inset: f64, top: f64) -> NSRect {
    let width = (bounds.size.width - 2.0 * inset).max(1.0);
    let height = (bounds.size.height - inset - top).max(1.0);
    let y = if flipped { top } else { inset };
    NSRect::new(NSPoint::new(inset, y), NSSize::new(width, height))
}

/// Pins `view` to the panel frame; autoresizing keeps the margins when the
/// window is resized (the shelf is resized to each screen it is shown on).
fn fit_to_panel(content: &NSView, view: &NSView, inset: f64, top: f64) {
    view.setFrame(panel_frame(content.bounds(), content.isFlipped(), inset, top));
    view.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
    );
}

/// Shrinks the effect view to the UI's panel: `inset` points from the left,
/// right and bottom edges, `top` from the top. Autoresizing keeps those
/// margins when the window is resized. Returns false if there is no effect
/// view (effects unsupported or not applied yet).
pub fn inset_effect_view(_mtm: MainThreadMarker, ns_window: *mut std::ffi::c_void, inset: f64, top: f64) -> bool {
    if ns_window.is_null() {
        return false;
    }
    // SAFETY: main thread, live NSWindow pointer of the shelf.
    let window: &NSWindow = unsafe { &*ns_window.cast::<NSWindow>() };
    let Some(content) = window.contentView() else { return false };
    let Some(effect) = content.viewWithTag(VIBRANCY_VIEW_TAG) else { return false };
    fit_to_panel(&content, &effect, inset, top);
    true
}

/// Whether this macOS has Liquid Glass (`NSGlassEffectView`, macOS 26+).
/// The class is looked up at run time, so the binary still launches on
/// macOS 11–15, where the shelf keeps the NSVisualEffectView blur.
/// `SATCHEL_NO_GLASS` forces the fallback (to check it on a new system).
pub fn liquid_glass_available() -> bool {
    std::env::var_os("SATCHEL_NO_GLASS").is_none() && AnyClass::get(c"NSGlassEffectView").is_some()
}

/// Puts a Liquid Glass view behind the web view, framed to the UI's panel
/// (`inset` from the left, right and bottom, `top` from the top) with the
/// panel's corner radius. Unlike window-vibrancy's glass, the web view is not
/// moved into it: the glass is a sibling below it in the content view, so it
/// can be inset to the panel while the web view keeps the whole window (the
/// panel's shadow is drawn there). Call once, at window creation. Returns
/// false (installing nothing) when glass is unavailable.
pub fn install_liquid_glass(
    mtm: MainThreadMarker,
    ns_window: *mut std::ffi::c_void,
    inset: f64,
    top: f64,
    radius: f64,
) -> bool {
    if ns_window.is_null() || !liquid_glass_available() {
        return false;
    }
    // SAFETY: main thread, live NSWindow pointer of the shelf.
    let window: &NSWindow = unsafe { &*ns_window.cast::<NSWindow>() };
    let Some(content) = window.contentView() else { return false };
    // While its window is key (always, when the shelf is up) the glass
    // casts a soft shadow of its own, reaching some 25-40 pt past its edge:
    // more than the window's margins, so the window edge would cut it off in
    // a hard line. The glass sits in a container clipped to the panel's
    // rounded shape, which drops that shadow and leaves the material and its
    // rim as they are; the panel's CSS shadow, sized to the margins, is the
    // only one.
    let clip = NSView::initWithFrame(mtm.alloc(), content.bounds());
    clip.setWantsLayer(true);
    // SAFETY: `setWantsLayer(true)` gave the view a backing CALayer; these
    // are its documented `cornerRadius` (CGFloat) and `masksToBounds` (BOOL)
    // setters, called on the main thread. The default corner curve is
    // circular, like the CSS panel's border-radius.
    unsafe {
        let layer: Option<objc2::rc::Retained<AnyObject>> = msg_send![&*clip, layer];
        let Some(layer) = layer else { return false };
        let _: () = msg_send![&*layer, setCornerRadius: radius];
        let _: () = msg_send![&*layer, setMasksToBounds: true];
    }
    fit_to_panel(&content, &clip, inset, top);
    // The class exists (checked above), so objc2's class lookup cannot fail.
    let glass = NSGlassEffectView::initWithFrame(mtm.alloc(), clip.bounds());
    glass.setStyle(NSGlassEffectViewStyle::Regular);
    glass.setCornerRadius(radius);
    glass.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    clip.addSubview(&glass);
    content.addSubview_positioned_relativeTo(&clip, NSWindowOrderingMode::Below, None);
    true
}

/// Whether Satchel is the active app (owns the menu bar and key window).
pub fn app_is_active(mtm: MainThreadMarker) -> bool {
    NSApplication::sharedApplication(mtm).isActive()
}

/// Gives up being the active app; the system activates the next one.
pub fn deactivate(mtm: MainThreadMarker) {
    NSApplication::sharedApplication(mtm).deactivate();
}

/// Modal confirmation. Satchel is an accessory app, so it activates itself
/// first or the alert would open behind other windows.
pub fn confirm(mtm: MainThreadMarker, message: &str, informative: &str, confirm_label: &str) -> bool {
    let app = NSApplication::sharedApplication(mtm);
    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);
    let alert = NSAlert::new(mtm);
    alert.setAlertStyle(NSAlertStyle::Warning);
    alert.setMessageText(&NSString::from_str(message));
    alert.setInformativeText(&NSString::from_str(informative));
    alert.addButtonWithTitle(&NSString::from_str(confirm_label));
    alert.addButtonWithTitle(&NSString::from_str("Cancel"));
    alert.runModal() == NSAlertFirstButtonReturn
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_frame_leaves_the_shadow_margins_on_every_side() {
        let bounds = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1000.0, 364.0));
        // AppKit's default, y up: the bottom margin is at y = 0.
        let up = panel_frame(bounds, false, 12.0, 24.0);
        assert_eq!((up.origin.x, up.origin.y, up.size.width, up.size.height), (12.0, 12.0, 976.0, 328.0));
        // Flipped (y down): the top margin is at y = 0.
        let down = panel_frame(bounds, true, 12.0, 24.0);
        assert_eq!((down.origin.x, down.origin.y, down.size.width, down.size.height), (12.0, 24.0, 976.0, 328.0));
        // A window too small for its margins never gets an empty view.
        let tiny = panel_frame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(10.0, 20.0)), false, 12.0, 24.0);
        assert_eq!((tiny.size.width, tiny.size.height), (1.0, 1.0));
    }

    #[test]
    fn panel_class_has_window_layout() {
        // The run-time guard in `make_shelf_panel` must hold on this OS, or
        // the shelf silently falls back to a normal window.
        assert_eq!(NSPanel::class().instance_size(), NSWindow::class().instance_size());
        assert_eq!(ShelfPanel::class().instance_size(), NSPanel::class().instance_size());
    }

    #[test]
    fn frontmost_app_is_never_ourselves() {
        let desktop = MacDesktop::new();
        if let Some(app) = desktop.frontmost_app() {
            assert_ne!(app.pid, std::process::id() as i32);
        }
    }


    #[test]
    fn accessibility_grant_is_enough_even_when_the_post_event_check_is_stale() {
        assert!(super::keystrokes_allowed(true, || false));
    }

    #[test]
    fn post_event_access_alone_is_enough() {
        assert!(super::keystrokes_allowed(false, || true));
    }

    #[test]
    fn neither_permission_means_no_keystrokes() {
        assert!(!super::keystrokes_allowed(false, || false));
    }

    #[test]
    fn post_event_check_is_skipped_once_accessibility_is_granted() {
        assert!(super::keystrokes_allowed(true, || panic!("must not be consulted")));
    }
}
