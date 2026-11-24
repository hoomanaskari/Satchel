//! The menu bar icon and its "clip saved" animation, platform independent.
//!
//! The satchel glyph is drawn from code as a function of a [`Pose`] (how far
//! the flap is lifted, where the dropped card is), so every frame is
//! rendered on demand at the exact pixel size the menu bar needs (18 px at
//! 1x, 36 px at 2x): no image files, nothing hand-edited, the same code
//! makes the resting icon. [`pose_at`] is the timeline and [`Animator`] the
//! trigger/coalescing state machine; the platform layer only turns frames
//! into template images on a timer that runs while an animation does.
//!
//! The animation, about 0.6 s:
//! 1. the flap lifts (ease-out), opening the bag's mouth;
//! 2. a small card fades in at the top and falls (ease-in, like gravity),
//!    passing in front of the handle and the lifted flap (a clear moat keeps
//!    it legible in one colour) and disappearing behind the front panel;
//! 3. the flap drops shut and rebounds once, a little, before resting.

/// Total length in seconds.
pub const DURATION: f64 = 0.60;
/// The flap opens over `[0, OPEN_END]`.
const OPEN_END: f64 = 0.18;
/// The card falls over `[ITEM_START, ITEM_END]`.
const ITEM_START: f64 = 0.06;
const ITEM_END: f64 = 0.34;
/// The flap closes over `[CLOSE_START, CLOSE_END]`, then rebounds once.
const CLOSE_START: f64 = 0.31;
const CLOSE_END: f64 = 0.45;
/// Peak of the rebound, as a fraction of fully open.
const REBOUND: f64 = 0.14;

// Glyph geometry in a unit square, y pointing down, laid out on the 18 pt
// grid of a menu bar icon (`PT`) so the straight edges of the resting icon
// fall on whole pixels at 1x and 2x.
const PT: f64 = 1.0 / 18.0;
const BODY: (f64, f64, f64, f64) = (2.0 * PT, 16.0 * PT, 6.0 * PT, 16.0 * PT); // left, right, top, bottom
const BODY_RADIUS: f64 = 1.75 * PT;
/// Handle: the top half of a ring centred on the body's top edge.
const HANDLE_CENTER: (f64, f64) = (9.0 * PT, 6.0 * PT);
const HANDLE_RADII: (f64, f64) = (2.5 * PT, 4.0 * PT);
/// Bottom edge of the closed flap (top of the seam).
const FLAP_BOTTOM: f64 = 9.0 * PT;
/// How far the flap's bottom edge rises when fully open.
const FLAP_LIFT: f64 = 2.0 * PT;
/// Top edge of the front panel (bottom of the seam): the bag's mouth.
const FRONT_TOP: f64 = 10.0 * PT;
const SEAM_X: (f64, f64) = (3.0 * PT, 15.0 * PT);
const CLASP_X: (f64, f64) = (8.0 * PT, 10.0 * PT);
/// Clasp extent relative to the flap's bottom edge.
const CLASP_ABOVE: f64 = 1.0 * PT;
const CLASP_BELOW: f64 = 3.0 * PT;
/// The clasp hangs this much shorter when the flap is fully lifted (it
/// tilts up with the flap), keeping the mouth clear for the card.
const CLASP_FOLD: f64 = 2.0 * PT;
/// The card: appears inside the handle's loop and falls into the mouth.
const ITEM_X: (f64, f64) = (7.0 * PT, 11.0 * PT);
const ITEM_HEIGHT: f64 = 3.0 * PT;
const ITEM_RADIUS: f64 = 0.75 * PT;
const ITEM_TOP_FROM: f64 = 3.5 * PT;
const ITEM_TOP_TO: f64 = 11.0 * PT;
/// The card is fully opaque after this fraction of its fall.
const ITEM_FADE: f64 = 0.35;
/// Clear gap around the card, so it stays readable over other ink.
const MOAT: f64 = 0.045;

/// What the glyph looks like at one instant.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    /// 0 closed, 1 fully lifted.
    pub flap: f64,
    /// Top edge of the card and its opacity, while it is visible.
    pub item: Option<(f64, f64)>,
}

impl Pose {
    pub const REST: Pose = Pose { flap: 0.0, item: None };
}

fn ease_out_cubic(u: f64) -> f64 {
    1.0 - (1.0 - u).powi(3)
}

fn ease_in_quad(u: f64) -> f64 {
    u * u
}

fn ease_in_out_cubic(u: f64) -> f64 {
    if u < 0.5 { 4.0 * u * u * u } else { 1.0 - (-2.0 * u + 2.0).powi(3) / 2.0 }
}

fn progress(t: f64, from: f64, to: f64) -> f64 {
    ((t - from) / (to - from)).clamp(0.0, 1.0)
}

/// Flap openness while opening, and its inverse.
fn opening(t: f64) -> f64 {
    ease_out_cubic(progress(t, 0.0, OPEN_END))
}

fn opening_time(flap: f64) -> f64 {
    (1.0 - (1.0 - flap.clamp(0.0, 1.0)).cbrt()) * OPEN_END
}

fn flap_at(t: f64) -> f64 {
    if t < CLOSE_START {
        opening(t)
    } else if t < CLOSE_END {
        1.0 - ease_in_out_cubic(progress(t, CLOSE_START, CLOSE_END))
    } else {
        let u = progress(t, CLOSE_END, DURATION);
        REBOUND * (std::f64::consts::PI * u).sin() * (1.0 - u)
    }
}

fn item_top_at(t: f64) -> f64 {
    ITEM_TOP_FROM + (ITEM_TOP_TO - ITEM_TOP_FROM) * ease_in_quad(progress(t, ITEM_START, ITEM_END))
}

/// The first instant the card is entirely behind the front panel.
fn item_hidden_from() -> f64 {
    let p = (FRONT_TOP - ITEM_TOP_FROM) / (ITEM_TOP_TO - ITEM_TOP_FROM);
    ITEM_START + p.sqrt() * (ITEM_END - ITEM_START)
}

/// The pose `t` seconds into the animation. Exactly [`Pose::REST`] outside
/// `(0, DURATION)`.
pub fn pose_at(t: f64) -> Pose {
    if t <= 0.0 || t >= DURATION {
        return Pose::REST;
    }
    let item = (t > ITEM_START && t < item_hidden_from()).then(|| {
        let alpha = (progress(t, ITEM_START, ITEM_END) / ITEM_FADE).min(1.0);
        (item_top_at(t), alpha)
    });
    Pose { flap: flap_at(t), item }
}

/// When a new clip arrives at `t` seconds into a running animation whose
/// card is already inside: an instant of the opening phase that looks
/// exactly the same (same flap, no card), so the animation can continue
/// from there with no visible jump. `None` when no such instant exists (the
/// flap is still lifted further than it is when the card first appears).
fn rewind(t: f64) -> Option<f64> {
    if t < item_hidden_from() || t >= DURATION {
        return None;
    }
    let flap = flap_at(t);
    (flap <= opening(ITEM_START)).then(|| opening_time(flap))
}

/// Trigger and coalescing logic, in seconds on any monotonic clock.
///
/// - Idle: a clip starts the animation.
/// - Before the card has gone in: the clip joins the card already falling.
/// - After that, while the flap is low again: the animation rewinds to the
///   matching moment of the opening (see [`rewind`]) and drops another card.
/// - Otherwise (the flap is still high): one replay is remembered and plays
///   when this one ends. Never more than one, however many clips arrive.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Animator {
    start: Option<f64>,
    replay: bool,
}

/// What the platform layer shows for one timer tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Frame {
    Pose(Pose),
    /// Finished: show the resting icon and stop the timer.
    Done,
}

impl Animator {
    /// A clip was saved. Returns true when the animation was idle, i.e. the
    /// caller must start its frame timer.
    pub fn trigger(&mut self, now: f64) -> bool {
        let Some(start) = self.start else {
            self.start = Some(now);
            return true;
        };
        let t = now - start;
        if t < item_hidden_from() {
            // The falling card stands for this clip too.
        } else if let Some(earlier) = rewind(t) {
            self.start = Some(now - earlier);
        } else {
            self.replay = true;
        }
        false
    }

    /// The frame to show at `now`.
    pub fn frame(&mut self, now: f64) -> Frame {
        let Some(start) = self.start else { return Frame::Done };
        let t = now - start;
        if t < DURATION {
            return Frame::Pose(pose_at(t.max(0.0)));
        }
        if std::mem::take(&mut self.replay) {
            self.start = Some(now);
            return Frame::Pose(pose_at(0.0));
        }
        self.start = None;
        Frame::Done
    }
}

fn in_rounded_rect(x: f64, y: f64, (left, right, top, bottom): (f64, f64, f64, f64), r: f64) -> bool {
    if !(left..=right).contains(&x) || !(top..=bottom).contains(&y) {
        return false;
    }
    let cx = x.clamp(left + r, right - r);
    let cy = y.clamp(top + r, bottom - r);
    (x - cx).powi(2) + (y - cy).powi(2) <= r * r
}

/// Ink of everything behind the card: handle, flap with its clasp, and the
/// bag's sides and front, with the mouth (the seam at rest) cut out.
fn bag(x: f64, y: f64, flap: f64) -> bool {
    let (hx, hy) = (x - HANDLE_CENTER.0, y - HANDLE_CENTER.1);
    let ring = (hx * hx + hy * hy).sqrt();
    if y < HANDLE_CENTER.1 && (HANDLE_RADII.0..=HANDLE_RADII.1).contains(&ring) {
        return true;
    }
    if !in_rounded_rect(x, y, BODY, BODY_RADIUS) {
        return false;
    }
    let flap_bottom = FLAP_BOTTOM - FLAP_LIFT * flap;
    let clasp = (CLASP_X.0..=CLASP_X.1).contains(&x)
        && (flap_bottom - CLASP_ABOVE..=flap_bottom + CLASP_BELOW - CLASP_FOLD * flap).contains(&y);
    let mouth = (flap_bottom..=FRONT_TOP).contains(&y) && (SEAM_X.0..=SEAM_X.1).contains(&x);
    clasp || !mouth
}

/// Ink at `(x, y)` in a unit square, 0 to 1.
fn ink(x: f64, y: f64, pose: &Pose) -> f64 {
    let back = || bag(x, y, pose.flap) as u8 as f64;
    let Some((top, alpha)) = pose.item else { return back() };
    // The front panel is drawn over the card: below the mouth, and a moat
    // just above it, the card does not show.
    if y >= FRONT_TOP - MOAT {
        return back();
    }
    let rect = (ITEM_X.0, ITEM_X.1, top, top + ITEM_HEIGHT);
    if in_rounded_rect(x, y, rect, ITEM_RADIUS) {
        return alpha + (1.0 - alpha) * back();
    }
    let moat = (rect.0 - MOAT, rect.1 + MOAT, rect.2 - MOAT, rect.3 + MOAT);
    if in_rounded_rect(x, y, moat, ITEM_RADIUS + MOAT) {
        return (1.0 - alpha) * back();
    }
    back()
}

/// Black-on-transparent RGBA template image of `pose`, `size` pixels
/// square, supersampled (finer at small sizes, where each pixel counts).
pub fn render(size: u32, pose: &Pose) -> Vec<u8> {
    let ss: u32 = if size <= 24 { 6 } else { 4 };
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for py in 0..size {
        for px in 0..size {
            let mut sum = 0.0;
            for sy in 0..ss {
                for sx in 0..ss {
                    let x = (px as f64 + (sx as f64 + 0.5) / ss as f64) / size as f64;
                    let y = (py as f64 + (sy as f64 + 0.5) / ss as f64) / size as f64;
                    sum += ink(x, y, pose);
                }
            }
            let alpha = (sum * 255.0 / (ss * ss) as f64).round() as u8;
            out.extend_from_slice(&[0, 0, 0, alpha]);
        }
    }
    out
}

/// Lets only one "clip saved" hop to the UI thread be in flight: the capture
/// thread posts when `claim` returns true, the UI thread calls `release`
/// before handling it. Copies that arrive meanwhile ride along.
#[derive(Default)]
pub struct PostGate(std::sync::atomic::AtomicBool);

impl PostGate {
    pub const fn new() -> Self {
        Self(std::sync::atomic::AtomicBool::new(false))
    }
    pub fn claim(&self) -> bool {
        !self.0.swap(true, std::sync::atomic::Ordering::AcqRel)
    }
    pub fn release(&self) {
        self.0.store(false, std::sync::atomic::Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STEP: f64 = 1.0 / 240.0;

    fn times() -> impl Iterator<Item = f64> {
        (0..=(DURATION / STEP) as usize + 2).map(|i| i as f64 * STEP)
    }

    #[test]
    fn timeline_is_ordered_and_short() {
        assert!((0.4..=0.7).contains(&DURATION));
        const {
            assert!(ITEM_START < OPEN_END && OPEN_END < CLOSE_START && CLOSE_START < CLOSE_END);
            assert!(CLOSE_END < DURATION);
        }
        // The flap only starts closing once the card is nearly in, and is
        // fully open while the card passes the mouth.
        let hidden = item_hidden_from();
        assert!(hidden > CLOSE_START && hidden < CLOSE_END, "{hidden}");
        let reaches_mouth = ITEM_START
            + ((FRONT_TOP - ITEM_HEIGHT - ITEM_TOP_FROM) / (ITEM_TOP_TO - ITEM_TOP_FROM)).sqrt()
                * (ITEM_END - ITEM_START);
        assert!(reaches_mouth >= OPEN_END, "{reaches_mouth}");
    }

    #[test]
    fn starts_and_ends_exactly_at_rest() {
        assert_eq!(pose_at(0.0), Pose::REST);
        assert_eq!(pose_at(DURATION), Pose::REST);
        assert_eq!(pose_at(DURATION + 1.0), Pose::REST);
        assert_eq!(render(36, &pose_at(DURATION)), render(36, &Pose::REST));
        // The last frames before the end are already all but at rest.
        let last = pose_at(DURATION - STEP);
        assert!(last.item.is_none() && last.flap < 0.01, "{last:?}");
    }

    #[test]
    fn poses_change_smoothly() {
        let mut prev = pose_at(0.0);
        for t in times().skip(1) {
            let p = pose_at(t);
            assert!((p.flap - prev.flap).abs() < 0.12, "flap jump at {t}");
            assert!((0.0..=1.0).contains(&p.flap), "{t}");
            if let (Some((a, _)), Some((b, _))) = (prev.item, p.item) {
                assert!(b >= a && b - a < 0.05, "card jump at {t}");
            }
            prev = p;
        }
    }

    #[test]
    fn card_fades_in_and_disappears_behind_the_front() {
        let first = times().find_map(|t| pose_at(t).item).unwrap();
        assert!(first.1 < 0.1, "{first:?}");
        let last = times().filter_map(|t| pose_at(t).item).last().unwrap();
        assert!(last.0 < FRONT_TOP && last.0 > FRONT_TOP - 0.05, "{last:?}");
        assert_eq!(last.1, 1.0);
    }

    #[test]
    fn every_frame_stays_inside_the_icon_box() {
        // Same image size every frame; the drawing never needs more room, so
        // neighbouring menu bar items cannot shift.
        for t in times() {
            let rgba = render(18, &pose_at(t));
            assert_eq!(rgba.len(), 18 * 18 * 4);
            assert!(rgba.chunks(4).all(|p| p[..3] == [0, 0, 0]));
        }
    }

    #[test]
    fn open_flap_shows_the_mouth_and_rest_keeps_the_seam() {
        let size = 36;
        let alpha = |img: &[u8], x: f64, y: f64| {
            let (px, py) = ((x * size as f64) as usize, (y * size as f64) as usize);
            img[(py * size + px) * 4 + 3]
        };
        let rest = render(size as u32, &Pose::REST);
        let open = render(size as u32, &Pose { flap: 1.0, item: None });
        // Left of the clasp, between the lifted flap and the front: ink at
        // rest (flap), clear when open (mouth).
        assert_eq!(alpha(&rest, 0.3, 0.45), 255);
        assert_eq!(alpha(&open, 0.3, 0.45), 0);
        // The front panel never moves.
        assert_eq!(alpha(&rest, 0.3, 0.75), 255);
        assert_eq!(alpha(&open, 0.3, 0.75), 255);
    }

    #[test]
    fn card_is_drawn_with_a_clear_moat_over_the_handle() {
        let at = |x: f64, y: f64, pose: &Pose| ink(x * PT, y * PT, pose);
        let card = Pose { flap: 1.0, item: Some((3.6 * PT, 1.0)) };
        let empty = Pose { flap: 1.0, item: None };
        assert_eq!(at(9.0, 5.0, &card), 1.0); // the card
        // Beside it, the handle is cut by the moat.
        assert_eq!(at(6.5, 5.0, &empty), 1.0);
        assert_eq!(at(6.5, 5.0, &card), 0.0);
        // A half-faded card cuts it only halfway.
        let faint = Pose { item: Some((3.6 * PT, 0.5)), ..card };
        assert_eq!(at(6.5, 5.0, &faint), 0.5);
        // The front panel is untouched.
        assert_eq!(at(4.0, 13.0, &card), 1.0);
    }

    #[test]
    fn idle_trigger_starts_and_the_animation_ends_on_its_own() {
        let mut a = Animator::default();
        assert_eq!(a.frame(0.0), Frame::Done);
        assert!(a.trigger(10.0));
        assert!(a.start.is_some());
        assert!(matches!(a.frame(10.1), Frame::Pose(_)));
        assert_eq!(a.frame(10.0 + DURATION + 1e-9), Frame::Done);
        assert!(!a.start.is_some());
        // A later clip starts it again.
        assert!(a.trigger(20.0));
    }

    #[test]
    fn clips_while_the_card_falls_join_it() {
        let mut a = Animator::default();
        a.trigger(0.0);
        let before = a;
        for t in [0.01, 0.1, 0.2, item_hidden_from() - 0.001] {
            assert!(!a.trigger(t));
            assert_eq!(a, before, "{t}");
        }
        assert_eq!(a.frame(DURATION), Frame::Done);
    }

    #[test]
    fn a_clip_once_the_flap_is_low_again_rewinds_without_a_jump() {
        let mut rewound = 0;
        for t in times().filter(|&t| t >= item_hidden_from() && t < DURATION) {
            let mut a = Animator::default();
            a.trigger(0.0);
            let Frame::Pose(shown) = a.frame(t) else { panic!("{t}") };
            assert!(!a.trigger(t));
            if a.replay {
                assert!(shown.flap > opening(ITEM_START) - 1e-9, "{t}");
                continue;
            }
            rewound += 1;
            let Frame::Pose(next) = a.frame(t) else { panic!("{t}") };
            assert!((next.flap - shown.flap).abs() < 1e-9, "flap jumped at {t}");
            assert_eq!(next.item, None, "card popped in at {t}");
            // And the new run still drops a card and ends at rest.
            let mut saw_card = false;
            let mut now = t;
            loop {
                now += STEP;
                match a.frame(now) {
                    Frame::Pose(p) => saw_card |= p.item.is_some(),
                    Frame::Done => break,
                }
            }
            assert!(saw_card, "{t}");
            assert!(now - t <= DURATION + STEP, "{t}");
        }
        assert!(rewound > 20, "{rewound}");
    }

    #[test]
    fn many_clips_queue_at_most_one_replay() {
        let mut a = Animator::default();
        a.trigger(0.0);
        // Just after the card went in the flap is still high: replay.
        let t = item_hidden_from() + 0.001;
        assert!(rewind(t).is_none());
        for _ in 0..50 {
            a.trigger(t);
        }
        assert!(a.replay);
        // Plays once more, then stops for good.
        assert!(matches!(a.frame(DURATION), Frame::Pose(_)));
        assert_eq!(a.frame(DURATION + DURATION), Frame::Done);
        assert!(!a.start.is_some());
    }

    #[test]
    fn a_steady_stream_of_clips_never_builds_up() {
        // A copy every 300 ms (the watcher's poll interval) for 10 s.
        let mut a = Animator::default();
        let mut now: f64 = 0.0;
        let mut timer_starts = 0;
        while now < 10.0 {
            if (now / 0.3).fract() < STEP / 0.3 {
                timer_starts += a.trigger(now) as u32;
            }
            a.frame(now);
            now += STEP;
        }
        assert!(timer_starts <= 1, "{timer_starts}");
        // Once the copies stop it settles within one more run.
        let mut end = now;
        while a.frame(end) != Frame::Done {
            end += STEP;
        }
        assert!(end - now <= 2.0 * DURATION, "{}", end - now);
    }

    #[test]
    fn post_gate_lets_one_hop_through_until_released() {
        let gate = PostGate::default();
        assert!(gate.claim());
        assert!(!gate.claim());
        assert!(!gate.claim());
        gate.release();
        assert!(gate.claim());
    }

    /// Writes every frame side by side, enlarged, to the PNG named by
    /// `SATCHEL_TRAY_SHEET` (black on white, top row 2x, bottom row 1x), to
    /// review the animation by eye:
    /// `SATCHEL_TRAY_SHEET=/tmp/sheet.png cargo test -p satchel tray_sheet -- --ignored`
    #[test]
    #[ignore]
    fn tray_sheet() {
        let Some(path) = std::env::var_os("SATCHEL_TRAY_SHEET") else { return };
        let fps = 60.0;
        let n = (DURATION * fps).ceil() as u32 + 1;
        let (cell, zoom) = (36u32, 3u32);
        let w = n * (cell + 4) * zoom;
        let h = 2 * (cell + 4) * zoom;
        let mut img = vec![255u8; (w * h * 4) as usize];
        for i in 0..n {
            let pose = pose_at(i as f64 / fps);
            for (row, size) in [(0u32, 36u32), (1, 18)] {
                let frame = render(size, &pose);
                let scale = zoom * cell / size;
                for y in 0..size * scale {
                    for x in 0..size * scale {
                        let a = frame[(((y / scale) * size + x / scale) * 4 + 3) as usize] as u32;
                        let v = (255 - a) as u8;
                        let ox = i * (cell + 4) * zoom + x;
                        let oy = row * (cell + 4) * zoom + y;
                        let o = ((oy * w + ox) * 4) as usize;
                        img[o..o + 3].copy_from_slice(&[v, v, v]);
                    }
                }
            }
        }
        let png = crate::thumbnail::encode_rgba(&img, w, h).unwrap();
        std::fs::write(path, png).unwrap();
    }
}
