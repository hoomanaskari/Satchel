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

/// Where the shelf goes on a screen whose full frame is `screen` and whose
/// usable area (without menu bar and Dock) is `work`, in physical pixels with
/// y pointing down. Like the Paste app it sits at the very bottom of the
/// screen, over the Dock (the panel's window level is above the Dock's), and
/// spans the whole width, also over a Dock on the left or right. Only the top
/// of the usable area matters: on a short screen the shelf never reaches
/// into the menu bar.
pub fn dock_rect(screen: Rect, work: Rect, scale: f64) -> Rect {
    let bottom = screen.y as i64 + screen.height as i64;
    let room = (bottom - (work.y as i64).max(screen.y as i64)).max(1);
    let height = ((HEIGHT * scale).round() as i64).min(room).max(1);
    Rect {
        x: screen.x,
        y: (bottom - height) as i32,
        width: screen.width.max(1),
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

/// Slide-in when the shelf opens: from just below the bottom edge of the
/// screen up into place, easing out (macOS's own ease-out for sheets and
/// panels). The window stays opaque: Liquid Glass ignores window alpha, so a
/// fade would show full-strength glass under fading content.
pub const SHOW_SECONDS: f64 = 0.22;
pub const SHOW_CURVE: [f32; 4] = [0.25, 0.8, 0.35, 1.0];
/// The slide-in starts this long after the window is ordered in (just below
/// the screen, already key so typing works at once), so WebKit has repainted
/// the page by the time the panel comes into view (see
/// `platform::macos::keep_painting_off_screen`).
pub const SHOW_DELAY: Duration = Duration::from_millis(50);
/// Slide-out when it is dismissed: shorter, easing in (it accelerates away).
pub const HIDE_SECONDS: f64 = 0.16;
pub const HIDE_CURVE: [f32; 4] = [0.45, 0.0, 0.85, 0.4];
/// With "Reduce motion" on there is no slide: the shelf appears and goes at
/// once (a fade is out for the same reason as above).
pub const LINEAR: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

/// What `show` has to do, given where the shelf is in its motion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShowKind {
    /// It is hidden: place it and slide it in from below.
    FromHidden,
    /// It is sliding out: turn round and slide back up from where it is.
    Reverse,
    /// It is already up (or on its way up): nothing moves.
    Already,
}

/// The shelf's show / hide state across animations. Every change bumps
/// `generation`, so a slide-out that finishes after the user has already
/// asked for the shelf again is ignored instead of hiding it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Motion {
    shown: bool,
    hiding: bool,
    generation: u64,
}

impl Motion {
    /// Up, or on its way up (not hidden or sliding out).
    pub fn is_shown(&self) -> bool {
        self.shown && !self.hiding
    }

    pub fn show(&mut self) -> ShowKind {
        let kind = match (self.shown, self.hiding) {
            (true, false) => ShowKind::Already,
            (true, true) => ShowKind::Reverse,
            _ => ShowKind::FromHidden,
        };
        self.shown = true;
        self.hiding = false;
        self.generation += 1;
        kind
    }

    /// Starts a slide-out; returns its generation, or `None` when the shelf
    /// is already hidden or on its way out.
    pub fn hide(&mut self) -> Option<u64> {
        if !self.is_shown() {
            return None;
        }
        self.hiding = true;
        self.generation += 1;
        Some(self.generation)
    }

    /// The slide-out `generation` has ended: true if the window should now
    /// really be ordered out (nothing happened since).
    pub fn finish_hide(&mut self, generation: u64) -> bool {
        if self.hiding && generation == self.generation {
            self.shown = false;
            self.hiding = false;
            true
        } else {
            false
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Hidden at once, cancelling any animation (paste, settings).
    pub fn hide_now(&mut self) {
        self.shown = false;
        self.hiding = false;
        self.generation += 1;
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
    /// The blur view has been inset to the panel shape (always true on the
    /// glass path, which is framed when it is installed).
    glass_fitted: AtomicBool,
    last_rect: Mutex<Option<Rect>>,
    motion: Mutex<Motion>,
    /// Where the window rests when it is up (AppKit points), captured on
    /// each show: the target of slide-ins and the origin of slide-outs.
    #[cfg(target_os = "macos")]
    home: Mutex<Option<crate::platform::macos::Frame>>,
}

impl<R: Runtime> Shelf<R> {
    /// Creates the hidden shelf. Must run on the main thread (Tauri setup).
    pub fn create(app: &AppHandle<R>) -> tauri::Result<Self> {
        let _ = MAIN_THREAD.set(std::thread::current().id());
        TRACE.store(std::env::var_os("SATCHEL_TRACE").is_some(), Ordering::Relaxed);

        let background = Background::detect();
        let mut builder = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::default())
            .title(crate::app_identity::name())
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
            motion: Mutex::new(Motion::default()),
            #[cfg(target_os = "macos")]
            home: Mutex::new(None),
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

    /// Up or on its way up (a shelf sliding out counts as hidden, so the
    /// shortcut brings it back).
    pub fn is_shown(&self) -> bool {
        self.motion().is_shown()
    }

    fn motion(&self) -> std::sync::MutexGuard<'_, Motion> {
        self.motion.lock().unwrap_or_else(|p| p.into_inner())
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
        let screen = Rect {
            x: monitor.position().x,
            y: monitor.position().y,
            width: monitor.size().width,
            height: monitor.size().height,
        };
        let rect = dock_rect(screen, work, monitor.scale_factor());
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
    ///
    /// The window is ordered in and made key before it starts to move, so
    /// typing and Return work from the first frame; the slide itself runs on
    /// the main run loop and never delays this call.
    pub fn show(&self, app: &AppHandle<R>, ctx: &Ctx) {
        let started = Instant::now();
        let kind = self.motion().show();
        if kind == ShowKind::FromHidden {
            ctx.remember_previous_app(ctx.desktop.frontmost_app().map(|a| a.pid));
            self.place(app);
            self.fit_glass();
        }
        #[cfg(target_os = "macos")]
        if self.panel
            && let (Some(mtm), Ok(ns_window)) = (objc2::MainThreadMarker::new(), self.window.ns_window())
        {
            use crate::platform::macos::{animate_window, reduce_motion, show_panel, window_frame};
            let webview = self.webview.load(Ordering::SeqCst) as *mut std::ffi::c_void;
            let mut home = self.home.lock().unwrap_or_else(|p| p.into_inner());
            if kind == ShowKind::FromHidden {
                *home = window_frame(mtm, ns_window);
            }
            if let Some(rest) = *home
                && kind != ShowKind::Already
            {
                let seconds = if reduce_motion() { 0.0 } else { SHOW_SECONDS };
                // Also with Reduce Motion: it then jumps into place after the
                // delay, so it never shows the empty glass for a frame.
                if kind == ShowKind::FromHidden {
                    crate::platform::macos::keep_painting_off_screen(mtm, webview);
                    animate_window(mtm, ns_window, below(rest), 1.0, 0.0, LINEAR);
                    show_panel(mtm, ns_window, webview);
                    let generation = self.motion().generation();
                    let app = app.clone();
                    std::thread::spawn(move || {
                        std::thread::sleep(SHOW_DELAY);
                        let handle = app.clone();
                        let _ = app.run_on_main_thread(move || {
                            if let Some(shelf) = handle.try_state::<Shelf<R>>() {
                                shelf.slide_in(generation, seconds);
                            }
                        });
                    });
                } else {
                    show_panel(mtm, ns_window, webview);
                    animate_window(mtm, ns_window, rest, 1.0, seconds, SHOW_CURVE);
                }
            } else {
                show_panel(mtm, ns_window, webview);
            }
        }
        if !self.panel {
            let _ = self.window.show();
            let _ = self.window.set_focus();
        }
        if kind != ShowKind::Already {
            let _ = app.emit(SHELF_SHOWN, ());
        }
        if TRACE.load(Ordering::Relaxed) {
            eprintln!("satchel: shelf shown ({kind:?}) in {} us", started.elapsed().as_micros());
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

    /// Starts the delayed slide-in of show `generation`, unless the shelf
    /// was hidden or shown again in the meantime.
    #[cfg(target_os = "macos")]
    fn slide_in(&self, generation: u64, seconds: f64) {
        if self.motion().generation() != generation {
            return;
        }
        let rest = *self.home.lock().unwrap_or_else(|p| p.into_inner());
        if let (Some(rest), Some(mtm), Ok(ns_window)) =
            (rest, objc2::MainThreadMarker::new(), self.window.ns_window())
        {
            crate::platform::macos::animate_window(mtm, ns_window, rest, 1.0, seconds, SHOW_CURVE);
        }
    }

    /// Hides at once, cancelling any slide (paste: the keystroke must not
    /// wait for an animation, and the shelf must be gone before it is sent).
    fn hide_now(&self) {
        self.motion().hide_now();
        self.order_out();
    }

    /// Slides the shelf out of sight, then orders it out. Returns at once.
    /// Main thread only.
    fn dismiss(&self, app: &AppHandle<R>) {
        #[cfg(target_os = "macos")]
        if self.panel
            && let (Some(mtm), Ok(ns_window)) = (objc2::MainThreadMarker::new(), self.window.ns_window())
        {
            let Some(generation) = self.motion().hide() else { return };
            let home = *self.home.lock().unwrap_or_else(|p| p.into_inner());
            let rest = home.or_else(|| crate::platform::macos::window_frame(mtm, ns_window));
            let Some(rest) = rest else {
                self.hide_now();
                return;
            };
            if crate::platform::macos::reduce_motion() {
                self.hide_now();
                return;
            }
            let seconds = HIDE_SECONDS;
            crate::platform::macos::animate_window(mtm, ns_window, below(rest), 1.0, seconds, HIDE_CURVE);
            let app = app.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs_f64(seconds) + Duration::from_millis(10));
                let handle = app.clone();
                let _ = app.run_on_main_thread(move || {
                    if let Some(shelf) = handle.try_state::<Shelf<R>>()
                        && shelf.motion().finish_hide(generation)
                    {
                        shelf.order_out();
                    }
                });
            });
            return;
        }
        let _ = app;
        if self.motion().hide().is_some() {
            self.hide_now();
        }
    }

    /// Orders the window out and puts it back at rest, opaque, for the next
    /// show (it is out of sight by then, so nothing visibly jumps).
    fn order_out(&self) {
        let _ = self.window.hide();
        #[cfg(target_os = "macos")]
        if self.panel
            && let (Some(mtm), Ok(ns_window)) = (objc2::MainThreadMarker::new(), self.window.ns_window())
            && let Some(rest) = *self.home.lock().unwrap_or_else(|p| p.into_inner())
        {
            crate::platform::macos::animate_window(mtm, ns_window, rest, 1.0, 0.0, LINEAR);
        }
        if TRACE.load(Ordering::Relaxed) {
            eprintln!("satchel: shelf hidden");
            self.trace_page();
        }
    }

    /// Shortcut / tray behavior: show, or dismiss when already up. Pressed
    /// again while the shelf slides out, it slides back up from there.
    pub fn toggle(&self, app: &AppHandle<R>, ctx: &Ctx) {
        if self.is_shown() {
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
            // Also fires when the slide-out orders the window out; the
            // motion state makes that a no-op.
            if let WindowEvent::Focused(false) = event
                && let Some(shelf) = handle.try_state::<Shelf<R>>()
            {
                shelf.dismiss(&handle);
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
fn hide_and_hand_back<R: Runtime>(app: &AppHandle<R>, animate: bool) {
    if let Some(shelf) = app.try_state::<Shelf<R>>() {
        if animate {
            shelf.dismiss(app);
        } else {
            shelf.hide_now();
        }
    }
    #[cfg(target_os = "macos")]
    if let (Some(mtm), Some(ctx)) = (objc2::MainThreadMarker::new(), app.try_state::<std::sync::Arc<Ctx>>())
        && let Some(pid) = ctx.previous_app()
    {
        crate::platform::macos::hand_focus_to(mtm, pid);
    }
}

impl<R: Runtime> TauriShelf<R> {
    fn run(&self, animate: bool) {
        if on_main_thread() {
            hide_and_hand_back(&self.app, animate);
            return;
        }
        let (tx, rx) = mpsc::channel();
        let app = self.app.clone();
        let sent = self.app.run_on_main_thread(move || {
            hide_and_hand_back(&app, animate);
            let _ = tx.send(());
        });
        if sent.is_ok() {
            let _ = rx.recv_timeout(Duration::from_secs(1));
        }
    }
}

impl<R: Runtime> ShelfControl for TauriShelf<R> {
    fn hide(&self) {
        self.run(false);
    }

    fn dismiss(&self) {
        self.run(true);
    }
}

/// The resting frame moved down by its own height: the window's top edge
/// at the bottom of the screen, the panel and its shadow out of sight.
#[cfg(target_os = "macos")]
fn below(rest: crate::platform::macos::Frame) -> crate::platform::macos::Frame {
    (rest.0, rest.1 - rest.3, rest.2, rest.3)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_show_slides_in_and_a_second_does_nothing() {
        let mut m = Motion::default();
        assert!(!m.is_shown());
        assert_eq!(m.show(), ShowKind::FromHidden);
        assert!(m.is_shown());
        assert_eq!(m.show(), ShowKind::Already);
    }

    #[test]
    fn a_slide_out_ends_hidden() {
        let mut m = Motion::default();
        m.show();
        let g = m.hide().expect("a shown shelf slides out");
        assert!(!m.is_shown(), "sliding out counts as hidden for the shortcut");
        assert_eq!(m.hide(), None, "a second dismiss does not restart it");
        assert!(m.finish_hide(g));
        assert_eq!(m.show(), ShowKind::FromHidden);
    }

    #[test]
    fn showing_mid_slide_out_reverses_and_the_stale_finish_is_ignored() {
        let mut m = Motion::default();
        m.show();
        let g = m.hide().unwrap();
        assert_eq!(m.show(), ShowKind::Reverse);
        assert!(!m.finish_hide(g), "the old slide-out must not hide the shelf");
        assert!(m.is_shown());
        // Toggle again: a new slide-out, which does finish.
        let g2 = m.hide().unwrap();
        assert!(!m.finish_hide(g));
        assert!(m.finish_hide(g2));
        assert!(!m.is_shown());
    }

    #[test]
    fn hiding_at_once_cancels_a_slide_out() {
        let mut m = Motion::default();
        m.show();
        let g = m.hide().unwrap();
        m.hide_now();
        assert!(!m.finish_hide(g));
        assert_eq!(m.hide(), None, "nothing to slide out");
        assert_eq!(m.show(), ShowKind::FromHidden);
    }

    #[test]
    fn a_hidden_shelf_has_nothing_to_dismiss() {
        assert_eq!(Motion::default().hide(), None);
    }

    #[test]
    fn the_slide_is_short_and_out_is_faster_than_in() {
        assert!((0.18..=0.24).contains(&SHOW_SECONDS));
        const { assert!(HIDE_SECONDS < SHOW_SECONDS) };
    }

    #[test]
    fn init_script_tells_the_page_its_background() {
        assert_eq!(init_script(Background::Glass), r#"window.__SATCHEL_SHELF_BACKGROUND__ = "glass";"#);
        assert_eq!(init_script(Background::Blur), r#"window.__SATCHEL_SHELF_BACKGROUND__ = "blur";"#);
        assert_eq!(init_script(Background::None), r#"window.__SATCHEL_SHELF_BACKGROUND__ = "none";"#);
    }

    #[test]
    fn spans_the_bottom_of_a_retina_screen() {
        // 1512x982 pt MacBook screen at 2x, menu bar 37 pt (74 px), no Dock.
        let screen = Rect { x: 0, y: 0, width: 3024, height: 1964 };
        let work = Rect { x: 0, y: 74, width: 3024, height: 1890 };
        let r = dock_rect(screen, work, 2.0);
        assert_eq!(r, Rect { x: 0, y: 1964 - 728, width: 3024, height: 728 });
    }

    #[test]
    fn sits_over_a_dock_at_the_bottom() {
        // 2560x1440 pt at 2x, menu bar 37 pt, Dock 74 pt tall at the bottom:
        // the shelf ignores the Dock and reaches the screen's bottom edge.
        let screen = Rect { x: 0, y: 0, width: 5120, height: 2880 };
        let work = Rect { x: 0, y: 74, width: 5120, height: 2880 - 74 - 148 };
        let r = dock_rect(screen, work, 2.0);
        assert_eq!(r, Rect { x: 0, y: 2880 - 728, width: 5120, height: 728 });
    }

    #[test]
    fn spans_the_full_width_over_a_dock_on_the_side() {
        // Dock on the left (90 px) or right: the work area is narrower, the
        // shelf is not.
        let screen = Rect { x: 0, y: 0, width: 1920, height: 1080 };
        for work in [
            Rect { x: 90, y: 25, width: 1830, height: 1055 },
            Rect { x: 0, y: 25, width: 1830, height: 1055 },
        ] {
            let r = dock_rect(screen, work, 1.0);
            assert_eq!(r, Rect { x: 0, y: 1080 - 364, width: 1920, height: 364 });
        }
    }

    #[test]
    fn respects_the_origin_of_secondary_screens() {
        let screen = Rect { x: -1920, y: -200, width: 1920, height: 1080 };
        let work = Rect { x: -1920, y: -175, width: 1920, height: 1000 };
        let r = dock_rect(screen, work, 1.0);
        assert_eq!(r.x, -1920);
        assert_eq!(r.y + r.height as i32, -200 + 1080);
        assert_eq!((r.width, r.height), (1920, 364));
    }

    #[test]
    fn a_notched_screen_keeps_the_full_bottom() {
        // 14" MacBook Pro, 1512x982 pt at 2x with a 38 pt notch menu bar:
        // the notch only moves the top of the usable area.
        let screen = Rect { x: 0, y: 0, width: 3024, height: 1964 };
        let work = Rect { x: 0, y: 76, width: 3024, height: 1888 };
        let r = dock_rect(screen, work, 2.0);
        assert_eq!((r.y, r.height), (1964 - 728, 728));
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
    fn never_reaches_into_the_menu_bar() {
        let screen = Rect { x: 0, y: 0, width: 800, height: 300 };
        let work = Rect { x: 0, y: 25, width: 800, height: 200 };
        let r = dock_rect(screen, work, 1.0);
        assert_eq!((r.y, r.height), (25, 275));
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
        let zero = Rect { x: 0, y: 0, width: 0, height: 0 };
        let r = dock_rect(zero, zero, 3.0);
        assert!(r.width >= 1 && r.height >= 1);
    }
}
