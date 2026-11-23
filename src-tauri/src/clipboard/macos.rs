//! NSPasteboard implementation.
//!
//! The watcher keeps one `NSPasteboard` handle and polls `changeCount`, a
//! plain integer property: no allocation, no autorelease traffic when nothing
//! changed. Types and data are only fetched after a change, inside an
//! autorelease pool.
//!
//! What is kept from one change (see `plan` and `read_payload`):
//! - Files: `public.file-url` items whose files exist (Finder, most apps).
//!   URLs to files that do not exist (promises, already-deleted temporary
//!   files) do not count, and the change falls through to the image or text.
//!   Copying an image file in Finder is a File clip: the file itself is what
//!   pastes into Finder, Mail or a chat, and the file stays where it is.
//!   Finder adds only an icon (TIFF/ICNS) next to file URLs; an app that puts
//!   the picture itself (PNG, JPEG, HEIC...) next to the URL of its cached
//!   copy (Messages, Photos, chat apps) copied an image, and the image wins.
//! - Text: `public.utf8-plain-text`, with its `public.html` and `public.rtf`
//!   flavors when present.
//! - Images: PNG as is; TIFF, JPEG, HEIC/HEIF, WebP, GIF and BMP converted to
//!   PNG by ImageIO (via NSBitmapImageRep). Text wins over a picture
//!   rendition, except a browser's "Copy Image" (`image_wins_over_text`).
//! - Colors: an NSColor (`com.apple.cocoa.pasteboard.color`, from color wells
//!   and pickers) with no text is kept as its sRGB `#RRGGBB(AA)`.

use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{
    NSBitmapImageFileType, NSBitmapImageRep, NSColor, NSColorSpace, NSPasteboard, NSPasteboardItem,
    NSPasteboardType, NSPasteboardTypeFileURL, NSPasteboardTypeHTML, NSPasteboardTypePNG,
    NSPasteboardTypeRTF, NSPasteboardTypeString, NSPasteboardTypeTIFF, NSPasteboardWriting,
};
use objc2_foundation::{NSArray, NSData, NSDictionary, NSString, NSURL};
use satchel_core::ClipKind;

use super::{
    ClipboardSink, ClipboardSource, MAX_FILES, MAX_PATH_BYTES, Payload, ReadLimits,
    SELF_MARKER_TYPE, SOURCE_MARKER_TYPE, WriteRequest, image_wins_over_text,
};
use crate::thumbnail::MAX_PIXELS;

pub const BUNDLE_ID: &str = "app.satchel.desktop";
/// TIFF and BMP are uncompressed; accept a larger raw size before converting
/// to PNG (the PNG is checked against the cap again).
const UNCOMPRESSED_FACTOR: u64 = 4;
/// Satchel also writes a TIFF of a pasted image for apps that only read TIFF,
/// but only up to this PNG size: the TIFF is uncompressed and built eagerly.
const MAX_PNG_FOR_TIFF_COPY: usize = 16 * 1024 * 1024;

const UTI_FILE_URL: &str = "public.file-url";
const UTI_TEXT: &str = "public.utf8-plain-text";
const UTI_COLOR: &str = "com.apple.cocoa.pasteboard.color";
/// File icons, as Finder adds them next to copied files.
const UTI_ICNS: &str = "com.apple.icns";

/// Image types read, in order of preference. PNG is stored as is; the rest
/// are converted. `uncompressed` raises the raw-size cap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageType {
    pub uti: &'static str,
    pub uncompressed: bool,
}

pub const IMAGE_TYPES: &[ImageType] = &[
    ImageType { uti: "public.png", uncompressed: false },
    ImageType { uti: "public.tiff", uncompressed: true },
    ImageType { uti: "public.jpeg", uncompressed: false },
    ImageType { uti: "public.heic", uncompressed: false },
    ImageType { uti: "public.heif", uncompressed: false },
    ImageType { uti: "org.webmproject.webp", uncompressed: false },
    ImageType { uti: "com.compuserve.gif", uncompressed: false },
    ImageType { uti: "com.microsoft.bmp", uncompressed: true },
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plan {
    /// File URLs first; if none exists, fall back to `then`.
    Files { then: Option<Fallback> },
    /// Image data next to file URLs (not a Finder icon): the image, or the
    /// files if the image cannot be read.
    ImageOrFiles(ImageType),
    /// Text, unless the image is what was copied (`image` is the best image
    /// type present, if any).
    Text { image: Option<ImageType> },
    Image(ImageType),
    Color,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fallback {
    Text { image: Option<ImageType> },
    Image(ImageType),
}

/// The best image type present, when images are captured.
pub fn best_image(types: &[String], images: bool) -> Option<ImageType> {
    if !images {
        return None;
    }
    IMAGE_TYPES.iter().copied().find(|it| types.iter().any(|t| t == it.uti))
}

/// Decides what to read from the type list alone.
pub fn plan(types: &[String], images: bool) -> Option<Plan> {
    let has = |t: &str| types.iter().any(|x| x == t);
    let image = best_image(types, images);
    let after_files = if has(UTI_TEXT) {
        Some(Fallback::Text { image })
    } else {
        image.map(Fallback::Image)
    };
    if has(UTI_FILE_URL) {
        match image {
            // Finder writes icons as TIFF (and ICNS); any other image type
            // is content.
            Some(it) if it.uti != "public.tiff" && !has(UTI_ICNS) => Some(Plan::ImageOrFiles(it)),
            _ => Some(Plan::Files { then: after_files }),
        }
    } else if let Some(f) = after_files {
        Some(match f {
            Fallback::Text { image } => Plan::Text { image },
            Fallback::Image(it) => Plan::Image(it),
        })
    } else if has(UTI_COLOR) {
        Some(Plan::Color)
    } else {
        None
    }
}

fn data(pb: &NSPasteboard, ty: &NSPasteboardType, max: u64) -> Option<Vec<u8>> {
    let data = pb.dataForType(ty)?;
    if data.len() as u64 > max || data.is_empty() {
        return None;
    }
    Some(data.to_vec())
}

fn utf8(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

/// Encodes a decoded bitmap as PNG, refusing absurd pixel counts before any
/// pixel is decoded.
fn rep_to_png(rep: &NSBitmapImageRep) -> Option<Vec<u8>> {
    let (w, h) = (rep.pixelsWide(), rep.pixelsHigh());
    if w <= 0 || h <= 0 || (w as u64) * (h as u64) > MAX_PIXELS {
        return None;
    }
    let props = NSDictionary::new();
    // SAFETY: an empty dictionary trivially satisfies the expected
    // `NSDictionary<NSBitmapImageRepPropertyKey, id>` element types.
    let png = unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &props) }?;
    Some(png.to_vec())
}

/// Any ImageIO-readable image (TIFF, JPEG, HEIC, WebP, GIF, BMP...) as PNG.
#[cfg(test)]
fn image_to_png(bytes: &[u8]) -> Option<Vec<u8>> {
    rep_to_png(&*NSBitmapImageRep::imageRepWithData(&NSData::with_bytes(bytes))?)
}

fn png_to_tiff(png: &[u8]) -> Option<Retained<NSData>> {
    NSBitmapImageRep::imageRepWithData(&NSData::with_bytes(png))?.TIFFRepresentation()
}

/// Paths of the copied file URLs that exist on disk.
fn file_paths(pb: &NSPasteboard) -> Vec<String> {
    let Some(items) = pb.pasteboardItems() else { return Vec::new() };
    let mut paths = Vec::new();
    for item in items.iter().take(MAX_FILES) {
        // SAFETY: the static is a valid, immutable NSString provided by AppKit.
        let Some(url) = item.stringForType(unsafe { NSPasteboardTypeFileURL }) else { continue };
        let Some(url) = NSURL::URLWithString(&url) else { continue };
        if !url.isFileURL() {
            continue;
        }
        // `path` resolves file reference URLs (`file:///.file/id=...`) too.
        if let Some(path) = url.path() {
            let path = path.to_string();
            if !path.is_empty() && path.len() <= MAX_PATH_BYTES && std::path::Path::new(&path).exists()
            {
                paths.push(path);
            }
        }
    }
    paths
}

fn read_image(pb: &NSPasteboard, it: ImageType, limits: &ReadLimits) -> Option<Payload> {
    let too_large = || Payload::TooLarge { kind: ClipKind::Image, limit_bytes: limits.max_image_bytes };
    let ty = NSString::from_str(it.uti);
    let data = pb.dataForType(&ty)?;
    if data.is_empty() {
        return None;
    }
    let raw_cap = if it.uncompressed {
        limits.max_image_bytes.saturating_mul(UNCOMPRESSED_FACTOR)
    } else {
        limits.max_image_bytes
    };
    if data.len() as u64 > raw_cap {
        return Some(too_large());
    }
    let png = if it.uti == "public.png" {
        let png = data.to_vec();
        if crate::thumbnail::png_dimensions(&png).is_some_and(|(w, h)| w as u64 * h as u64 > MAX_PIXELS) {
            return Some(too_large());
        }
        png
    } else {
        let rep = NSBitmapImageRep::imageRepWithData(&data)?;
        let (w, h) = (rep.pixelsWide(), rep.pixelsHigh());
        if (w.max(0) as u64) * (h.max(0) as u64) > MAX_PIXELS {
            return Some(too_large());
        }
        rep_to_png(&rep)?
    };
    if png.len() as u64 > limits.max_image_bytes {
        return Some(too_large());
    }
    Some(Payload::Image { png })
}

fn read_text(pb: &NSPasteboard, types: &[String], image: Option<ImageType>, limits: &ReadLimits) -> Option<Payload> {
    // SAFETY (constants below): immutable AppKit NSString constants.
    let raw = pb.dataForType(unsafe { NSPasteboardTypeString })?;
    if raw.len() as u64 > limits.max_text_bytes {
        return Some(Payload::TooLarge { kind: ClipKind::Text, limit_bytes: limits.max_text_bytes });
    }
    let text = utf8(raw.to_vec());
    let html = data(pb, unsafe { NSPasteboardTypeHTML }, limits.max_text_bytes).map(utf8);
    if let Some(it) = image
        && image_wins_over_text(types, &text, html.as_deref())
        && let Some(img) = read_image(pb, it, limits)
    {
        return Some(img);
    }
    let rtf = data(pb, unsafe { NSPasteboardTypeRTF }, limits.max_text_bytes);
    Some(Payload::Text { text, html, rtf })
}

/// `#RRGGBB`, or `#RRGGBBAA` when not opaque, of an sRGB color.
pub fn hex_color(r: f64, g: f64, b: f64, a: f64) -> String {
    let c = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    if c(a) == 255 {
        format!("#{:02X}{:02X}{:02X}", c(r), c(g), c(b))
    } else {
        format!("#{:02X}{:02X}{:02X}{:02X}", c(r), c(g), c(b), c(a))
    }
}

fn read_color(pb: &NSPasteboard) -> Option<Payload> {
    let color = NSColor::colorFromPasteboard(pb)?;
    let srgb = color.colorUsingColorSpace(&NSColorSpace::sRGBColorSpace())?;
    let text = hex_color(
        srgb.redComponent(),
        srgb.greenComponent(),
        srgb.blueComponent(),
        srgb.alphaComponent(),
    );
    Some(Payload::Text { text, html: None, rtf: None })
}

fn read_payload(pb: &NSPasteboard, limits: &ReadLimits) -> Option<Payload> {
    let types: Vec<String> = pb
        .types()
        .map(|t| t.iter().map(|s| s.to_string()).collect())
        .unwrap_or_default();
    let fallback = |f: Fallback| match f {
        Fallback::Text { image } => read_text(pb, &types, image, limits),
        Fallback::Image(it) => read_image(pb, it, limits),
    };
    match plan(&types, limits.images)? {
        Plan::Files { then } => {
            let paths = file_paths(pb);
            if !paths.is_empty() {
                Some(Payload::Files(paths))
            } else {
                then.and_then(fallback)
            }
        }
        Plan::ImageOrFiles(it) => read_image(pb, it, limits).or_else(|| {
            let paths = file_paths(pb);
            (!paths.is_empty()).then_some(Payload::Files(paths))
        }),
        Plan::Text { image } => fallback(Fallback::Text { image }),
        Plan::Image(it) => fallback(Fallback::Image(it)),
        Plan::Color => read_color(pb),
    }
}

pub struct MacClipboardSource {
    pasteboard: Retained<NSPasteboard>,
}

impl MacClipboardSource {
    pub fn new() -> Self {
        Self { pasteboard: NSPasteboard::generalPasteboard() }
    }
}

impl ClipboardSource for MacClipboardSource {
    fn change_count(&mut self) -> i64 {
        self.pasteboard.changeCount() as i64
    }

    fn format_names(&mut self) -> Vec<String> {
        autoreleasepool(|_| {
            self.pasteboard
                .types()
                .map(|t| t.iter().map(|s| s.to_string()).collect())
                .unwrap_or_default()
        })
    }

    fn read(&mut self, limits: &ReadLimits) -> Option<Payload> {
        autoreleasepool(|_| read_payload(&self.pasteboard, limits))
    }
}

pub struct MacClipboardSink;

fn mark(item: &NSPasteboardItem) {
    item.setString_forType(&NSString::from_str("1"), &NSString::from_str(SELF_MARKER_TYPE));
    item.setString_forType(&NSString::from_str(BUNDLE_ID), &NSString::from_str(SOURCE_MARKER_TYPE));
}

fn write_items(pb: &NSPasteboard, items: Vec<Retained<NSPasteboardItem>>) -> bool {
    let objects: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> =
        items.into_iter().map(ProtocolObject::from_retained).collect();
    pb.writeObjects(&NSArray::from_retained_slice(&objects))
}

impl ClipboardSink for MacClipboardSink {
    fn write(&self, request: &WriteRequest) -> Result<i64, ()> {
        autoreleasepool(|_| write_to(&NSPasteboard::generalPasteboard(), request))
    }
}

/// Replaces `pb`'s contents with `request` (plus Satchel's markers) and
/// returns the change count after the write.
fn write_to(pb: &NSPasteboard, request: &WriteRequest) -> Result<i64, ()> {
    pb.clearContents();
    let ok = if !request.files.is_empty() {
        let items: Vec<Retained<NSPasteboardItem>> = request
            .files
            .iter()
            .filter_map(|path| {
                let url = NSURL::fileURLWithPath(&NSString::from_str(path));
                let item = NSPasteboardItem::new();
                // SAFETY: immutable AppKit NSString constant.
                item.setString_forType(&*url.absoluteString()?, unsafe { NSPasteboardTypeFileURL });
                Some(item)
            })
            .collect();
        if let Some(first) = items.first() {
            mark(first);
        }
        !items.is_empty() && write_items(pb, items)
    } else {
        let item = NSPasteboardItem::new();
        // SAFETY (constants below): immutable AppKit NSString constants.
        if let Some(text) = &request.text {
            item.setString_forType(&NSString::from_str(text), unsafe { NSPasteboardTypeString });
        }
        if let Some(html) = &request.html {
            item.setString_forType(&NSString::from_str(html), unsafe { NSPasteboardTypeHTML });
        }
        if let Some(rtf) = &request.rtf {
            item.setData_forType(&NSData::with_bytes(rtf), unsafe { NSPasteboardTypeRTF });
        }
        if let Some(png) = &request.png {
            item.setData_forType(&NSData::with_bytes(png), unsafe { NSPasteboardTypePNG });
            // Older apps only read TIFF.
            if png.len() <= MAX_PNG_FOR_TIFF_COPY
                && let Some(tiff) = png_to_tiff(png)
            {
                item.setData_forType(&tiff, unsafe { NSPasteboardTypeTIFF });
            }
        }
        mark(&item);
        write_items(pb, vec![item])
    };
    if ok { Ok(pb.changeCount() as i64) } else { Err(()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn types(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    const PNG: ImageType = IMAGE_TYPES[0];
    const TIFF: ImageType = IMAGE_TYPES[1];
    const JPEG: ImageType = IMAGE_TYPES[2];

    #[test]
    fn files_come_first_with_text_and_icons_as_fallback() {
        // Finder: file URL + file name as text + icon as TIFF.
        let finder = types(&[UTI_FILE_URL, UTI_TEXT, "public.tiff"]);
        assert_eq!(plan(&finder, true), Some(Plan::Files { then: Some(Fallback::Text { image: Some(TIFF) }) }));
        // Photos / Messages: the image itself plus a URL to a cached file.
        let photos = types(&["public.jpeg", UTI_FILE_URL]);
        assert_eq!(plan(&photos, true), Some(Plan::ImageOrFiles(JPEG)));
        assert_eq!(plan(&photos, false), Some(Plan::Files { then: None }));
        assert_eq!(plan(&types(&[UTI_FILE_URL]), true), Some(Plan::Files { then: None }));
        // A file icon is never the content, whatever its format.
        let icons = types(&[UTI_FILE_URL, UTI_ICNS, "public.png"]);
        assert_eq!(plan(&icons, true), Some(Plan::Files { then: Some(Fallback::Image(PNG)) }));
    }

    #[test]
    fn text_is_planned_with_the_image_it_may_yield_to() {
        let office = types(&["public.html", UTI_TEXT, "public.png"]);
        assert_eq!(plan(&office, true), Some(Plan::Text { image: Some(PNG) }));
        assert_eq!(plan(&office, false), Some(Plan::Text { image: None }));
    }

    #[test]
    fn screenshot_types_plan_png() {
        // `screencapture -c` / Ctrl-Shift-Cmd-4 on macOS 27.
        let shot = types(&["public.png", "Apple PNG pasteboard type", "public.tiff", "NeXT TIFF v4.0 pasteboard type"]);
        assert_eq!(plan(&shot, true), Some(Plan::Image(PNG)));
    }

    #[test]
    fn every_image_format_is_planned_and_needs_the_setting() {
        for it in IMAGE_TYPES {
            assert_eq!(plan(&types(&[it.uti]), true), Some(Plan::Image(*it)), "{}", it.uti);
            assert_eq!(plan(&types(&[it.uti]), false), None);
        }
        // Preference: PNG over TIFF over JPEG.
        assert_eq!(plan(&types(&["public.jpeg", "public.tiff"]), true), Some(Plan::Image(TIFF)));
    }

    #[test]
    fn colors_without_text_are_planned() {
        assert_eq!(plan(&types(&[UTI_COLOR]), true), Some(Plan::Color));
        // A color well that also writes text: the text is classified.
        assert_eq!(plan(&types(&[UTI_COLOR, UTI_TEXT]), true), Some(Plan::Text { image: None }));
    }

    #[test]
    fn unknown_types_are_ignored() {
        assert_eq!(plan(&types(&["com.apple.flat-rtfd"]), true), None);
        assert_eq!(plan(&types(&["com.adobe.pdf"]), true), None);
        assert_eq!(plan(&[], true), None);
    }

    #[test]
    fn hex_colors() {
        assert_eq!(hex_color(1.0, 0.0, 0.5, 1.0), "#FF0080");
        assert_eq!(hex_color(0.0, 0.0, 0.0, 0.5), "#00000080");
        assert_eq!(hex_color(2.0, -1.0, 0.0, 1.0), "#FF0000");
    }

    #[test]
    fn utf8_is_lossy_not_failing() {
        assert_eq!(utf8(b"ok".to_vec()), "ok");
        assert_eq!(utf8(vec![b'a', 0xff, b'b']), "a\u{fffd}b");
    }

    #[test]
    fn uti_constants_match_appkit() {
        // SAFETY: immutable AppKit NSString constants.
        unsafe {
            assert_eq!(NSPasteboardTypeString.to_string(), UTI_TEXT);
            assert_eq!(NSPasteboardTypeFileURL.to_string(), UTI_FILE_URL);
            assert_eq!(NSPasteboardTypePNG.to_string(), PNG.uti);
            assert_eq!(NSPasteboardTypeTIFF.to_string(), TIFF.uti);
            assert_eq!(NSPasteboardTypeRTF.to_string(), "public.rtf");
            assert_eq!(objc2_app_kit::NSPasteboardTypeColor.to_string(), UTI_COLOR);
        }
    }

    #[test]
    fn png_tiff_round_trip_via_appkit() {
        let png = crate::thumbnail::tests::png(8, 4);
        let tiff = png_to_tiff(&png).unwrap().to_vec();
        let back = image_to_png(&tiff).unwrap();
        assert_eq!(crate::thumbnail::tests::dimensions(&back), (8, 4));
    }

    /// Encodes an 8x4 test image in `format` with ImageIO.
    fn encode(format: NSBitmapImageFileType) -> Vec<u8> {
        let png = crate::thumbnail::tests::png(8, 4);
        let rep = NSBitmapImageRep::imageRepWithData(&NSData::with_bytes(&png)).unwrap();
        let props = NSDictionary::new();
        // SAFETY: as in `rep_to_png`.
        unsafe { rep.representationUsingType_properties(format, &props) }.unwrap().to_vec()
    }

    #[test]
    fn jpeg_gif_bmp_convert_to_png() {
        for f in [NSBitmapImageFileType::JPEG, NSBitmapImageFileType::GIF, NSBitmapImageFileType::BMP] {
            let png = image_to_png(&encode(f)).unwrap();
            assert_eq!(crate::thumbnail::tests::dimensions(&png), (8, 4));
            assert!(crate::thumbnail::png_thumbnail(&png, 320).is_ok());
        }
    }

    #[test]
    fn heic_converts_to_png() {
        // A tiny HEIC made by `sips -s format heic` (ImageIO cannot write one
        // from a test without a temporary file).
        let dir = std::env::temp_dir().join(format!("satchel-heic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("a.png");
        let dst = dir.join("a.heic");
        std::fs::write(&src, crate::thumbnail::tests::png(64, 32)).unwrap();
        let ok = std::process::Command::new("/usr/bin/sips")
            .args(["-s", "format", "heic"])
            .arg(&src)
            .arg("--out")
            .arg(&dst)
            .output()
            .is_ok_and(|o| o.status.success());
        if ok {
            let png = image_to_png(&std::fs::read(&dst).unwrap()).unwrap();
            assert_eq!(crate::thumbnail::tests::dimensions(&png), (64, 32));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn garbage_is_not_an_image() {
        assert_eq!(image_to_png(b"not an image"), None);
    }

    // ---- real AppKit, on private pasteboards (never the user's clipboard)

    /// A uniquely named pasteboard, released when dropped.
    struct Board(Retained<NSPasteboard>);

    impl Board {
        fn new() -> Self {
            let pb = NSPasteboard::pasteboardWithUniqueName();
            pb.clearContents();
            Self(pb)
        }
        /// One item carrying `flavors` (type, bytes), like a source app's copy.
        fn item(&self, flavors: &[(&str, Vec<u8>)]) {
            let item = NSPasteboardItem::new();
            for (ty, bytes) in flavors {
                item.setData_forType(&NSData::with_bytes(bytes), &NSString::from_str(ty));
            }
            assert!(write_items(&self.0, vec![item]));
        }
        fn read(&self, limits: &ReadLimits) -> Option<Payload> {
            read_payload(&self.0, limits)
        }
        fn types(&self) -> Vec<String> {
            self.0.types().map(|t| t.iter().map(|s| s.to_string()).collect()).unwrap_or_default()
        }
    }

    impl Drop for Board {
        fn drop(&mut self) {
            // SAFETY: `releaseGlobally` takes no arguments and returns void.
            unsafe {
                let _: () = objc2::msg_send![&*self.0, releaseGlobally];
            }
        }
    }

    fn limits() -> ReadLimits {
        ReadLimits { max_text_bytes: 1024 * 1024, max_image_bytes: 8 * 1024 * 1024, images: true }
    }

    fn png_of(payload: Option<Payload>) -> Vec<u8> {
        match payload {
            Some(Payload::Image { png }) => png,
            other => panic!("expected an image, got {other:?}"),
        }
    }

    #[test]
    fn real_screenshot_flavors_read_as_the_png() {
        let png = crate::thumbnail::tests::png(40, 20);
        let b = Board::new();
        b.item(&[("public.png", png.clone()), ("public.tiff", png_to_tiff(&png).unwrap().to_vec())]);
        assert_eq!(png_of(b.read(&limits())), png);
    }

    #[test]
    fn real_safari_copy_image_reads_the_image_not_the_url() {
        let png = crate::thumbnail::tests::png(40, 20);
        let url = b"https://example.com/cat.jpg".to_vec();
        let b = Board::new();
        b.item(&[
            ("public.tiff", png_to_tiff(&png).unwrap().to_vec()),
            ("com.apple.webarchive", b"bplist00".to_vec()),
            ("public.url", url.clone()),
            ("public.utf8-plain-text", url),
        ]);
        let got = png_of(b.read(&limits()));
        assert_eq!(crate::thumbnail::tests::dimensions(&got), (40, 20));
        // With image capture off, the URL is kept as text instead.
        let off = ReadLimits { images: false, ..limits() };
        assert!(matches!(b.read(&off), Some(Payload::Text { .. })));
    }

    #[test]
    fn real_chrome_copy_image_reads_the_image() {
        let png = crate::thumbnail::tests::png(16, 16);
        let b = Board::new();
        b.item(&[
            ("public.html", b"<meta charset='utf-8'><img src=\"https://e.com/a.png\"/>".to_vec()),
            ("public.png", png.clone()),
        ]);
        assert_eq!(png_of(b.read(&limits())), png);
    }

    #[test]
    fn real_photos_copy_with_a_missing_file_reads_the_jpeg() {
        let jpeg = encode(NSBitmapImageFileType::JPEG);
        let b = Board::new();
        b.item(&[
            ("public.jpeg", jpeg),
            ("public.file-url", b"file:///private/var/folders/none/satchel-missing.jpg".to_vec()),
        ]);
        let got = png_of(b.read(&limits()));
        assert_eq!(crate::thumbnail::tests::dimensions(&got), (8, 4));
    }

    #[test]
    fn real_messages_copy_with_an_existing_cached_file_reads_the_image() {
        let dir = std::env::temp_dir().join(format!("satchel-msg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("IMG_0001.png");
        let png = crate::thumbnail::tests::png(12, 6);
        std::fs::write(&file, &png).unwrap();
        let url = NSURL::fileURLWithPath(&NSString::from_str(file.to_str().unwrap()));
        let b = Board::new();
        b.item(&[
            ("public.png", png.clone()),
            ("public.file-url", url.absoluteString().unwrap().to_string().into_bytes()),
        ]);
        assert_eq!(png_of(b.read(&limits())), png);
        // Image capture off: the file is kept instead.
        let off = ReadLimits { images: false, ..limits() };
        assert_eq!(b.read(&off), Some(Payload::Files(vec![file.to_str().unwrap().to_string()])));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn real_finder_copy_reads_existing_files() {
        let dir = std::env::temp_dir().join(format!("satchel-finder-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("photo one.png");
        std::fs::write(&file, crate::thumbnail::tests::png(4, 4)).unwrap();
        let url = NSURL::fileURLWithPath(&NSString::from_str(file.to_str().unwrap()));
        let b = Board::new();
        b.item(&[
            ("public.file-url", url.absoluteString().unwrap().to_string().into_bytes()),
            ("public.utf8-plain-text", b"photo one.png".to_vec()),
            ("public.tiff", png_to_tiff(&crate::thumbnail::tests::png(4, 4)).unwrap().to_vec()),
        ]);
        assert_eq!(b.read(&limits()), Some(Payload::Files(vec![file.to_str().unwrap().to_string()])));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn real_rich_text_keeps_html_and_rtf() {
        let b = Board::new();
        b.item(&[
            ("public.utf8-plain-text", b"Hello".to_vec()),
            ("public.rtf", b"{\\rtf1 \\b Hello}".to_vec()),
            ("public.png", crate::thumbnail::tests::png(4, 4)),
        ]);
        assert_eq!(
            b.read(&limits()),
            Some(Payload::Text { text: "Hello".into(), html: None, rtf: Some(b"{\\rtf1 \\b Hello}".to_vec()) })
        );
    }

    #[test]
    fn real_color_well_copy_reads_hex() {
        let b = Board::new();
        let color = NSColor::colorWithSRGBRed_green_blue_alpha(1.0, 0.5, 0.0, 1.0);
        // What a color well or the color panel does.
        let obj = ProtocolObject::<dyn NSPasteboardWriting>::from_retained(color);
        assert!(b.0.writeObjects(&NSArray::from_retained_slice(&[obj])));
        assert!(b.types().contains(&UTI_COLOR.to_string()), "{:?}", b.types());
        assert_eq!(b.read(&limits()), Some(Payload::Text { text: "#FF8000".into(), html: None, rtf: None }));
    }

    #[test]
    fn real_oversized_copies_are_reported_not_read() {
        let png = crate::thumbnail::tests::png(64, 64);
        let b = Board::new();
        b.item(&[("public.png", png.clone())]);
        let tiny = ReadLimits { max_image_bytes: png.len() as u64 - 1, ..limits() };
        assert_eq!(
            b.read(&tiny),
            Some(Payload::TooLarge { kind: ClipKind::Image, limit_bytes: png.len() as u64 - 1 })
        );
        let b = Board::new();
        b.item(&[("public.utf8-plain-text", vec![b'x'; 100])]);
        let tiny = ReadLimits { max_text_bytes: 99, ..limits() };
        assert_eq!(b.read(&tiny), Some(Payload::TooLarge { kind: ClipKind::Text, limit_bytes: 99 }));
    }

    #[test]
    fn written_clips_read_back_identically() {
        let dir = std::env::temp_dir().join(format!("satchel-rt-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("folder")).unwrap();
        let file = dir.join("a.txt");
        std::fs::write(&file, b"x").unwrap();
        let png = crate::thumbnail::tests::png(30, 10);
        let rtf = b"{\\rtf1 \\i hi}".to_vec();
        let cases = [
            WriteRequest {
                text: Some("hi".into()),
                html: Some("<i>hi</i>".into()),
                rtf: Some(rtf.clone()),
                ..WriteRequest::default()
            },
            WriteRequest { png: Some(png.clone()), ..WriteRequest::default() },
            WriteRequest {
                files: vec![file.to_str().unwrap().into(), dir.join("folder").to_str().unwrap().into()],
                ..WriteRequest::default()
            },
        ];
        for req in &cases {
            let b = Board::new();
            write_to(&b.0, req).unwrap();
            let types = b.types();
            assert!(types.contains(&SELF_MARKER_TYPE.to_string()), "{types:?}");
            let back = b.read(&limits()).unwrap();
            match back {
                Payload::Text { text, html, rtf: r } => {
                    assert_eq!(Some(text), req.text);
                    assert_eq!(html, req.html);
                    assert_eq!(r, req.rtf);
                }
                Payload::Image { png: p } => {
                    assert_eq!(Some(p), req.png);
                    assert!(types.contains(&"public.tiff".to_string()));
                }
                Payload::Files(paths) => assert_eq!(paths, req.files),
                other => panic!("{other:?}"),
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
