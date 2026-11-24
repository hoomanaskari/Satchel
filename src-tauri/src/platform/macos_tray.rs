//! The menu bar icon's "clip saved" animation on macOS.
//!
//! Driven natively on the `NSStatusBarButton` that Tauri's tray creates:
//! each frame is a template `NSImage` with an 18 px and a 36 px bitmap
//! (crisp on 1x and 2x displays, tinted by AppKit for light and dark menu
//! bars and the highlighted state), set straight on the button from an
//! `NSTimer` in the common run loop modes (so it keeps running while the
//! menu is open). The timer exists only while an animation runs. Frames are
//! drawn on demand by `tray_anim` from the elapsed time, so a late tick
//! never slows the animation down; nothing is cached between animations.
//! Every image is 18 pt square like the resting icon, so the status item's
//! width never changes.

use std::cell::RefCell;
use std::ptr::NonNull;
use std::sync::OnceLock;
use std::time::Instant;

use block2::RcBlock;
use objc2::{AnyThread, MainThreadMarker};
use objc2::rc::Retained;
use objc2_app_kit::{NSBitmapImageRep, NSImage, NSStatusBarButton, NSStatusItem};
use objc2_foundation::{NSData, NSRunLoop, NSRunLoopCommonModes, NSSize, NSTimer};

use crate::tray_anim::{Animator, Frame, Pose};

/// Menu bar icon size in points.
const POINTS: f64 = 18.0;
/// Pixel sizes of the bitmaps in each image: 1x and 2x.
const PIXELS: [u32; 2] = [18, 36];
/// Timer period. Frames are sampled from the clock, not counted.
const FRAME_SECONDS: f64 = 1.0 / 60.0;

struct State {
    button: Retained<NSStatusBarButton>,
    rest: Retained<NSImage>,
    animator: Animator,
    timer: Option<Retained<NSTimer>>,
}

thread_local! {
    /// Main thread only (AppKit objects).
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

/// Monotonic seconds since first use.
fn now() -> f64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64()
}

/// A template image of `pose`, 18 pt square, with 1x and 2x bitmaps.
fn image(pose: &Pose) -> Option<Retained<NSImage>> {
    let image = NSImage::initWithSize(NSImage::alloc(), NSSize::new(POINTS, POINTS));
    for px in PIXELS {
        let png = crate::thumbnail::encode_rgba(&crate::tray_anim::render(px, pose), px, px).ok()?;
        let rep = NSBitmapImageRep::imageRepWithData(&NSData::from_vec(png))?;
        rep.setSize(NSSize::new(POINTS, POINTS));
        image.addRepresentation(&rep);
    }
    image.setTemplate(true);
    Some(image)
}

/// Takes over the status item's image: shows the resting icon (with a 1x
/// bitmap, sharper on non-Retina displays than the tray's single 2x one)
/// and remembers the button for the animation.
pub fn install(mtm: MainThreadMarker, item: &NSStatusItem) {
    let (Some(button), Some(rest)) = (item.button(mtm), image(&Pose::REST)) else { return };
    button.setImage(Some(&rest));
    STATE.with_borrow_mut(|state| {
        *state = Some(State { button, rest, animator: Animator::default(), timer: None });
    });
}

/// A clip was saved: starts the animation, or folds this clip into the one
/// running (see `tray_anim::Animator`). With Reduce Motion on, the icon
/// stays still.
pub fn clip_saved(_mtm: MainThreadMarker) {
    if super::macos::reduce_motion() {
        return;
    }
    let start = STATE.with_borrow_mut(|state| {
        state.as_mut().is_some_and(|s| s.animator.trigger(now()))
    });
    if start {
        start_timer();
        tick(); // First frame now, not one period later.
    }
}

fn start_timer() {
    let block = RcBlock::new(|_timer: NonNull<NSTimer>| tick());
    // SAFETY: the block captures nothing, so it is trivially sendable; it is
    // scheduled on the main run loop below and so only ever runs on the
    // main thread, where `tick` touches AppKit.
    let timer = unsafe { NSTimer::timerWithTimeInterval_repeats_block(FRAME_SECONDS, true, &block) };
    // SAFETY: `NSRunLoopCommonModes` is a constant NSString exported by
    // Foundation; adding a timer to the main run loop from the main thread.
    unsafe { NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes) };
    STATE.with_borrow_mut(|state| match state {
        Some(s) => {
            if let Some(old) = s.timer.replace(timer) {
                old.invalidate();
            }
        }
        None => timer.invalidate(),
    });
}

fn tick() {
    STATE.with_borrow_mut(|state| {
        let Some(s) = state.as_mut() else { return };
        match s.animator.frame(now()) {
            Frame::Pose(pose) => {
                if let Some(img) = image(&pose) {
                    s.button.setImage(Some(&img));
                }
            }
            Frame::Done => {
                s.button.setImage(Some(&s.rest));
                if let Some(timer) = s.timer.take() {
                    timer.invalidate();
                }
            }
        }
    });
}
