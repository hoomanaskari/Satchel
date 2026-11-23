//! Shared application state. Everything OS- or Tauri-specific sits behind a
//! trait object so command handlers, the watcher and paste sequencing can be
//! exercised with fakes.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use satchel_core::{ClipKind, Settings, privacy::CaptureContext};

use crate::clipboard::{ClipboardSink, SelfWrites};
use crate::events::EventSink;
use crate::apps::IconCache;
use crate::platform::{AppCatalog, Clock, Desktop};
use crate::shortcut::ShortcutRegistrar;
use crate::storage::SharedStore;

/// The core's decision functions, behind a seam so the app logic can be
/// tested while the core is still being written.
pub trait Policy: Send + Sync {
    fn should_capture(&self, ctx: &CaptureContext<'_>, settings: &Settings) -> bool;
    fn classify_text(&self, text: &str) -> ClipKind;
}

pub struct CorePolicy;

impl Policy for CorePolicy {
    fn should_capture(&self, ctx: &CaptureContext<'_>, settings: &Settings) -> bool {
        satchel_core::privacy::should_capture(ctx, settings)
    }
    fn classify_text(&self, text: &str) -> ClipKind {
        satchel_core::classify::classify_text(text)
    }
}

/// Hides the shelf window. Implementations block until it is hidden.
pub trait ShelfControl: Send + Sync {
    fn hide(&self);
}

/// Opens and closes the settings window.
pub trait SettingsWindowControl: Send + Sync {
    /// Shows the settings window (creating it if needed) and gives it
    /// keyboard focus. `return_to` is the app that gets focus back when the
    /// window closes; it is ignored when the window is already open.
    fn open(&self, return_to: Option<i32>);
    /// Closes the settings window if it is open.
    fn close(&self);
}

pub struct Ctx {
    pub store: SharedStore,
    pub settings: RwLock<Settings>,
    pub self_writes: SelfWrites,
    pub events: Arc<dyn EventSink>,
    pub desktop: Arc<dyn Desktop>,
    pub clipboard: Arc<dyn ClipboardSink>,
    pub policy: Arc<dyn Policy>,
    pub shelf: Arc<dyn ShelfControl>,
    pub settings_window: Arc<dyn SettingsWindowControl>,
    pub shortcuts: Arc<dyn ShortcutRegistrar>,
    pub clock: Arc<dyn Clock>,
    /// Installed and running apps for the excluded-apps picker (and the
    /// source-app icons on the shelf's cards).
    pub apps: Arc<dyn AppCatalog>,
    /// Their icons; emptied when the settings window closes.
    pub app_icons: IconCache,
    /// Pid of the app that was frontmost when the shelf opened.
    pub previous_app: Mutex<Option<i32>>,
    /// One paste sequence at a time.
    pub paste_lock: Mutex<()>,
    /// The Accessibility prompt is shown at most once per run.
    pub accessibility_prompted: AtomicBool,
}

impl Ctx {
    pub fn settings(&self) -> Settings {
        self.settings.read().unwrap_or_else(|p| p.into_inner()).clone()
    }

    pub fn set_settings(&self, settings: Settings) {
        *self.settings.write().unwrap_or_else(|p| p.into_inner()) = settings;
    }

    pub fn capture_paused(&self) -> bool {
        self.settings.read().unwrap_or_else(|p| p.into_inner()).capture_paused
    }

    pub fn remember_previous_app(&self, pid: Option<i32>) {
        *self.previous_app.lock().unwrap_or_else(|p| p.into_inner()) = pid;
    }

    pub fn previous_app(&self) -> Option<i32> {
        *self.previous_app.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Returns true the first time only.
    pub fn take_accessibility_prompt(&self) -> bool {
        !self.accessibility_prompted.swap(true, Ordering::SeqCst)
    }
}
