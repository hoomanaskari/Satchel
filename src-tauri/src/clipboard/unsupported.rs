//! Compiling fallback for platforms whose clipboard watcher is not written
//! yet. It reports no changes and refuses writes.
//!
//! Windows: an event-driven watcher on `AddClipboardFormatListener`
//! (`WM_CLIPBOARDUPDATE`) with `GetClipboardSequenceNumber` as the change
//! count; formats via `EnumClipboardFormats`/`GetClipboardFormatName`, which
//! also exposes the `ExcludeClipboardContentFromMonitorProcessing` /
//! `CanIncludeInClipboardHistory` opt-out formats the privacy gate needs.
//! Linux/X11: XFixes `SelectionNotify` events on `CLIPBOARD`, targets via
//! the `TARGETS` atom (honoring `x-kde-passwordManagerHint`).

use super::{ClipboardSink, ClipboardSource, Payload, ReadLimits, WriteRequest};

pub struct UnsupportedClipboardSource;

impl UnsupportedClipboardSource {
    pub fn new() -> Self {
        Self
    }
}

impl ClipboardSource for UnsupportedClipboardSource {
    fn change_count(&mut self) -> i64 {
        0
    }
    fn format_names(&mut self) -> Vec<String> {
        Vec::new()
    }
    fn read(&mut self, _limits: &ReadLimits) -> Option<Payload> {
        None
    }
}

pub struct UnsupportedClipboardSink;

impl ClipboardSink for UnsupportedClipboardSink {
    fn write(&self, _request: &WriteRequest) -> Result<i64, ()> {
        Err(())
    }
}
