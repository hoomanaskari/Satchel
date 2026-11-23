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

use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Manager, Runtime, WebviewUrl, WebviewWindowBuilder, WindowEvent};

use crate::ctx::SettingsWindowControl;

pub const LABEL: &str = "settings";
/// The SvelteKit route the window loads (emitted as `settings.html`).
pub const ROUTE: &str = "settings";
pub const WIDTH: f64 = 680.0;
pub const HEIGHT: f64 = 520.0;

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
}

impl<R: Runtime> TauriSettingsWindow<R> {
    pub fn new(app: AppHandle<R>) -> Self {
        Self { app, return_to: Arc::new(Mutex::new(None)) }
    }
}

impl<R: Runtime> SettingsWindowControl for TauriSettingsWindow<R> {
    fn open(&self, return_to: Option<i32>) {
        let app = self.app.clone();
        let slot = self.return_to.clone();
        let _ = self.app.run_on_main_thread(move || open_on_main(&app, &slot, return_to));
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

fn open_on_main<R: Runtime>(app: &AppHandle<R>, slot: &Arc<Mutex<Option<i32>>>, return_to: Option<i32>) {
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
        .resizable(false)
        .maximizable(false)
        .minimizable(true)
        .closable(true)
        .center()
        .focused(true)
        .visible(true)
        .build();
    let window = match built {
        Ok(w) => w,
        Err(_) => {
            eprintln!("satchel: cannot open the settings window");
            *slot.lock().unwrap_or_else(|p| p.into_inner()) = None;
            return;
        }
    };

    let slot = slot.clone();
    window.on_window_event(move |event| {
        if let WindowEvent::Destroyed = event {
            let return_to = slot.lock().unwrap_or_else(|p| p.into_inner()).take();
            restore_focus(return_to);
        }
    });
    // Activates Satchel and makes the window key, so it takes typing.
    let _ = window.set_focus();
}

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
