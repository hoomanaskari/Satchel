//! Saved window frames: where a window was, checked against the displays
//! that exist when it opens again, and a debounced writer for saving it while
//! the user moves or resizes the window.
//!
//! Frames and displays are in points in one global coordinate space (on
//! macOS, AppKit screen coordinates). Points do not change with a display's
//! scale factor, so a frame saved on a Retina display and restored on a
//! standard one keeps its size; nothing here deals with pixels.

use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::Duration;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self { x, y, width, height }
    }

    fn right(&self) -> f64 {
        self.x + self.width
    }

    fn top(&self) -> f64 {
        self.y + self.height
    }

    fn contains(&self, (x, y): (f64, f64)) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.top()
    }

    fn overlap_area(&self, other: &Rect) -> f64 {
        let w = self.right().min(other.right()) - self.x.max(other.x);
        let h = self.top().min(other.top()) - self.y.max(other.y);
        if w > 0.0 && h > 0.0 { w * h } else { 0.0 }
    }

    fn is_valid(&self) -> bool {
        [self.x, self.y, self.width, self.height].iter().all(|v| v.is_finite())
            && self.width > 0.0
            && self.height > 0.0
    }
}

/// One display: its whole frame, and the part windows may use (without the
/// menu bar and the Dock).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Display {
    pub frame: Rect,
    pub work_area: Rect,
}

/// Smallest and largest frame size, in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Limits {
    pub min: (f64, f64),
    pub max: (f64, f64),
}

/// The stored form of a frame (JSON).
pub fn encode(frame: &Rect) -> String {
    serde_json::to_string(frame).unwrap_or_default()
}

/// A stored frame, or None when it is missing, malformed or nonsensical.
pub fn decode(stored: &str) -> Option<Rect> {
    serde_json::from_str::<Rect>(stored).ok().filter(Rect::is_valid)
}

/// The display a window should open on when it has no usable saved frame:
/// the one under the cursor, else the first (the main display).
pub fn fallback_display(displays: &[Display], cursor: Option<(f64, f64)>) -> Option<Display> {
    cursor
        .and_then(|c| displays.iter().find(|d| d.frame.contains(c)))
        .or_else(|| displays.first())
        .copied()
}

/// Where to put a window that was last at `saved`.
///
/// The size is clamped to `limits`. A frame that is still at least partly on
/// a display keeps its place, moved just enough to be fully inside that
/// display's work area (the display it overlaps most); one that is on no
/// display (moved off-screen, or its display is gone) is centred on
/// `fallback`. A frame larger than the work area shrinks to fit it, even
/// below the minimum: being reachable matters more.
pub fn restore(saved: Rect, displays: &[Display], fallback: Display, limits: Limits) -> Rect {
    let width = saved.width.clamp(limits.min.0, limits.max.0);
    let height = saved.height.clamp(limits.min.1, limits.max.1);
    let best = displays
        .iter()
        .map(|d| (d, saved.overlap_area(&d.frame)))
        .filter(|(_, area)| *area > 0.0)
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(d, _)| *d);
    let (area, x, y) = match best {
        Some(d) => (d.work_area, saved.x, saved.y),
        None => {
            let a = fallback.work_area;
            (a, a.x + (a.width - width) / 2.0, a.y + (a.height - height) / 2.0)
        }
    };
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: x.clamp(area.x, area.right() - width).round(),
        y: y.clamp(area.y, area.top() - height).round(),
        width: width.round(),
        height: height.round(),
    }
}

/// Saves frames off the main thread: `note` keeps the latest and writes it
/// once the window has been still for `quiet`; `flush` writes at once (on
/// close) and drops anything pending.
pub struct FrameSaver {
    tx: Sender<Msg>,
}

enum Msg {
    Note(Rect),
    Flush(Rect),
}

impl FrameSaver {
    pub fn new(quiet: Duration, write: impl Fn(Rect) + Send + 'static) -> Self {
        let (tx, rx) = mpsc::channel::<Msg>();
        std::thread::Builder::new()
            .name("satchel-window-frame".into())
            .spawn(move || {
                let mut pending: Option<Rect> = None;
                loop {
                    let msg = match pending {
                        Some(_) => rx.recv_timeout(quiet),
                        None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
                    };
                    match msg {
                        Ok(Msg::Note(frame)) => pending = Some(frame),
                        Ok(Msg::Flush(frame)) => {
                            pending = None;
                            write(frame);
                        }
                        Err(RecvTimeoutError::Timeout) => {
                            if let Some(frame) = pending.take() {
                                write(frame);
                            }
                        }
                        Err(RecvTimeoutError::Disconnected) => {
                            if let Some(frame) = pending.take() {
                                write(frame);
                            }
                            return;
                        }
                    }
                }
            })
            .expect("spawn the window frame thread");
        Self { tx }
    }

    /// Another handle to the same writer.
    pub fn clone_sender(&self) -> Self {
        Self { tx: self.tx.clone() }
    }

    pub fn note(&self, frame: Rect) {
        let _ = self.tx.send(Msg::Note(frame));
    }

    pub fn flush(&self, frame: Rect) {
        let _ = self.tx.send(Msg::Flush(frame));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    const LIMITS: Limits = Limits { min: (600.0, 430.0), max: (1040.0, 1030.0) };

    /// A 1440×900 main display (menu bar 25 pt at the top, Dock 70 pt at the
    /// bottom) in y-up coordinates.
    fn main() -> Display {
        Display { frame: Rect::new(0.0, 0.0, 1440.0, 900.0), work_area: Rect::new(0.0, 70.0, 1440.0, 805.0) }
    }

    /// A 1920×1080 display to the right of the main one, without a Dock.
    fn right() -> Display {
        Display { frame: Rect::new(1440.0, 0.0, 1920.0, 1080.0), work_area: Rect::new(1440.0, 0.0, 1920.0, 1055.0) }
    }

    #[test]
    fn encoding_round_trips() {
        let f = Rect::new(-1200.5, 300.0, 760.0, 630.0);
        assert_eq!(decode(&encode(&f)), Some(f));
    }

    #[test]
    fn malformed_or_nonsensical_frames_are_ignored() {
        assert_eq!(decode(""), None);
        assert_eq!(decode("{\"x\":1}"), None);
        assert_eq!(decode("{\"x\":0,\"y\":0,\"width\":0,\"height\":500}"), None);
        assert_eq!(decode("{\"x\":0,\"y\":0,\"width\":-5,\"height\":500}"), None);
        assert_eq!(decode("{\"x\":1e999,\"y\":0,\"width\":700,\"height\":500}"), None);
    }

    #[test]
    fn a_frame_fully_on_a_display_is_kept() {
        let f = Rect::new(100.0, 120.0, 800.0, 600.0);
        assert_eq!(restore(f, &[main(), right()], main(), LIMITS), f);
        let g = Rect::new(2000.0, 200.0, 900.0, 700.0);
        assert_eq!(restore(g, &[main(), right()], main(), LIMITS), g);
    }

    #[test]
    fn the_size_is_clamped_to_the_limits() {
        let small = restore(Rect::new(100.0, 120.0, 300.0, 200.0), &[main()], main(), LIMITS);
        assert_eq!((small.width, small.height), (600.0, 430.0));
        let big = restore(Rect::new(1500.0, 10.0, 1800.0, 1070.0), &[main(), right()], main(), LIMITS);
        assert_eq!((big.width, big.height), (1040.0, 1030.0));
    }

    #[test]
    fn a_frame_partly_off_screen_is_moved_fully_on() {
        // Hanging off the left edge and under the Dock.
        let f = restore(Rect::new(-300.0, 20.0, 800.0, 600.0), &[main()], main(), LIMITS);
        assert_eq!(f, Rect::new(0.0, 70.0, 800.0, 600.0));
        // Title bar pushed up under the menu bar.
        let g = restore(Rect::new(200.0, 400.0, 800.0, 600.0), &[main()], main(), LIMITS);
        assert_eq!(g, Rect::new(200.0, 275.0, 800.0, 600.0));
    }

    #[test]
    fn a_frame_spanning_two_displays_moves_onto_the_one_it_overlaps_most() {
        let f = restore(Rect::new(1200.0, 100.0, 800.0, 600.0), &[main(), right()], main(), LIMITS);
        assert_eq!(f, Rect::new(1440.0, 100.0, 800.0, 600.0));
        let g = restore(Rect::new(1000.0, 100.0, 800.0, 600.0), &[main(), right()], main(), LIMITS);
        assert_eq!(g, Rect::new(640.0, 100.0, 800.0, 600.0));
    }

    #[test]
    fn a_frame_on_a_display_that_is_gone_is_centred_on_the_fallback() {
        // Saved on the right display, which has been unplugged.
        let f = restore(Rect::new(2000.0, 200.0, 800.0, 600.0), &[main()], main(), LIMITS);
        assert_eq!(f, Rect::new(320.0, 173.0, 800.0, 600.0));
    }

    #[test]
    fn a_frame_far_off_screen_is_centred_on_the_fallback() {
        let f = restore(Rect::new(-9000.0, 9000.0, 800.0, 600.0), &[main(), right()], right(), LIMITS);
        assert_eq!(f, Rect::new(1440.0 + 560.0, 228.0, 800.0, 600.0));
    }

    #[test]
    fn a_frame_larger_than_the_display_shrinks_to_fit_it() {
        let tiny = Display { frame: Rect::new(0.0, 0.0, 800.0, 500.0), work_area: Rect::new(0.0, 0.0, 800.0, 475.0) };
        let f = restore(Rect::new(0.0, 0.0, 1000.0, 900.0), &[tiny], tiny, LIMITS);
        assert_eq!(f, Rect::new(0.0, 0.0, 800.0, 475.0));
        // Even below the minimum: reachable beats the minimum size.
        let tinier = Display { frame: Rect::new(0.0, 0.0, 500.0, 400.0), work_area: Rect::new(0.0, 0.0, 500.0, 375.0) };
        let g = restore(Rect::new(50.0, 50.0, 700.0, 500.0), &[tinier], tinier, LIMITS);
        assert_eq!(g, Rect::new(0.0, 0.0, 500.0, 375.0));
    }

    #[test]
    fn displays_left_of_or_below_the_main_one_have_negative_coordinates() {
        let left = Display {
            frame: Rect::new(-1920.0, -180.0, 1920.0, 1080.0),
            work_area: Rect::new(-1920.0, -180.0, 1920.0, 1055.0),
        };
        let f = Rect::new(-1500.0, 0.0, 800.0, 600.0);
        assert_eq!(restore(f, &[main(), left], main(), LIMITS), f);
    }

    #[test]
    fn the_fallback_is_the_display_under_the_cursor_else_the_main_one() {
        let displays = [main(), right()];
        assert_eq!(fallback_display(&displays, Some((2000.0, 500.0))), Some(right()));
        assert_eq!(fallback_display(&displays, Some((10.0, 10.0))), Some(main()));
        assert_eq!(fallback_display(&displays, Some((-50.0, 10.0))), Some(main()));
        assert_eq!(fallback_display(&displays, None), Some(main()));
        assert_eq!(fallback_display(&[], None), None);
    }

    fn recorder() -> (Arc<Mutex<Vec<Rect>>>, impl Fn(Rect) + Send + 'static) {
        let written = Arc::new(Mutex::new(Vec::new()));
        let sink = written.clone();
        (written, move |f| sink.lock().unwrap().push(f))
    }

    fn wait_for(written: &Arc<Mutex<Vec<Rect>>>, n: usize) {
        for _ in 0..200 {
            if written.lock().unwrap().len() >= n {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn the_saver_writes_only_the_last_frame_once_the_window_is_still() {
        let (written, write) = recorder();
        let saver = FrameSaver::new(Duration::from_millis(40), write);
        for i in 0..10 {
            saver.note(Rect::new(f64::from(i), 0.0, 700.0, 500.0));
        }
        wait_for(&written, 1);
        std::thread::sleep(Duration::from_millis(80));
        assert_eq!(*written.lock().unwrap(), vec![Rect::new(9.0, 0.0, 700.0, 500.0)]);
    }

    #[test]
    fn flushing_writes_at_once_and_drops_what_was_pending() {
        let (written, write) = recorder();
        let saver = FrameSaver::new(Duration::from_secs(60), write);
        saver.note(Rect::new(1.0, 0.0, 700.0, 500.0));
        saver.flush(Rect::new(2.0, 0.0, 700.0, 500.0));
        wait_for(&written, 1);
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(*written.lock().unwrap(), vec![Rect::new(2.0, 0.0, 700.0, 500.0)]);
    }
}
