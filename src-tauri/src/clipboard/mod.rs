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
/// representation Satchel keeps (files win over text, text over images).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Payload {
    Files(Vec<String>),
    Text { text: String, html: Option<String> },
    /// PNG-encoded image.
    Image { png: Vec<u8> },
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
    pub png: Option<Vec<u8>>,
    pub files: Vec<String>,
}

/// Write side of the clipboard.
pub trait ClipboardSink: Send + Sync {
    /// Replaces the clipboard contents and returns the change count after
    /// the write.
    fn write(&self, request: &WriteRequest) -> Result<i64, ()>;
}

/// Builds what gets written for a stored clip. `plain` drops rich text; for
/// files it pastes their paths as text. Images ignore `plain`.
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
                ..WriteRequest::default()
            }
        }
    };
    Ok(req)
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
            image_png: None,
            file_paths: vec![],
        }
    }

    #[test]
    fn rich_text_keeps_html_plain_drops_it() {
        let rich = write_request(&content(ClipKind::Text), false).unwrap();
        assert_eq!(rich.text.as_deref(), Some("hello"));
        assert_eq!(rich.html.as_deref(), Some("<b>hello</b>"));
        let plain = write_request(&content(ClipKind::Link), true).unwrap();
        assert_eq!(plain.text.as_deref(), Some("hello"));
        assert_eq!(plain.html, None);
    }

    #[test]
    fn files_write_urls_or_paths_as_text() {
        let c = ClipContent {
            kind: ClipKind::File,
            text: None,
            html: None,
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

    #[test]
    fn marker_detection() {
        assert!(has_self_marker(&["public.utf8-plain-text".into(), SELF_MARKER_TYPE.into()]));
        assert!(!has_self_marker(&["public.utf8-plain-text".into()]));
    }
}
