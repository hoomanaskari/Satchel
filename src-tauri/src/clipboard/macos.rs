//! NSPasteboard implementation.
//!
//! The watcher keeps one `NSPasteboard` handle and polls `changeCount`, a
//! plain integer property: no allocation, no autorelease traffic when nothing
//! changed. Types and data are only fetched after a change, inside an
//! autorelease pool.

use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{
    NSBitmapImageFileType, NSBitmapImageRep, NSPasteboard, NSPasteboardItem, NSPasteboardType,
    NSPasteboardTypeFileURL, NSPasteboardTypeHTML, NSPasteboardTypePNG, NSPasteboardTypeString,
    NSPasteboardTypeTIFF, NSPasteboardWriting,
};
use objc2_foundation::{NSArray, NSData, NSDictionary, NSString, NSURL};

use super::{
    ClipboardSink, ClipboardSource, MAX_FILES, MAX_PATH_BYTES, Payload, ReadLimits,
    SELF_MARKER_TYPE, SOURCE_MARKER_TYPE, WriteRequest,
};

pub const BUNDLE_ID: &str = "app.satchel.desktop";
/// TIFF is uncompressed; accept a larger raw size before converting to PNG.
const TIFF_FACTOR: u64 = 4;

const UTI_FILE_URL: &str = "public.file-url";
const UTI_TEXT: &str = "public.utf8-plain-text";
const UTI_PNG: &str = "public.png";
const UTI_TIFF: &str = "public.tiff";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plan {
    Files,
    Text,
    Png,
    Tiff,
}

/// Decides what to read from the type list alone: files win over text,
/// text over images (rich apps put a picture rendition next to copied text).
pub fn plan(types: &[String], images: bool) -> Option<Plan> {
    let has = |t: &str| types.iter().any(|x| x == t);
    if has(UTI_FILE_URL) {
        Some(Plan::Files)
    } else if has(UTI_TEXT) {
        Some(Plan::Text)
    } else if images && has(UTI_PNG) {
        Some(Plan::Png)
    } else if images && has(UTI_TIFF) {
        Some(Plan::Tiff)
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

fn tiff_to_png(tiff: &[u8]) -> Option<Vec<u8>> {
    let rep = NSBitmapImageRep::imageRepWithData(&NSData::with_bytes(tiff))?;
    let props = NSDictionary::new();
    // SAFETY: an empty dictionary trivially satisfies the expected
    // `NSDictionary<NSBitmapImageRepPropertyKey, id>` element types.
    let png = unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &props) }?;
    Some(png.to_vec())
}

fn png_to_tiff(png: &[u8]) -> Option<Retained<NSData>> {
    NSBitmapImageRep::imageRepWithData(&NSData::with_bytes(png))?.TIFFRepresentation()
}

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
        if let Some(path) = url.path() {
            let path = path.to_string();
            if !path.is_empty() && path.len() <= MAX_PATH_BYTES {
                paths.push(path);
            }
        }
    }
    paths
}

fn read_payload(pb: &NSPasteboard, limits: &ReadLimits) -> Option<Payload> {
    let types: Vec<String> = pb
        .types()
        .map(|t| t.iter().map(|s| s.to_string()).collect())
        .unwrap_or_default();
    // SAFETY (all `unsafe { NSPasteboardType* }` below): these are immutable
    // NSString constants exported by AppKit, valid for the process lifetime.
    match plan(&types, limits.images)? {
        Plan::Files => {
            let paths = file_paths(pb);
            (!paths.is_empty()).then_some(Payload::Files(paths))
        }
        Plan::Text => {
            let text = utf8(data(pb, unsafe { NSPasteboardTypeString }, limits.max_text_bytes)?);
            let html = data(pb, unsafe { NSPasteboardTypeHTML }, limits.max_text_bytes).map(utf8);
            Some(Payload::Text { text, html })
        }
        Plan::Png => {
            let png = data(pb, unsafe { NSPasteboardTypePNG }, limits.max_image_bytes)?;
            Some(Payload::Image { png })
        }
        Plan::Tiff => {
            let tiff = data(pb, unsafe { NSPasteboardTypeTIFF }, limits.max_image_bytes * TIFF_FACTOR)?;
            let png = tiff_to_png(&tiff)?;
            (png.len() as u64 <= limits.max_image_bytes).then_some(Payload::Image { png })
        }
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
        autoreleasepool(|_| {
            let pb = NSPasteboard::generalPasteboard();
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
                !items.is_empty() && write_items(&pb, items)
            } else {
                let item = NSPasteboardItem::new();
                // SAFETY (constants below): immutable AppKit NSString constants.
                if let Some(text) = &request.text {
                    item.setString_forType(&NSString::from_str(text), unsafe { NSPasteboardTypeString });
                }
                if let Some(html) = &request.html {
                    item.setString_forType(&NSString::from_str(html), unsafe { NSPasteboardTypeHTML });
                }
                if let Some(png) = &request.png {
                    item.setData_forType(&NSData::with_bytes(png), unsafe { NSPasteboardTypePNG });
                    // Older apps only read TIFF.
                    if let Some(tiff) = png_to_tiff(png) {
                        item.setData_forType(&tiff, unsafe { NSPasteboardTypeTIFF });
                    }
                }
                mark(&item);
                write_items(&pb, vec![item])
            };
            if ok { Ok(pb.changeCount() as i64) } else { Err(()) }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn types(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn files_win_over_text_and_icons() {
        // Finder: file URL + file name as text + icon as TIFF.
        let finder = types(&[UTI_FILE_URL, UTI_TEXT, UTI_TIFF]);
        assert_eq!(plan(&finder, true), Some(Plan::Files));
    }

    #[test]
    fn text_wins_over_picture_renditions() {
        let office = types(&["public.html", UTI_TEXT, UTI_PNG]);
        assert_eq!(plan(&office, true), Some(Plan::Text));
    }

    #[test]
    fn images_need_the_setting() {
        assert_eq!(plan(&types(&[UTI_PNG, UTI_TIFF]), true), Some(Plan::Png));
        assert_eq!(plan(&types(&[UTI_TIFF]), true), Some(Plan::Tiff));
        assert_eq!(plan(&types(&[UTI_PNG]), false), None);
    }

    #[test]
    fn unknown_types_are_ignored() {
        assert_eq!(plan(&types(&["com.apple.flat-rtfd"]), true), None);
        assert_eq!(plan(&[], true), None);
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
            assert_eq!(NSPasteboardTypePNG.to_string(), UTI_PNG);
            assert_eq!(NSPasteboardTypeTIFF.to_string(), UTI_TIFF);
        }
    }

    #[test]
    fn png_tiff_round_trip_via_appkit() {
        let png = crate::thumbnail::tests::png(8, 4);
        let tiff = png_to_tiff(&png).unwrap().to_vec();
        let back = tiff_to_png(&tiff).unwrap();
        assert_eq!(crate::thumbnail::tests::dimensions(&back), (8, 4));
    }
}
