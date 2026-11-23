//! The shelf window: created once at startup, hidden, then only shown and
//! hidden (never recreated) so it appears well under 100 ms.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::ThreadId;
use std::time::{Duration, Instant};

use tauri::utils::config::{Color, WindowEffectsConfig};
use tauri::window::{Effect, EffectState};
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, Runtime, WebviewUrl,
    WebviewWindow, WebviewWindowBuilder, WindowEvent,
};

use crate::ctx::{Ctx, ShelfControl};
use crate::events::SHELF_SHOWN;
use crate::paste;

pub const LABEL: &str = "shelf";
/// Logical window height: the UI's 380 px panel plus its 12 px bottom inset.
/// The window spans the full width of the screen's usable area; the UI
/// draws the panel inset by `PANEL_INSET` on the left, right and bottom.
pub const HEIGHT: f64 = 392.0;
/// The UI's `--panel-inset`; the native glass is inset to match.
pub const PANEL_INSET: f64 = 12.0;
pub const CORNER_RADIUS: f64 = 28.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// Where the shelf goes on a screen whose usable area (without menu bar and
/// Dock) is `work`, in physical pixels with y pointing down.
pub fn dock_rect(work: Rect, scale: f64) -> Rect {
    let height = ((HEIGHT * scale).round() as i64).min(work.height as i64).max(1);
    Rect {
        x: work.x,
        y: (work.y as i64 + work.height as i64 - height) as i32,
        width: work.width.max(1),
        height: height as u32,
    }
}

static MAIN_THREAD: OnceLock<ThreadId> = OnceLock::new();
static TRACE: AtomicBool = AtomicBool::new(false);

/// Whether the `SATCHEL_TRACE` debug aid is on. Traces never carry content.
pub(crate) fn tracing() -> bool {
    TRACE.load(Ordering::Relaxed)
}

fn on_main_thread() -> bool {
    MAIN_THREAD.get() == Some(&std::thread::current().id())
}

/// Native handles and bookkeeping for the one shelf window.
pub struct Shelf<R: Runtime> {
    window: WebviewWindow<R>,
    /// True when the window was converted to a non-activating panel.
    panel: bool,
    /// WKWebView pointer (macOS), filled in once the web view exists.
    webview: Arc<AtomicUsize>,
    /// The glass view has been inset to the panel shape.
    glass_fitted: AtomicBool,
    last_rect: Mutex<Option<Rect>>,
}

impl<R: Runtime> Shelf<R> {
    /// Creates the hidden shelf. Must run on the main thread (Tauri setup).
    pub fn create(app: &AppHandle<R>) -> tauri::Result<Self> {
        let _ = MAIN_THREAD.set(std::thread::current().id());
        TRACE.store(std::env::var_os("SATCHEL_TRACE").is_some(), Ordering::Relaxed);

        let builder = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::default())
            .title("Satchel")
            .inner_size(960.0, HEIGHT)
            .decorations(false)
            .transparent(true)
            .background_color(Color(0, 0, 0, 0))
            .always_on_top(true)
            .skip_taskbar(true)
            .visible_on_all_workspaces(true)
            .resizable(false)
            .maximizable(false)
            .minimizable(false)
            .closable(false)
            // The UI draws its own shadow; a native one would outline the
            // whole transparent window.
            .shadow(false)
            .focused(false)
            .visible(false)
            .accept_first_mouse(true)
            // Native glass behind the transparent web view through Tauri's
            // built-in effects (the official window-vibrancy crate). macOS:
            // NSVisualEffectView, then inset to the UI's panel shape below.
            // Liquid Glass is not used: window-vibrancy re-parents the web view
            // into the glass view, so it could not be inset to the panel.
            // `Active` keeps the material lit although Satchel is never the
            // active app. Ignored on Linux; Windows would blur the whole
            // window, so no Windows effect is requested.
            .effects(WindowEffectsConfig {
                effects: vec![Effect::Popover],
                state: Some(EffectState::Active),
                radius: Some(CORNER_RADIUS),
                color: None,
                interactive: false,
            });
        let window = builder.build()?;

        #[cfg(target_os = "macos")]
        let panel = {
            let mtm = objc2::MainThreadMarker::new().expect("setup runs on the main thread");
            let ns_window = window.ns_window()?;
            crate::platform::macos::make_shelf_panel(mtm, ns_window)
        };
        #[cfg(not(target_os = "macos"))]
        let panel = false;

        let webview = Arc::new(AtomicUsize::new(0));
        #[cfg(target_os = "macos")]
        {
            let slot = webview.clone();
            let _ = window.with_webview(move |w| slot.store(w.inner() as usize, Ordering::SeqCst));
        }

        let shelf = Self {
            window,
            panel,
            webview,
            glass_fitted: AtomicBool::new(false),
            last_rect: Mutex::new(None),
        };
        shelf.fit_glass();
        Ok(shelf)
    }

    /// Insets the native glass to the UI panel (macOS). Retried on show in
    /// case the effect was applied after creation.
    fn fit_glass(&self) {
        if self.glass_fitted.load(Ordering::Relaxed) {
            return;
        }
        #[cfg(target_os = "macos")]
        if let (Some(mtm), Ok(ns_window)) = (objc2::MainThreadMarker::new(), self.window.ns_window()) {
            let fitted = crate::platform::macos::inset_effect_view(mtm, ns_window, PANEL_INSET);
            self.glass_fitted.store(fitted, Ordering::Relaxed);
            if TRACE.load(Ordering::Relaxed) {
                eprintln!("satchel: glass fitted to panel: {fitted}");
            }
        }
    }

    pub fn is_panel(&self) -> bool {
        self.panel
    }

    pub fn is_visible(&self) -> bool {
        self.window.is_visible().unwrap_or(false)
    }

    /// Moves the shelf to the bottom of the screen under the cursor; skips the
    /// native calls when it is already there.
    fn place(&self, app: &AppHandle<R>) {
        let monitor = app
            .cursor_position()
            .ok()
            .and_then(|c| app.monitor_from_point(c.x, c.y).ok().flatten())
            .or_else(|| app.primary_monitor().ok().flatten());
        let Some(monitor) = monitor else { return };
        let area = monitor.work_area();
        let work = Rect {
            x: area.position.x,
            y: area.position.y,
            width: area.size.width,
            height: area.size.height,
        };
        let rect = dock_rect(work, monitor.scale_factor());
        let mut last = self.last_rect.lock().unwrap_or_else(|p| p.into_inner());
        if *last != Some(rect) {
            let _ = self.window.set_size(PhysicalSize::new(rect.width, rect.height));
            let _ = self.window.set_position(PhysicalPosition::new(rect.x, rect.y));
            *last = Some(rect);
        }
    }

    /// Shows the shelf. Main thread only (shortcut and tray handlers).
    pub fn show(&self, app: &AppHandle<R>, ctx: &Ctx) {
        let started = Instant::now();
        ctx.remember_previous_app(ctx.desktop.frontmost_app().map(|a| a.pid));
        self.place(app);
        self.fit_glass();
        #[cfg(target_os = "macos")]
        if self.panel
            && let (Some(mtm), Ok(ns_window)) = (objc2::MainThreadMarker::new(), self.window.ns_window())
        {
            let webview = self.webview.load(Ordering::SeqCst) as *mut std::ffi::c_void;
            crate::platform::macos::show_panel(mtm, ns_window, webview);
        }
        if !self.panel {
            let _ = self.window.show();
            let _ = self.window.set_focus();
        }
        let _ = app.emit(SHELF_SHOWN, ());
        if TRACE.load(Ordering::Relaxed) {
            eprintln!("satchel: shelf shown in {} us", started.elapsed().as_micros());
            self.trace_page();
        }
    }

    /// Debug aid (`SATCHEL_TRACE`): reports page state half a second later.
    /// Prints counts only, never page text.
    fn trace_page(&self) {
        let window = self.window.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(500));
            let _ = window.eval_with_callback(
                "document.visibilityState + ' elements=' + document.getElementsByTagName('*').length",
                |result| eprintln!("satchel: page {result}"),
            );
        });
    }

    fn hide_now(&self) {
        let _ = self.window.hide();
        if TRACE.load(Ordering::Relaxed) {
            eprintln!("satchel: shelf hidden");
            self.trace_page();
        }
    }

    /// Shortcut / tray behavior: show, or dismiss when already visible.
    pub fn toggle(&self, app: &AppHandle<R>, ctx: &Ctx) {
        if self.is_visible() {
            paste::dismiss_shelf(ctx);
        } else {
            self.show(app, ctx);
        }
    }

    /// Hides when the shelf loses key focus (click elsewhere). Focus is not
    /// pushed back: the user chose where it goes.
    pub fn install_blur_handler(app: &AppHandle<R>) {
        let Some(window) = app.get_webview_window(LABEL) else { return };
        let handle = app.clone();
        window.on_window_event(move |event| {
            if TRACE.load(Ordering::Relaxed)
                && let WindowEvent::Focused(focused) = event
            {
                let front = handle
                    .try_state::<std::sync::Arc<Ctx>>()
                    .and_then(|c| c.desktop.frontmost_app())
                    .map(|a| a.name)
                    .unwrap_or_default();
                eprintln!("satchel: shelf focused={focused} (frontmost: {front})");
            }
            if let WindowEvent::Focused(false) = event
                && let Some(shelf) = handle.try_state::<Shelf<R>>()
            {
                shelf.hide_now();
            }
        });
    }
}

/// `ShelfControl` used by paste and the `hide_shelf` command. Blocks until
/// the window is really hidden, so a paste keystroke can never land in the
/// shelf itself.
pub struct TauriShelf<R: Runtime> {
    pub app: AppHandle<R>,
}

/// Hides the shelf and, if Satchel ended up as the active app, hands
/// keyboard focus back to the app that was in front when the shelf opened.
/// Main thread only.
fn hide_and_hand_back<R: Runtime>(app: &AppHandle<R>) {
    if let Some(shelf) = app.try_state::<Shelf<R>>() {
        shelf.hide_now();
    }
    #[cfg(target_os = "macos")]
    if let (Some(mtm), Some(ctx)) = (objc2::MainThreadMarker::new(), app.try_state::<std::sync::Arc<Ctx>>())
        && let Some(pid) = ctx.previous_app()
    {
        crate::platform::macos::hand_focus_to(mtm, pid);
    }
}

impl<R: Runtime> ShelfControl for TauriShelf<R> {
    fn hide(&self) {
        if on_main_thread() {
            hide_and_hand_back(&self.app);
            return;
        }
        let (tx, rx) = mpsc::channel();
        let app = self.app.clone();
        let sent = self.app.run_on_main_thread(move || {
            hide_and_hand_back(&app);
            let _ = tx.send(());
        });
        if sent.is_ok() {
            let _ = rx.recv_timeout(Duration::from_secs(1));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_the_bottom_of_a_retina_screen() {
        // 1512x982 pt MacBook screen at 2x, menu bar 37 pt (74 px), no Dock.
        let work = Rect { x: 0, y: 74, width: 3024, height: 1890 };
        let r = dock_rect(work, 2.0);
        assert_eq!(r, Rect { x: 0, y: 74 + 1890 - 784, width: 3024, height: 784 });
    }

    #[test]
    fn respects_the_origin_of_secondary_screens() {
        let work = Rect { x: -1920, y: -200, width: 1920, height: 1080 };
        let r = dock_rect(work, 1.0);
        assert_eq!(r.x, -1920);
        assert_eq!(r.y + r.height as i32, -200 + 1080);
        assert_eq!((r.width, r.height), (1920, 392));
    }

    #[test]
    fn never_taller_than_the_screen() {
        let work = Rect { x: 0, y: 0, width: 800, height: 200 };
        let r = dock_rect(work, 1.0);
        assert_eq!((r.y, r.height), (0, 200));
    }

    #[test]
    fn degenerate_screens_do_not_panic() {
        let r = dock_rect(Rect { x: 0, y: 0, width: 0, height: 0 }, 3.0);
        assert!(r.width >= 1 && r.height >= 1);
    }
}
