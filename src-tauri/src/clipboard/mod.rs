//! Clipboard seam: what the watcher reads and what paste/copy write.
//!
//! The OS implementations live in per-platform files; the logic that uses
//! them (capture, paste, self-write suppression) is tested with fakes.

use std::sync::Mutex;

use satchel_core::{ClipContent, ClipKind};

use crate::error::{self, CmdResult};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(target_os = "macos"))]
mod unsupported;

#[cfg(target_os = "macos")]
pub use macos::{MacClipboardSink as SystemClipboardSink, MacClipboardSource as SystemClipboardSource};
#[cfg(not(target_os = "macos"))]
pub use unsupported::{
    UnsupportedClipboardSink as SystemClipboardSink,
    UnsupportedClipboardSource as SystemClipboardSource,
};

/// Pasteboard type Satchel adds to everything it writes, so the watcher can
/// recognize its own writes even if the change count bookkeeping misses one.
pub const SELF_MARKER_TYPE: &str = "app.satchel.desktop.self";
/// nspasteboard.org convention: identifies the app that wrote the clipboard.
pub const SOURCE_MARKER_TYPE: &str = "org.nspasteboard.source";

/// Maximum number of file paths captured from one copy, and their length.
pub const MAX_FILES: usize = 1000;
pub const MAX_PATH_BYTES: usize = 4096;

/// What one clipboard change contained, already reduced to the single
/// representation Satchel keeps. Files (that exist) win over text, text over
/// images, except when the text is only the address or markup of a copied
/// image (a browser's "Copy Image"); see the platform reader.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Payload {
    Files(Vec<String>),
    /// Text with the rich flavors that came with it (HTML from browsers,
    /// RTF from Cocoa text views), each `None` when absent or over the cap.
    Text { text: String, html: Option<String>, rtf: Option<Vec<u8>> },
    /// PNG-encoded image (other formats are converted by the reader).
    Image { png: Vec<u8> },
    /// Something Satchel would keep, but over its size cap. Nothing beyond
    /// the size was read; it is reported, not stored.
    TooLarge { kind: ClipKind, limit_bytes: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadLimits {
    pub max_text_bytes: u64,
    pub max_image_bytes: u64,
    pub images: bool,
}

/// Read side of the clipboard. Owned by the watcher thread only, so it does
/// not need to be `Send`.
pub trait ClipboardSource {
    /// Monotonic change counter. Must be cheap and must not allocate: it is
    /// called on every poll.
    fn change_count(&mut self) -> i64;
    /// Raw type/format names of the current contents (no content is read).
    fn format_names(&mut self) -> Vec<String>;
    /// Reads the contents, honoring the limits. `None` when there is nothing
    /// Satchel keeps (unknown types, over the size caps, empty).
    fn read(&mut self, limits: &ReadLimits) -> Option<Payload>;
}

/// What to put on the clipboard.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WriteRequest {
    pub text: Option<String>,
    pub html: Option<String>,
    pub rtf: Option<Vec<u8>>,
    pub png: Option<Vec<u8>>,
    pub files: Vec<String>,
}

/// Write side of the clipboard.
pub trait ClipboardSink: Send + Sync {
    /// Replaces the clipboard contents and returns the change count after
    /// the write.
    fn write(&self, request: &WriteRequest) -> Result<i64, ()>;
}

/// Builds what gets written for a stored clip. `plain` drops rich text (HTML
/// and RTF); for files it pastes their paths as text. Images ignore `plain`.
pub fn write_request(content: &ClipContent, plain: bool) -> CmdResult<WriteRequest> {
    let req = match content.kind {
        ClipKind::File => {
            if content.file_paths.is_empty() {
                return Err(error::NOT_FOUND.into());
            }
            if plain {
                WriteRequest {
                    text: Some(content.file_paths.join("\n")),
                    ..WriteRequest::default()
                }
            } else {
                WriteRequest {
                    files: content.file_paths.clone(),
                    ..WriteRequest::default()
                }
            }
        }
        ClipKind::Image => match &content.image_png {
            Some(png) if !png.is_empty() => WriteRequest {
                png: Some(png.clone()),
                ..WriteRequest::default()
            },
            _ => return Err(error::NOT_FOUND.into()),
        },
        ClipKind::Text | ClipKind::Link | ClipKind::Code | ClipKind::Color => {
            let Some(text) = &content.text else {
                return Err(error::NOT_FOUND.into());
            };
            WriteRequest {
                text: Some(text.clone()),
                html: if plain { None } else { content.html.clone() },
                rtf: if plain { None } else { content.rtf.clone() },
                ..WriteRequest::default()
            }
        }
    };
    Ok(req)
}

/// Pasteboard types that say "this text is the address of what was copied"
/// (WebKit and Chromium add them to "Copy Image" and to copied links).
const URL_TYPES: &[&str] = &["public.url", "Apple URL pasteboard type"];
/// HTML longer than this is never treated as image-only markup.
const MAX_IMAGE_HTML_BYTES: usize = 64 * 1024;

/// Decides, when a change carries both text and an image, whether the image
/// is what the user copied. Rich apps (Pages, Word, Notes, Numbers) put a
/// picture rendition next to copied text, so text normally wins. A browser's
/// "Copy Image" instead puts the image next to its URL as text (Safari) or
/// next to `<img>` markup with no visible text (Chrome, Firefox, Electron),
/// and there the image wins.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn image_wins_over_text(types: &[String], text: &str, html: Option<&str>) -> bool {
    let t = text.trim();
    if t.is_empty() {
        // Blank text is never kept; the image is the content.
        return true;
    }
    let single_token = !t.is_empty() && !t.contains(char::is_whitespace);
    let has_url_type = types.iter().any(|ty| URL_TYPES.contains(&ty.as_str()));
    if has_url_type && single_token {
        return true;
    }
    html.is_some_and(html_is_only_images)
}

/// True when `html` shows at least one `<img>` and no visible text: the
/// markup a browser writes for a copied image.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn html_is_only_images(html: &str) -> bool {
    if html.len() > MAX_IMAGE_HTML_BYTES {
        return false;
    }
    let mut images = 0;
    let mut rest = html;
    let mut skip_until: Option<&str> = None;
    while !rest.is_empty() {
        if let Some(end) = skip_until {
            match find_ignore_case(rest, end) {
                Some(i) => {
                    rest = &rest[i + end.len()..];
                    skip_until = None;
                    continue;
                }
                None => break,
            }
        }
        match rest.find('<') {
            Some(i) => {
                if !is_blank_text(&rest[..i]) {
                    return false;
                }
                let tag_end = match rest[i..].find('>') {
                    Some(j) => i + j + 1,
                    None => return false,
                };
                let tag = rest[i + 1..tag_end - 1].trim_start().to_ascii_lowercase();
                let name: String = tag.chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
                match name.as_str() {
                    "img" => images += 1,
                    "style" | "script" | "title" | "head" => {
                        skip_until = Some(match name.as_str() {
                            "style" => "</style>",
                            "script" => "</script>",
                            "title" => "</title>",
                            _ => "</head>",
                        })
                    }
                    _ => {}
                }
                rest = &rest[tag_end..];
            }
            None => {
                if !is_blank_text(rest) {
                    return false;
                }
                break;
            }
        }
    }
    images > 0
}

fn find_ignore_case(hay: &str, needle: &str) -> Option<usize> {
    let hay_b = hay.as_bytes();
    let n = needle.as_bytes();
    (0..hay_b.len().saturating_sub(n.len() - 1))
        .find(|&i| hay_b[i..i + n.len()].eq_ignore_ascii_case(n))
}

/// Whitespace and the entities pages use for it.
fn is_blank_text(s: &str) -> bool {
    s.replace("&nbsp;", " ").replace("&#160;", " ").trim().is_empty()
}

/// Remembers the change count produced by Satchel's own latest write.
///
/// The lock is held across the write and the bookkeeping, and the watcher
/// takes it before deciding, so the watcher can never observe our write
/// before it has been recorded.
#[derive(Default)]
pub struct SelfWrites {
    last: Mutex<Option<i64>>,
}

impl SelfWrites {
    pub fn write_with<E>(&self, write: impl FnOnce() -> Result<i64, E>) -> Result<i64, E> {
        let mut last = self.last.lock().unwrap_or_else(|p| p.into_inner());
        let count = write()?;
        *last = Some(count);
        Ok(count)
    }

    pub fn is_self(&self, change_count: i64) -> bool {
        *self.last.lock().unwrap_or_else(|p| p.into_inner()) == Some(change_count)
    }
}

/// True when the format list carries Satchel's own marker.
pub fn has_self_marker(format_names: &[String]) -> bool {
    format_names.iter().any(|f| f == SELF_MARKER_TYPE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn content(kind: ClipKind) -> ClipContent {
        ClipContent {
            id: 1,
            kind,
            text: Some("hello".into()),
            html: Some("<b>hello</b>".into()),
            rtf: Some(b"{\\rtf1 \\b hello}".to_vec()),
            image_png: None,
            file_paths: vec![],
        }
    }

    #[test]
    fn rich_text_keeps_html_plain_drops_it() {
        let rich = write_request(&content(ClipKind::Text), false).unwrap();
        assert_eq!(rich.text.as_deref(), Some("hello"));
        assert_eq!(rich.html.as_deref(), Some("<b>hello</b>"));
        assert_eq!(rich.rtf.as_deref(), Some(&b"{\\rtf1 \\b hello}"[..]));
        let plain = write_request(&content(ClipKind::Link), true).unwrap();
        assert_eq!(plain.text.as_deref(), Some("hello"));
        assert_eq!(plain.html, None);
        assert_eq!(plain.rtf, None);
    }

    #[test]
    fn files_write_urls_or_paths_as_text() {
        let c = ClipContent {
            kind: ClipKind::File,
            text: None,
            html: None,
            rtf: None,
            file_paths: vec!["/a/b.txt".into(), "/c d/e.png".into()],
            ..content(ClipKind::File)
        };
        let rich = write_request(&c, false).unwrap();
        assert_eq!(rich.files, c.file_paths);
        assert_eq!(rich.text, None);
        let plain = write_request(&c, true).unwrap();
        assert!(plain.files.is_empty());
        assert_eq!(plain.text.as_deref(), Some("/a/b.txt\n/c d/e.png"));
    }

    #[test]
    fn images_ignore_plain() {
        let c = ClipContent {
            kind: ClipKind::Image,
            text: None,
            html: None,
            rtf: None,
            image_png: Some(vec![1, 2, 3]),
            ..content(ClipKind::Image)
        };
        assert_eq!(write_request(&c, true).unwrap().png, Some(vec![1, 2, 3]));
    }

    #[test]
    fn missing_payload_is_an_error() {
        let c = ClipContent { text: None, ..content(ClipKind::Code) };
        assert!(write_request(&c, false).is_err());
        let img = ClipContent { image_png: None, ..content(ClipKind::Image) };
        assert!(write_request(&img, false).is_err());
        let files = ClipContent { file_paths: vec![], ..content(ClipKind::File) };
        assert!(write_request(&files, false).is_err());
    }

    #[test]
    fn self_writes_are_recognized_by_change_count() {
        let sw = SelfWrites::default();
        assert!(!sw.is_self(5));
        assert_eq!(sw.write_with(|| Ok::<_, ()>(5)), Ok(5));
        assert!(sw.is_self(5));
        assert!(!sw.is_self(6));
    }

    #[test]
    fn failed_write_records_nothing() {
        let sw = SelfWrites::default();
        assert!(sw.write_with(|| Err::<i64, _>(())).is_err());
        assert!(!sw.is_self(0));
    }

    fn types(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn safari_copy_image_is_an_image_not_its_url() {
        // Safari "Copy Image": TIFF + web archive + the image URL as text.
        let t = types(&["public.tiff", "com.apple.webarchive", "public.url", "public.utf8-plain-text"]);
        assert!(image_wins_over_text(&t, "https://example.com/cat.jpg", None));
    }

    #[test]
    fn chrome_copy_image_is_an_image() {
        let html = "<meta charset='utf-8'><img src=\"https://example.com/cat.png\" alt=\"A cat\"/>";
        let t = types(&["public.html", "public.png"]);
        assert!(image_wins_over_text(&t, "", Some(html)));
        assert!(image_wins_over_text(&t, " ", Some(html)));
        assert!(image_wins_over_text(&t, " \n", None));
    }

    #[test]
    fn copied_text_with_a_picture_rendition_stays_text() {
        // Pages / Word / Notes: text, RTF or HTML, plus a PNG or PDF of it.
        let t = types(&["public.utf8-plain-text", "public.rtf", "public.png"]);
        assert!(!image_wins_over_text(&t, "Quarterly report", None));
        let html = "<p>Hello <img src=\"x.png\"> world</p>";
        assert!(!image_wins_over_text(&types(&["public.html", "public.png"]), "Hello world", Some(html)));
        // A copied link with an icon is a link: several words of text.
        let t = types(&["public.url", "public.utf8-plain-text", "public.tiff"]);
        assert!(!image_wins_over_text(&t, "Example Domain https://example.com", None));
    }

    #[test]
    fn image_only_html_detection() {
        assert!(html_is_only_images("<img src=a>"));
        assert!(html_is_only_images("<html><head><style>p{}</style><title>t</title></head><body>&nbsp;<IMG SRC=a></body></html>"));
        assert!(!html_is_only_images("<b>bold</b>"));
        assert!(!html_is_only_images("<p>caption</p><img src=a>"));
        assert!(!html_is_only_images("no tags"));
        assert!(!html_is_only_images("<img src=a"));
        assert!(!html_is_only_images(&format!("<img src=a>{}", " ".repeat(MAX_IMAGE_HTML_BYTES))));
    }

    #[test]
    fn marker_detection() {
        assert!(has_self_marker(&["public.utf8-plain-text".into(), SELF_MARKER_TYPE.into()]));
        assert!(!has_self_marker(&["public.utf8-plain-text".into()]));
    }
}
