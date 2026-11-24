//! macOS application catalog for the excluded-apps picker.
//!
//! - Installed apps: the `.app` bundles in the standard Applications folders
//!   (see `app_dirs`), read straight from each bundle's `Info.plist`. A
//!   directory scan is fast (a few hundred small plist reads), needs no
//!   Spotlight index and no run loop, and finds exactly what the user sees in
//!   Finder's Applications folder. Apps elsewhere are still reachable through
//!   the running-apps list and the "Choose…" open panel.
//! - Running apps: `NSWorkspace.runningApplications`, regular (Dock) apps only.
//! - Names: `NSFileManager.displayNameAtPath` (localized, as in Finder).
//! - One id to one app: Launch Services
//!   (`NSWorkspace.URLForApplicationWithBundleIdentifier`).
//! - Icons: `NSWorkspace.iconForFile`, drawn into a small bitmap and encoded
//!   as PNG.
//!
//! Everything except the open panel runs on the calling (worker) thread.

use std::path::{Path, PathBuf};
use std::sync::mpsc;

use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::AnyObject;
use objc2::{AllocAnyThread, MainThreadMarker};
use objc2_app_kit::{
    NSApplicationActivationPolicy, NSBitmapImageFileType, NSBitmapImageRep, NSCompositingOperation,
    NSDeviceRGBColorSpace, NSGraphicsContext, NSImage, NSModalResponseOK, NSOpenPanel, NSWorkspace,
};
use objc2_foundation::{NSArray, NSBundle, NSDictionary, NSFileManager, NSPoint, NSRect, NSSize, NSString, NSURL};

use super::{AppCatalog, AppInfo, MainThreadRunner};
use crate::apps;

/// Folders scanned for app bundles and how deep to look: 2 also covers one
/// level of sub-folders (Utilities, vendor folders such as "Microsoft Office").
fn app_dirs() -> Vec<(PathBuf, u8)> {
    let mut dirs = vec![
        (PathBuf::from("/Applications"), 2),
        (PathBuf::from("/System/Applications"), 2),
        // Keychain Access, Archive Utility and friends since macOS 15.
        (PathBuf::from("/System/Library/CoreServices/Applications"), 1),
    ];
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push((PathBuf::from(home).join("Applications"), 2));
    }
    dirs
}

pub struct MacApps {
    run_on_main: MainThreadRunner,
    own_pid: i32,
}

impl MacApps {
    pub fn new(run_on_main: MainThreadRunner) -> Self {
        Self { run_on_main, own_pid: std::process::id() as i32 }
    }
}

/// Satchel's own bundle id (`None` when run unbundled, e.g. `cargo run`).
pub fn own_bundle_id() -> Option<String> {
    autoreleasepool(|_| NSBundle::mainBundle().bundleIdentifier().map(|s| s.to_string()))
}

fn ns_path(path: &Path) -> Option<Retained<NSString>> {
    path.to_str().map(NSString::from_str)
}

/// Finder's name for the bundle, without the `.app` extension.
fn display_name(path: &Path) -> Option<String> {
    let name = NSFileManager::defaultManager().displayNameAtPath(&*ns_path(path)?).to_string();
    let name = name.strip_suffix(".app").unwrap_or(&name).trim().to_string();
    (!name.is_empty()).then_some(name)
}

/// Id and name of the app bundle at `path`, from its `Info.plist`.
fn bundle_info(path: &Path) -> Option<AppInfo> {
    let plist = ns_path(&path.join("Contents/Info.plist"))?;
    // SAFETY: the dictionary is only read, through `objectForKey`, and its
    // values are type-checked with `downcast` before use. Deprecated only in
    // favour of an error-returning variant; it reads any plist format.
    #[allow(deprecated)]
    let dict = unsafe { NSDictionary::<NSString, AnyObject>::dictionaryWithContentsOfFile(&plist) }?;
    let id = dict
        .objectForKey(&NSString::from_str("CFBundleIdentifier"))?
        .downcast::<NSString>()
        .ok()?
        .to_string();
    let id = id.trim().to_string();
    if id.is_empty() {
        return None;
    }
    let name = display_name(path).unwrap_or_else(|| id.clone());
    Some(AppInfo { id, name, running: false })
}

/// Launch Services' bundle for `id`.
fn app_path(id: &str) -> Option<PathBuf> {
    let url = NSWorkspace::sharedWorkspace().URLForApplicationWithBundleIdentifier(&NSString::from_str(id))?;
    Some(PathBuf::from(url.path()?.to_string()))
}

/// Draws `image` into a `px`-pixel square RGBA bitmap and encodes it as PNG.
fn png_of(image: &NSImage, px: u32) -> Option<Vec<u8>> {
    let side = px as isize;
    // SAFETY: a null planes pointer makes the rep allocate its own buffer;
    // the remaining arguments describe a valid 8-bit RGBA layout (row bytes
    // and bits per pixel 0 = computed). `NSDeviceRGBColorSpace` is an
    // immutable AppKit constant.
    let rep = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            side,
            side,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            0,
            0,
        )
    }?;
    let size = NSSize::new(f64::from(px), f64::from(px));
    rep.setSize(size);
    let context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep)?;
    // The current context is per thread; save and restore it around the draw.
    NSGraphicsContext::saveGraphicsState_class();
    NSGraphicsContext::setCurrentContext(Some(&context));
    image.drawInRect_fromRect_operation_fraction(
        NSRect::new(NSPoint::new(0.0, 0.0), size),
        NSRect::ZERO,
        NSCompositingOperation::SourceOver,
        1.0,
    );
    context.flushGraphics();
    NSGraphicsContext::restoreGraphicsState_class();
    let props = NSDictionary::new();
    // SAFETY: an empty dictionary trivially satisfies the expected
    // `NSDictionary<NSBitmapImageRepPropertyKey, id>` element types.
    let png = unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &props) }?;
    Some(png.to_vec())
}

/// The open panel, restricted to applications. Main thread only.
fn choose_on_main(mtm: MainThreadMarker) -> Option<AppInfo> {
    let panel = NSOpenPanel::openPanel(mtm);
    panel.setCanChooseFiles(true);
    panel.setCanChooseDirectories(false);
    panel.setAllowsMultipleSelection(false);
    panel.setTreatsFilePackagesAsDirectories(false);
    // `allowedContentTypes` would need the UniformTypeIdentifiers bindings
    // (a new crate) for one constant; the extension filter is equivalent for
    // application bundles.
    #[allow(deprecated)]
    panel.setAllowedFileTypes(Some(&NSArray::from_retained_slice(&[NSString::from_str("app")])));
    panel.setDirectoryURL(Some(&NSURL::fileURLWithPath(&NSString::from_str("/Applications"))));
    panel.setPrompt(Some(&NSString::from_str("Exclude")));
    let message = format!("Choose an app whose copies {} should never save.", crate::app_identity::name());
    panel.setMessage(Some(&NSString::from_str(&message)));
    if panel.runModal() != NSModalResponseOK {
        return None;
    }
    let path = panel.URL()?.path()?.to_string();
    bundle_info(Path::new(&path))
}

impl AppCatalog for MacApps {
    fn installed_apps(&self) -> Vec<AppInfo> {
        let mut bundles = Vec::new();
        for (dir, depth) in app_dirs() {
            apps::collect_bundles(&dir, depth, "app", &mut bundles);
        }
        let own = own_bundle_id();
        bundles
            .iter()
            .filter_map(|path| autoreleasepool(|_| bundle_info(path)))
            .filter(|app| own.as_deref().is_none_or(|own| !app.id.eq_ignore_ascii_case(own)))
            .collect()
    }

    fn running_apps(&self) -> Vec<AppInfo> {
        autoreleasepool(|_| {
            NSWorkspace::sharedWorkspace()
                .runningApplications()
                .iter()
                .filter(|app| {
                    app.activationPolicy() == NSApplicationActivationPolicy::Regular
                        && app.processIdentifier() != self.own_pid
                })
                .filter_map(|app| {
                    let id = app.bundleIdentifier()?.to_string();
                    let name = app.localizedName().map(|n| n.to_string()).unwrap_or_else(|| id.clone());
                    Some(AppInfo { id, name, running: true })
                })
                .collect()
        })
    }

    fn find_app(&self, id: &str) -> Option<AppInfo> {
        autoreleasepool(|_| {
            let path = app_path(id)?;
            let name = display_name(&path).unwrap_or_else(|| id.to_string());
            Some(AppInfo { id: id.to_string(), name, running: false })
        })
    }

    fn icon_png(&self, id: &str, px: u32) -> Option<Vec<u8>> {
        autoreleasepool(|_| {
            let path = app_path(id)?;
            let icon = NSWorkspace::sharedWorkspace().iconForFile(&*ns_path(&path)?);
            png_of(&icon, px)
        })
    }

    fn choose_app(&self) -> Option<AppInfo> {
        let (tx, rx) = mpsc::channel();
        let queued = (self.run_on_main)(Box::new(move || {
            let chosen = MainThreadMarker::new().and_then(|mtm| autoreleasepool(|_| choose_on_main(mtm)));
            let _ = tx.send(chosen);
        }));
        if !queued {
            return None;
        }
        // Dropped without running (app shutting down) also ends the wait.
        rx.recv().ok().flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> MacApps {
        MacApps::new(Box::new(|_| false))
    }

    #[test]
    fn finds_system_apps_with_names() {
        let apps = catalog().installed_apps();
        let calc = apps.iter().find(|a| a.id == "com.apple.calculator").expect("Calculator is installed");
        assert_eq!(calc.name, "Calculator");
        assert!(!calc.running);
    }

    #[test]
    fn resolves_an_id_through_launch_services() {
        let app = catalog().find_app("com.apple.TextEdit").expect("TextEdit is installed");
        assert_eq!(app.name, "TextEdit");
        assert!(catalog().find_app("com.example.not-installed-anywhere").is_none());
    }

    #[test]
    fn renders_icons_as_png_of_the_requested_size() {
        let png = catalog().icon_png("com.apple.TextEdit", 64).expect("icon");
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        let decoder = png::Decoder::new(std::io::Cursor::new(png));
        let info = decoder.read_info().unwrap().info().clone();
        assert_eq!((info.width, info.height), (64, 64));
        assert!(catalog().icon_png("com.example.not-installed-anywhere", 64).is_none());
    }

    #[test]
    fn choosing_without_a_main_thread_gives_nothing() {
        assert!(catalog().choose_app().is_none());
    }
}
