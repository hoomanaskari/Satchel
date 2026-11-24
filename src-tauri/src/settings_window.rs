//! The settings window: a regular, titled app window that loads the
//! `/settings` route.
//!
//! Unlike the shelf it is created on demand and destroyed when closed. It is
//! opened rarely, its first paint is not latency-critical, and a destroyed
//! web view gives its memory back, which matters more for a tray app that
//! runs all day (see the performance budgets). At most one exists: opening it
//! again shows and focuses the existing window.
//!
//! Satchel is an accessory app (no Dock icon) whose shelf is a
//! non-activating panel, so Satchel is normally never the active app. A
//! normal window only receives keystrokes when its app is active, so opening
//! settings activates Satchel (`set_focus` does `activateIgnoringOtherApps`
//! on macOS). When the window closes, focus is handed back to the app that
//! was in front when it opened; otherwise Satchel would stay active with no
//! visible window and keystrokes would go nowhere.
//!
//! The window is resizable between a minimum (the two-column layout without
//! clipping) and a maximum, with no zoom and no full screen. Its frame is
//! saved as it moves or resizes (debounced) and when it closes, in the
//! store's window state (not part of `Settings`), and restored on the next
//! open, checked against the displays present then (see `window_frame`).

use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use tauri::{AppHandle, Manager, Runtime, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent};

use crate::ctx::SettingsWindowControl;
use crate::window_frame::{FrameSaver, Rect};

pub const LABEL: &str = "settings";
/// The SvelteKit route the window loads (emitted as `settings.html`).
pub const ROUTE: &str = "settings";
// Sizes in points. tao converts them from content to frame size with the
// window's content rect, which for this window equals its frame, so in
// practice they are whole-window sizes, title bar included (measured).
/// Size on first open.
pub const WIDTH: f64 = 760.0;
pub const HEIGHT: f64 = 600.0;
/// Narrowest that fits both columns (two 260 pt sections, gaps and margins);
/// lowest that still shows a section above the pinned footer. The body
/// scrolls below the content's natural height.
pub const MIN_WIDTH: f64 = 600.0;
pub const MIN_HEIGHT: f64 = 400.0;
/// Past this the sections only gain empty space.
pub const MAX_WIDTH: f64 = 1040.0;
pub const MAX_HEIGHT: f64 = 1000.0;
/// Key of the saved frame in the store's window state.
const STATE_KEY: &str = "settings";
/// How long the window must be still before a move or resize is saved.
const SAVE_AFTER: Duration = Duration::from_millis(500);

/// What to do with focus once the settings window is gone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AfterClose {
    /// Satchel is not the active app (the user already moved on): leave
    /// focus where it is.
    Leave,
    /// Give focus back to this app.
    Activate(i32),
    /// No app to return to: deactivate Satchel so the system picks one.
    Deactivate,
}

pub fn after_close(satchel_active: bool, return_to: Option<i32>) -> AfterClose {
    match (satchel_active, return_to) {
        (false, _) => AfterClose::Leave,
        (true, Some(pid)) => AfterClose::Activate(pid),
        (true, None) => AfterClose::Deactivate,
    }
}

/// `SettingsWindowControl` backed by a Tauri window. Window work always runs
/// on the main thread.
pub struct TauriSettingsWindow<R: Runtime> {
    pub app: AppHandle<R>,
    /// The app to give focus back to when the window closes.
    return_to: Arc<Mutex<Option<i32>>>,
    /// Writes the window's frame to the store; started on first open.
    saver: Arc<OnceLock<FrameSaver>>,
}

impl<R: Runtime> TauriSettingsWindow<R> {
    pub fn new(app: AppHandle<R>) -> Self {
        Self { app, return_to: Arc::new(Mutex::new(None)), saver: Arc::new(OnceLock::new()) }
    }
}

impl<R: Runtime> SettingsWindowControl for TauriSettingsWindow<R> {
    fn open(&self, return_to: Option<i32>) {
        let app = self.app.clone();
        let slot = self.return_to.clone();
        let saver = self.saver.clone();
        let _ = self.app.run_on_main_thread(move || {
            let saver = saver.get_or_init(|| {
                let app = app.clone();
                FrameSaver::new(SAVE_AFTER, move |frame| save_frame(&app, frame))
            });
            open_on_main(&app, &slot, saver, return_to)
        });
    }

    fn close(&self) {
        let app = self.app.clone();
        let _ = self.app.run_on_main_thread(move || {
            if let Some(window) = app.get_webview_window(LABEL) {
                let _ = window.close();
            }
        });
    }
}

fn open_on_main<R: Runtime>(
    app: &AppHandle<R>,
    slot: &Arc<Mutex<Option<i32>>>,
    saver: &FrameSaver,
    return_to: Option<i32>,
) {
    if let Some(window) = app.get_webview_window(LABEL) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        return;
    }
    *slot.lock().unwrap_or_else(|p| p.into_inner()) = return_to;

    let built = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App(ROUTE.into()))
        .title("Satchel Settings")
        .inner_size(WIDTH, HEIGHT)
        .min_inner_size(MIN_WIDTH, MIN_HEIGHT)
        .max_inner_size(MAX_WIDTH, MAX_HEIGHT)
        .resizable(true)
        // No zoom button: the window has a size limit, and full screen
        // would only add empty space around the form.
        .maximizable(false)
        .minimizable(true)
        .closable(true)
        .center()
        .focused(true)
        // Shown once it has been moved to its saved frame.
        .visible(false)
        .build();
    let window = match built {
        Ok(w) => w,
        Err(_) => {
            eprintln!("satchel: cannot open the settings window");
            *slot.lock().unwrap_or_else(|p| p.into_inner()) = None;
            return;
        }
    };

    let saved = app
        .try_state::<Arc<crate::ctx::Ctx>>()
        .and_then(|ctx| ctx.store.lock().window_state(STATE_KEY).ok().flatten())
        .and_then(|stored| crate::window_frame::decode(&stored));
    place(&window, saved);

    // The latest frame, written once more when the window is destroyed.
    let last = Arc::new(Mutex::new(frame_of(&window)));
    let slot = slot.clone();
    let app_handle = app.clone();
    let tracked = window.clone();
    let saver = saver.clone_sender();
    window.on_window_event(move |event| match event {
        WindowEvent::Moved(_) | WindowEvent::Resized(_) => {
            if let Some(frame) = frame_of(&tracked) {
                *last.lock().unwrap_or_else(|p| p.into_inner()) = Some(frame);
                saver.note(frame);
            }
        }
        WindowEvent::Destroyed => {
            if let Some(frame) = *last.lock().unwrap_or_else(|p| p.into_inner()) {
                saver.flush(frame);
            }
            // The settings window asks for hundreds of icons; give their memory back.
            // The shelf keeps its own few (one per source app) as blob URLs, so it
            // does not need these.
            if let Some(ctx) = app_handle.try_state::<Arc<crate::ctx::Ctx>>() {
                ctx.app_icons.clear();
            }
            let return_to = slot.lock().unwrap_or_else(|p| p.into_inner()).take();
            restore_focus(return_to);
        }
        _ => {}
    });
    let _ = window.show();
    // Activates Satchel and makes the window key, so it takes typing.
    let _ = window.set_focus();
}

fn save_frame<R: Runtime>(app: &AppHandle<R>, frame: Rect) {
    if let Some(ctx) = app.try_state::<Arc<crate::ctx::Ctx>>() {
        let _ = ctx.store.lock().save_window_state(STATE_KEY, &crate::window_frame::encode(&frame));
    }
}

/// The window's frame in AppKit screen coordinates.
#[cfg(target_os = "macos")]
fn frame_of<R: Runtime>(window: &WebviewWindow<R>) -> Option<Rect> {
    let mtm = objc2::MainThreadMarker::new()?;
    let ns_window = window.ns_window().ok()?;
    let (x, y, width, height) = crate::platform::macos::window_frame(mtm, ns_window)?;
    Some(Rect::new(x, y, width, height))
}

/// Moves the window to its saved frame, brought back on screen and within
/// the size limits; without one it stays centred. Also keeps it out of full
/// screen.
#[cfg(target_os = "macos")]
fn place<R: Runtime>(window: &WebviewWindow<R>, saved: Option<Rect>) {
    use crate::platform::macos;
    use crate::window_frame::{Display, Limits, fallback_display, restore};

    let Some(mtm) = objc2::MainThreadMarker::new() else { return };
    let Ok(ns_window) = window.ns_window() else { return };
    macos::disallow_full_screen(mtm, ns_window);
    let (Some(saved), Some(current)) = (saved, frame_of(window)) else { return };
    // The limits are for the content; the frame adds the title bar.
    let Ok(inner) = window.inner_size() else { return };
    let inner = inner.to_logical::<f64>(window.scale_factor().unwrap_or(1.0));
    let (dw, dh) = ((current.width - inner.width).max(0.0), (current.height - inner.height).max(0.0));
    let limits = Limits { min: (MIN_WIDTH + dw, MIN_HEIGHT + dh), max: (MAX_WIDTH + dw, MAX_HEIGHT + dh) };
    let rect = |(x, y, w, h): macos::Frame| Rect::new(x, y, w, h);
    let displays: Vec<Display> = macos::displays(mtm)
        .into_iter()
        .map(|(frame, work_area)| Display { frame: rect(frame), work_area: rect(work_area) })
        .collect();
    let Some(fallback) = fallback_display(&displays, Some(macos::mouse_location(mtm))) else { return };
    let f = restore(saved, &displays, fallback, limits);
    macos::set_window_frame(mtm, ns_window, (f.x, f.y, f.width, f.height));
}

// TODO(windows, linux): save and restore the frame there too.
#[cfg(not(target_os = "macos"))]
fn frame_of<R: Runtime>(_window: &WebviewWindow<R>) -> Option<Rect> {
    None
}

#[cfg(not(target_os = "macos"))]
fn place<R: Runtime>(_window: &WebviewWindow<R>, _saved: Option<Rect>) {}

/// Runs on the main thread (window events are delivered there).
fn restore_focus(return_to: Option<i32>) {
    #[cfg(target_os = "macos")]
    {
        let Some(mtm) = objc2::MainThreadMarker::new() else { return };
        let active = crate::platform::macos::app_is_active(mtm);
        match after_close(active, return_to) {
            AfterClose::Leave => {}
            // macOS 14+ refuses a plain activation of another app unless
            // the active app yields to it first.
            AfterClose::Activate(pid) => crate::platform::macos::hand_focus_to(mtm, pid),
            AfterClose::Deactivate => crate::platform::macos::deactivate(mtm),
        }
    }
    // TODO(windows, linux): the OS moves focus to the next window itself.
    #[cfg(not(target_os = "macos"))]
    {
        let _ = return_to;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_goes_back_to_the_app_that_was_in_front() {
        assert_eq!(after_close(true, Some(42)), AfterClose::Activate(42));
    }

    #[test]
    fn focus_is_left_alone_when_the_user_already_switched_away() {
        assert_eq!(after_close(false, Some(42)), AfterClose::Leave);
        assert_eq!(after_close(false, None), AfterClose::Leave);
    }

    #[test]
    fn satchel_steps_aside_when_there_is_no_app_to_return_to() {
        assert_eq!(after_close(true, None), AfterClose::Deactivate);
    }
}
