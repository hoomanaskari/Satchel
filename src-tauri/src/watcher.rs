//! The clipboard watcher thread: the only periodic work Satchel does.
//!
//! macOS has no clipboard-change notification, so the thread sleeps and
//! compares `changeCount` (an integer read, no allocation). Windows will use
//! `AddClipboardFormatListener` and Linux/X11 XFixes selection events: both
//! are event driven and plug into the same `Capturer`.

use std::sync::Arc;
use std::time::Duration;

use crate::capture::Capturer;
use crate::clipboard::ClipboardSource;
use crate::ctx::Ctx;

/// Within the 250-500 ms budget: fast enough that a copy followed by opening
/// the shelf shows the new clip.
pub const POLL_INTERVAL: Duration = Duration::from_millis(300);

/// Polls `source` forever on the current thread.
pub fn run_polling<S: ClipboardSource>(ctx: Arc<Ctx>, source: S, interval: Duration) {
    let mut capturer = Capturer::new(source, ctx.clock.now());
    capturer.prune(&ctx);
    loop {
        std::thread::sleep(interval);
        capturer.poll(&ctx);
    }
}

/// Starts the platform watcher.
pub fn spawn(ctx: Arc<Ctx>) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        std::thread::Builder::new()
            .name("satchel-clipboard".into())
            // The pasteboard handle is created on, and never leaves, this
            // thread.
            .spawn(move || {
                run_polling(ctx, crate::clipboard::SystemClipboardSource::new(), POLL_INTERVAL)
            })?;
    }
    #[cfg(not(target_os = "macos"))]
    {
        // TODO(windows): message-only window + AddClipboardFormatListener.
        // TODO(linux): XFixesSelectSelectionInput on CLIPBOARD.
        let _ = ctx;
    }
    Ok(())
}
