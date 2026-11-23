//! Menu bar / tray icon: Show Satchel, Settings, Pause Capturing, Clear
//! History, Quit.

use std::sync::Arc;

use tauri::image::Image;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, Runtime};

use crate::commands::api;
use crate::ctx::Ctx;
use crate::shelf::Shelf;

pub const ID_SHOW: &str = "show";
pub const ID_SETTINGS: &str = "settings";
pub const ID_PAUSE: &str = "pause";
pub const ID_CLEAR: &str = "clear";
pub const ID_QUIT: &str = "quit";

/// Icon side in pixels: 18 pt at 2x, the menu bar size on Retina screens.
pub const ICON_SIZE: u32 = 36;

/// Pause item, kept so settings changes from the UI update its check mark.
pub struct PauseItem<R: Runtime>(pub CheckMenuItem<R>);

/// Coverage of a satchel glyph (flap bag with a handle) at `(x, y)` in a
/// unit square, y pointing down.
fn glyph(x: f64, y: f64) -> bool {
    // Handle: ring above the bag.
    let (hx, hy) = (0.5 - x, 0.36 - y);
    let ring = (hx * hx + hy * hy).sqrt();
    if y < 0.36 && (0.15..=0.24).contains(&ring) {
        return true;
    }
    // Body: rounded rectangle.
    let (left, right, top, bottom, r) = (0.1, 0.9, 0.34, 0.9, 0.1);
    if !(left..=right).contains(&x) || !(top..=bottom).contains(&y) {
        return false;
    }
    let cx = x.clamp(left + r, right - r);
    let cy = y.clamp(top + r, bottom - r);
    if (x - cx).powi(2) + (y - cy).powi(2) > r * r {
        return false;
    }
    // A flap seam is cut out, interrupted by the clasp, so it reads as a bag.
    let seam = (0.52..=0.58).contains(&y) && (0.16..=0.84).contains(&x);
    let clasp = (0.43..=0.57).contains(&x) && (0.48..=0.66).contains(&y);
    clasp || !seam
}

/// Black-on-transparent RGBA template image, 4x4 supersampled.
pub fn icon_rgba(size: u32) -> Vec<u8> {
    const SS: u32 = 4;
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for py in 0..size {
        for px in 0..size {
            let mut hits = 0u32;
            for sy in 0..SS {
                for sx in 0..SS {
                    let x = (px as f64 + (sx as f64 + 0.5) / SS as f64) / size as f64;
                    let y = (py as f64 + (sy as f64 + 0.5) / SS as f64) / size as f64;
                    hits += glyph(x, y) as u32;
                }
            }
            let alpha = (hits * 255 / (SS * SS)) as u8;
            out.extend_from_slice(&[0, 0, 0, alpha]);
        }
    }
    out
}

pub fn create<R: Runtime>(app: &AppHandle<R>, paused: bool) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, ID_SHOW, "Show Satchel", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, ID_SETTINGS, "Settings…", true, None::<&str>)?;
    let pause = CheckMenuItem::with_id(app, ID_PAUSE, "Pause Capturing", true, paused, None::<&str>)?;
    let clear = MenuItem::with_id(app, ID_CLEAR, "Clear History…", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, ID_QUIT, "Quit Satchel", true, Some("CmdOrCtrl+Q"))?;
    let menu = Menu::with_items(
        app,
        &[&show, &settings, &PredefinedMenuItem::separator(app)?, &pause, &PredefinedMenuItem::separator(app)?, &clear, &PredefinedMenuItem::separator(app)?, &quit],
    )?;
    app.manage(PauseItem(pause));

    TrayIconBuilder::with_id("satchel")
        .icon(Image::new_owned(icon_rgba(ICON_SIZE), ICON_SIZE, ICON_SIZE))
        .icon_as_template(true)
        .tooltip("Satchel")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| handle_menu(app, event.id().as_ref()))
        .build(app)?;
    Ok(())
}

fn handle_menu<R: Runtime>(app: &AppHandle<R>, id: &str) {
    let Some(ctx) = app.try_state::<Arc<Ctx>>() else { return };
    let ctx = ctx.inner().clone();
    match id {
        ID_SHOW => {
            if let Some(shelf) = app.try_state::<Shelf<R>>()
                && !shelf.is_visible()
            {
                shelf.show(app, &ctx);
            }
        }
        ID_SETTINGS => api::open_settings(&ctx),
        ID_PAUSE => {
            let paused = app
                .try_state::<PauseItem<R>>()
                .and_then(|item| item.0.is_checked().ok())
                .unwrap_or(!ctx.capture_paused());
            let _ = api::set_capture_paused(&ctx, paused);
        }
        ID_CLEAR => {
            if confirm_clear() {
                // The store call can take a moment; keep the main thread free.
                std::thread::spawn(move || {
                    let _ = api::clear_history(&ctx);
                });
            }
        }
        ID_QUIT => app.exit(0),
        _ => {}
    }
}

#[cfg(target_os = "macos")]
fn confirm_clear() -> bool {
    let Some(mtm) = objc2::MainThreadMarker::new() else { return false };
    crate::platform::macos::confirm(
        mtm,
        "Clear clipboard history?",
        "All clips that are not on a pinboard will be deleted. This cannot be undone.",
        "Clear History",
    )
}

/// TODO(windows, linux): native confirmation (MessageBoxW / GTK dialog).
/// Until then the destructive action is refused rather than run unasked.
#[cfg(not(target_os = "macos"))]
fn confirm_clear() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_has_the_right_size_and_is_a_template() {
        let rgba = icon_rgba(ICON_SIZE);
        assert_eq!(rgba.len(), (ICON_SIZE * ICON_SIZE * 4) as usize);
        assert!(rgba.chunks(4).all(|p| p[0] == 0 && p[1] == 0 && p[2] == 0));
    }

    #[test]
    fn icon_has_ink_in_the_body_and_clear_corners() {
        let rgba = icon_rgba(ICON_SIZE);
        let alpha = |x: u32, y: u32| rgba[((y * ICON_SIZE + x) * 4 + 3) as usize];
        assert_eq!(alpha(0, 0), 0);
        assert_eq!(alpha(ICON_SIZE - 1, 0), 0);
        assert_eq!(alpha(ICON_SIZE / 2, ICON_SIZE * 3 / 4), 255);
        let ink = rgba.chunks(4).filter(|p| p[3] > 0).count();
        let total = (ICON_SIZE * ICON_SIZE) as usize;
        assert!(ink > total / 5 && ink < total * 3 / 4, "{ink}");
    }
}
