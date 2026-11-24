//! Decoding downloaded preview images with ImageIO (via NSBitmapImageRep).

use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep};
use objc2_foundation::{NSData, NSDictionary};

/// Largest decoded image accepted, in pixels. A preview image or icon is
/// scaled to a few hundred pixels, so this only refuses decompression
/// bombs.
const MAX_PIXELS: u64 = 40_000_000;

/// The image as PNG, or `None` when ImageIO cannot read it or it is too
/// large. The caller has already checked the format by its first bytes.
pub fn to_png(bytes: &[u8]) -> Option<Vec<u8>> {
    objc2::rc::autoreleasepool(|_| {
        let rep = NSBitmapImageRep::imageRepWithData(&NSData::with_bytes(bytes))?;
        let (w, h) = (rep.pixelsWide(), rep.pixelsHigh());
        if w <= 0 || h <= 0 || (w as u64) * (h as u64) > MAX_PIXELS {
            return None;
        }
        let props = NSDictionary::new();
        // SAFETY: an empty dictionary trivially satisfies the expected
        // `NSDictionary<NSBitmapImageRepPropertyKey, id>` element types.
        let png = unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &props) }?;
        Some(png.to_vec())
    })
}
