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
    AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, PhysicalPosition, PhysicalSize, Runtime, WebviewUrl,
    WebviewWindow, WebviewWindowBuilder, WindowEvent,
};

use crate::ctx::{Ctx, ShelfControl};
use crate::events::SHELF_SHOWN;
use crate::paste;

pub const LABEL: &str = "shelf";
/// Shelf geometry, in logical points. The window spans the full width of the
/// screen's usable area and is transparent; the UI draws the rounded panel
/// inside it, `PANEL_INSET` from the left, right and bottom edges and
/// `PANEL_TOP` from the top, and the native glass / blur view is framed to
/// exactly the same rectangle (`platform::macos::fit_to_panel`).
///
/// The margins are where the panel's CSS shadow is drawn, so they bound it:
/// anything the shadow paints past the window edge is cut off in a hard
/// line. The shadow in `src/routes/+page.svelte` (`.panel`) is shaped to fade
/// out within them: about 12 pt at the sides and bottom (the screen edge and
/// the Dock leave no more room there without moving the panel) and about
/// 22 pt above, over the windows the shelf floats on. Mirrored in
/// `src/app.css` as `--panel-inset`, `--panel-top` and `--shelf-height`; a
/// test checks they agree.
pub const PANEL_INSET: f64 = 12.0;
/// Transparent room above the panel for its upward shadow.
pub const PANEL_TOP: f64 = 24.0;
/// The panel: a 64 pt top bar over the 262 pt card row, plus its border.
pub const PANEL_HEIGHT: f64 = 328.0;
/// Logical window height: the panel plus its top and bottom margins.
pub const HEIGHT: f64 = PANEL_TOP + PANEL_HEIGHT + PANEL_INSET;
pub const CORNER_RADIUS: f64 = 28.0;

/// The native material behind the shelf panel. The UI is told which one it
/// has (see `init_script`) and makes its panel tint nearly clear over glass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)] // each platform constructs only its own variants
pub enum Background {
    /// Liquid Glass (`NSGlassEffectView`), macOS 26+.
    Glass,
    /// Frosted blur (`NSVisualEffectView` via Tauri's window effects),
    /// macOS 11–15.
    Blur,
    /// No native material (Windows, Linux): the UI's panel colour only.
    None,
}

impl Background {
    /// Picks the material this system supports.
    fn detect() -> Self {
        #[cfg(target_os = "macos")]
        {
            if crate::platform::macos::liquid_glass_available() { Self::Glass } else { Self::Blur }
        }
        #[cfg(not(target_os = "macos"))]
        {
            Self::None
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Glass => "glass",
            Self::Blur => "blur",
            Self::None => "none",
        }
    }
}

/// Initialization script for the shelf's web view: runs before the page's
/// own scripts and tells it which background it sits on. A WKUserScript /
/// WebView2 init script, so the page's CSP does not apply to it.
fn init_script(background: Background) -> String {
    format!("window.__SATCHEL_SHELF_BACKGROUND__ = {:?};", background.as_str())
}

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

/// A display's bounds in its own physical pixels (as Tauri reports a
/// monitor's position and size) and its scale factor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Display {
    pub bounds: Rect,
    pub scale: f64,
}

/// Index of the display under `cursor`, or of the nearest one when the
/// cursor is on no display (it can sit on a far edge, outside the half-open
/// bounds). `None` only when there are no displays.
///
/// Compared in logical points: `cursor` is in physical pixels at
/// `cursor_scale`, each display's bounds at its own scale. On macOS Tauri
/// (tao) reports the cursor at the main display's scale and every monitor at
/// its own, and `monitor_from_point` compares that physical cursor against
/// bounds in points: on a 2x main display the right half of the screen
/// "belongs" to whatever display lies at twice the cursor's x.
pub fn display_under(cursor: (f64, f64), cursor_scale: f64, displays: &[Display]) -> Option<usize> {
    let (cx, cy) = (cursor.0 / cursor_scale, cursor.1 / cursor_scale);
    let distance = |d: &Display| {
        let s = d.scale;
        let (left, top) = (d.bounds.x as f64 / s, d.bounds.y as f64 / s);
        let (right, bottom) = (left + d.bounds.width as f64 / s, top + d.bounds.height as f64 / s);
        if (left..right).contains(&cx) && (top..bottom).contains(&cy) {
            return 0.0;
        }
        let dx = (left - cx).max(cx - right).max(0.0);
        let dy = (top - cy).max(cy - bottom).max(0.0);
        // Never zero: an edge point outside the half-open bounds must not tie
        // with the display it really is on.
        dx * dx + dy * dy + f64::MIN_POSITIVE
    };
    displays
        .iter()
        .map(distance)
        .enumerate()
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
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
    /// The blur view has been inset to the panel shape (always true on the
    /// glass path, which is framed when it is installed).
    glass_fitted: AtomicBool,
    last_rect: Mutex<Option<Rect>>,
}

impl<R: Runtime> Shelf<R> {
    /// Creates the hidden shelf. Must run on the main thread (Tauri setup).
    pub fn create(app: &AppHandle<R>) -> tauri::Result<Self> {
        let _ = MAIN_THREAD.set(std::thread::current().id());
        TRACE.store(std::env::var_os("SATCHEL_TRACE").is_some(), Ordering::Relaxed);

        let background = Background::detect();
        let mut builder = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::default())
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
            .initialization_script(init_script(background));
        if background == Background::Blur {
            // macOS 11–15: frosted glass behind the transparent web view
            // through Tauri's built-in effects (the official window-vibrancy
            // crate): an NSVisualEffectView, inset to the UI's panel shape by
            // `fit_glass`. `Active` keeps the material lit although Satchel is
            // never the active app.
            //
            // macOS 26+ gets Liquid Glass instead, installed by
            // `platform::macos::install_liquid_glass` below rather than by
            // window-vibrancy: its glass path re-parents the web view into the
            // glass view, so the glass could not be inset to the panel. Only
            // one of the two materials is ever installed.
            //
            // No effect is requested elsewhere: Linux ignores it and Windows
            // would blur the whole transparent window.
            builder = builder.effects(WindowEffectsConfig {
                effects: vec![Effect::Popover],
                state: Some(EffectState::Active),
                radius: Some(CORNER_RADIUS),
                color: None,
                interactive: false,
            });
        }
        let window = builder.build()?;

        #[cfg(target_os = "macos")]
        let panel = {
            let mtm = objc2::MainThreadMarker::new().expect("setup runs on the main thread");
            let ns_window = window.ns_window()?;
            crate::platform::macos::make_shelf_panel(mtm, ns_window)
        };
        #[cfg(not(target_os = "macos"))]
        let panel = false;

        // Created once with the window, never per show.
        #[cfg(target_os = "macos")]
        if background == Background::Glass {
            let mtm = objc2::MainThreadMarker::new().expect("setup runs on the main thread");
            let installed =
                crate::platform::macos::install_liquid_glass(
                mtm,
                window.ns_window()?,
                PANEL_INSET,
                PANEL_TOP,
                CORNER_RADIUS,
            );
            // Cannot fail once availability was checked (it needs only the
            // content view). If it ever does, the panel's own tint still
            // keeps the text legible, over no material.
            if TRACE.load(Ordering::Relaxed) {
                eprintln!("satchel: liquid glass installed: {installed}");
            }
        }

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
            glass_fitted: AtomicBool::new(background != Background::Blur),
            last_rect: Mutex::new(None),
        };
        shelf.fit_glass();
        Ok(shelf)
    }

    /// Insets the blur view to the UI panel (macOS 11–15). Retried on show
    /// in case the effect was applied after creation.
    fn fit_glass(&self) {
        if self.glass_fitted.load(Ordering::Relaxed) {
            return;
        }
        #[cfg(target_os = "macos")]
        if let (Some(mtm), Ok(ns_window)) = (objc2::MainThreadMarker::new(), self.window.ns_window()) {
            let fitted = crate::platform::macos::inset_effect_view(mtm, ns_window, PANEL_INSET, PANEL_TOP);
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
        // Not `monitor_from_point`: it mixes units on macOS (see
        // `display_under`).
        let monitors = app.available_monitors().unwrap_or_default();
        let primary = app.primary_monitor().ok().flatten();
        // tao reports the cursor at the main display's scale on macOS;
        // elsewhere the cursor and monitors share one physical space.
        let (cursor_scale, logical) = if cfg!(target_os = "macos") {
            (primary.as_ref().map_or(1.0, |m| m.scale_factor()), true)
        } else {
            (1.0, false)
        };
        let displays: Vec<Display> = monitors
            .iter()
            .map(|m| Display {
                bounds: Rect {
                    x: m.position().x,
                    y: m.position().y,
                    width: m.size().width,
                    height: m.size().height,
                },
                scale: if logical { m.scale_factor() } else { 1.0 },
            })
            .collect();
        let cursor = app.cursor_position().ok();
        let index = cursor.and_then(|c| display_under((c.x, c.y), cursor_scale, &displays));
        if TRACE.load(Ordering::Relaxed) {
            eprintln!("satchel: cursor {cursor:?} at scale {cursor_scale}; displays {displays:?}; chose {index:?}");
        }
        let monitor = index.and_then(|i| monitors.get(i).cloned()).or(primary);
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
            if cfg!(target_os = "macos") {
                // tao turns physical values into points with the window's
                // *current* scale, which is the old display's when the shelf
                // moves between displays of different scales: hand it points.
                let s = monitor.scale_factor();
                let _ = self.window.set_size(LogicalSize::new(rect.width as f64 / s, rect.height as f64 / s));
                let _ = self.window.set_position(LogicalPosition::new(rect.x as f64 / s, rect.y as f64 / s));
            } else {
                let _ = self.window.set_size(PhysicalSize::new(rect.width, rect.height));
                let _ = self.window.set_position(PhysicalPosition::new(rect.x, rect.y));
            }
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
    fn init_script_tells_the_page_its_background() {
        assert_eq!(init_script(Background::Glass), r#"window.__SATCHEL_SHELF_BACKGROUND__ = "glass";"#);
        assert_eq!(init_script(Background::Blur), r#"window.__SATCHEL_SHELF_BACKGROUND__ = "blur";"#);
        assert_eq!(init_script(Background::None), r#"window.__SATCHEL_SHELF_BACKGROUND__ = "none";"#);
    }

    #[test]
    fn spans_the_bottom_of_a_retina_screen() {
        // 1512x982 pt MacBook screen at 2x, menu bar 37 pt (74 px), no Dock.
        let work = Rect { x: 0, y: 74, width: 3024, height: 1890 };
        let r = dock_rect(work, 2.0);
        assert_eq!(r, Rect { x: 0, y: 74 + 1890 - 728, width: 3024, height: 728 });
    }

    #[test]
    fn respects_the_origin_of_secondary_screens() {
        let work = Rect { x: -1920, y: -200, width: 1920, height: 1080 };
        let r = dock_rect(work, 1.0);
        assert_eq!(r.x, -1920);
        assert_eq!(r.y + r.height as i32, -200 + 1080);
        assert_eq!((r.width, r.height), (1920, 364));
    }

    #[test]
    fn the_window_is_the_panel_plus_its_shadow_margins() {
        assert_eq!(HEIGHT, PANEL_TOP + PANEL_HEIGHT + PANEL_INSET);
        assert_eq!(HEIGHT, 364.0);
        // Room for the upward shadow, and some above the panel at all.
        const { assert!(PANEL_TOP > PANEL_INSET) };
    }

    #[test]
    fn the_ui_sizes_its_panel_from_the_same_height_and_inset() {
        // The panel is `--shelf-height - --panel-top - --panel-inset` tall
        // and the native glass / blur view is the window minus `PANEL_TOP`
        // and `PANEL_INSET`: they only match if the CSS mirrors these
        // constants.
        let css = include_str!("../../src/app.css");
        assert!(css.contains(&format!("--panel-top: {PANEL_TOP}px;")), "--panel-top in src/app.css must be {PANEL_TOP}px");
        assert!(css.contains(&format!("--shelf-height: {HEIGHT}px;")), "--shelf-height in src/app.css must be {HEIGHT}px");
        assert!(css.contains(&format!("--panel-inset: {PANEL_INSET}px;")), "--panel-inset in src/app.css must be {PANEL_INSET}px");
        assert!(css.contains(&format!("--panel-radius: {CORNER_RADIUS}px;")), "--panel-radius in src/app.css must be {CORNER_RADIUS}px");
    }

    #[test]
    fn never_taller_than_the_screen() {
        let work = Rect { x: 0, y: 0, width: 800, height: 200 };
        let r = dock_rect(work, 1.0);
        assert_eq!((r.y, r.height), (0, 200));
    }

    /// A display at `(x, y)` points, `w`x`h` points, reported as tao does:
    /// bounds in its own physical pixels.
    fn display(x: f64, y: f64, w: f64, h: f64, scale: f64) -> Display {
        Display {
            bounds: Rect {
                x: (x * scale) as i32,
                y: (y * scale) as i32,
                width: (w * scale) as u32,
                height: (h * scale) as u32,
            },
            scale,
        }
    }

    #[test]
    fn right_half_of_a_2x_main_display_is_still_the_main_display() {
        // The reported bug: 2560x1440 pt main at 2x, a 1512x982 pt Retina
        // to its right, bottom-aligned. The cursor comes at the main scale.
        let displays = [display(0.0, 0.0, 2560.0, 1440.0, 2.0), display(2560.0, 458.0, 1512.0, 982.0, 2.0)];
        for x in [0.0, 600.0, 1200.0, 1300.0, 2000.0, 2559.5] {
            assert_eq!(display_under((x * 2.0, 700.0 * 2.0), 2.0, &displays), Some(0), "x = {x} pt");
        }
        for x in [2560.0, 3000.0, 4071.0] {
            assert_eq!(display_under((x * 2.0, 1000.0 * 2.0), 2.0, &displays), Some(1), "x = {x} pt");
        }
    }

    #[test]
    fn mixed_scales_compare_in_points() {
        // 2x main, 1x external to its right. Cursor at the main scale (2x).
        let displays = [display(0.0, 0.0, 1512.0, 982.0, 2.0), display(1512.0, 0.0, 1920.0, 1080.0, 1.0)];
        assert_eq!(display_under((1000.0 * 2.0, 500.0 * 2.0), 2.0, &displays), Some(0));
        assert_eq!(display_under((1600.0 * 2.0, 500.0 * 2.0), 2.0, &displays), Some(1));
        assert_eq!(display_under((3400.0 * 2.0, 1000.0 * 2.0), 2.0, &displays), Some(1));
        // 1x main, 2x Retina to its right: cursor at 1x.
        let displays = [display(0.0, 0.0, 1920.0, 1080.0, 1.0), display(1920.0, 0.0, 1512.0, 982.0, 2.0)];
        assert_eq!(display_under((1900.0, 500.0), 1.0, &displays), Some(0));
        assert_eq!(display_under((2000.0, 500.0), 1.0, &displays), Some(1));
        assert_eq!(display_under((3400.0, 900.0), 1.0, &displays), Some(1));
    }

    #[test]
    fn displays_to_the_left_and_above_have_negative_origins() {
        let displays = [
            display(0.0, 0.0, 1512.0, 982.0, 2.0),
            display(-1920.0, -98.0, 1920.0, 1080.0, 1.0),
            display(0.0, -1440.0, 2560.0, 1440.0, 2.0),
        ];
        assert_eq!(display_under((-2.0, 10.0 * 2.0), 2.0, &displays), Some(1));
        assert_eq!(display_under((-1920.0 * 2.0, -98.0 * 2.0), 2.0, &displays), Some(1));
        assert_eq!(display_under((10.0 * 2.0, 10.0 * 2.0), 2.0, &displays), Some(0));
        assert_eq!(display_under((2000.0 * 2.0, -2.0), 2.0, &displays), Some(2));
        assert_eq!(display_under((100.0 * 2.0, -1440.0 * 2.0), 2.0, &displays), Some(2));
    }

    #[test]
    fn a_cursor_on_no_display_picks_the_nearest() {
        let displays = [display(0.0, 0.0, 1512.0, 982.0, 2.0), display(1512.0, 0.0, 1920.0, 1080.0, 1.0)];
        // The far bottom-right corner sits outside the half-open bounds.
        assert_eq!(display_under((3432.0 * 2.0, 1080.0 * 2.0), 2.0, &displays), Some(1));
        // Below the shorter main display: nearer to it than to the external.
        assert_eq!(display_under((100.0 * 2.0, 1000.0 * 2.0), 2.0, &displays), Some(0));
        assert_eq!(display_under((0.0, 0.0), 1.0, &[]), None);
    }

    #[test]
    fn one_physical_space_works_with_unit_scales() {
        // Windows and Linux: cursor and monitors already share physical pixels.
        let displays = [display(0.0, 0.0, 3840.0, 2160.0, 1.0), display(3840.0, 0.0, 1920.0, 1080.0, 1.0)];
        assert_eq!(display_under((3000.0, 1000.0), 1.0, &displays), Some(0));
        assert_eq!(display_under((4000.0, 1000.0), 1.0, &displays), Some(1));
    }

    #[test]
    fn degenerate_screens_do_not_panic() {
        let r = dock_rect(Rect { x: 0, y: 0, width: 0, height: 0 }, 3.0);
        assert!(r.width >= 1 && r.height >= 1);
    }
}
