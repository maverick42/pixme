//! macOS screen sampling for the full-screen pick: while the "PixMe
//! Pick" overlay is up, `CGWindowListCreateImage` with
//! `kCGWindowListOptionOnScreenBelowWindow` composites only the windows
//! BELOW it — the overlay's alpha-20 veil darkens every framebuffer
//! pixel by ~8 %, so reads that include our own window
//! (`image_for_rect`, `CGDisplay::image()`) systematically report a
//! colour slightly darker than the screen shows. Without the overlay
//! the framebuffer composite is clean already, and without the Screen
//! Recording permission all these reads return `None`, surfacing
//! [`PickOutcome::Denied`] rather than an ambiguous black.

use core_graphics::display::CGDisplay;
use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use core_graphics::image::CGImage;
use core_graphics::window::{
    create_image, kCGWindowImageDefault, kCGWindowListOptionOnScreenBelowWindow,
};
use image::RgbaImage;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::{PICK_OVERLAY_TITLE, PREVIEW_PATCH};

/// The display id of the monitor containing the native-space point
/// `(x, y)` (points on macOS), or the primary monitor's id when the
/// point sits in no rect — gaps between monitors still show pixels.
fn display_at(x: i32, y: i32) -> Option<u32> {
    let monitors = crate::capture::screen::monitors().ok()?;
    let inside = monitors.iter().find(|m| {
        (m.rect.x..m.rect.x + m.rect.width as i32).contains(&x)
            && (m.rect.y..m.rect.y + m.rect.height as i32).contains(&y)
    });
    inside.or(monitors.first()).map(|m| m.id)
}

/// `kCGWindowNumber` of the "PixMe Pick" overlay while it is up — with
/// one overlay per monitor, the LOWEST of ours (last in the
/// front-to-back list) is the reference: a "below window" composite
/// then excludes every pick overlay. The loupe samples at ~30 Hz, so
/// positive WindowServer lookups are cached for half a second (`None`
/// is never cached so the very first frame before the window appears in
/// `window_list()` cannot lock in a 500 ms veil-tinted fallback).
fn overlay_window_id() -> Option<u32> {
    static CACHE: Mutex<Option<(Instant, u32)>> = Mutex::new(None);
    let mut cache = CACHE.lock().ok()?;
    if let Some((refreshed, id)) = *cache
        && refreshed.elapsed() < Duration::from_millis(500)
    {
        return Some(id);
    }
    let me = std::process::id();
    let id = crate::platform::macos::window_list()
        .into_iter()
        .rfind(|w| w.pid == me && w.title == PICK_OVERLAY_TITLE)
        .map(|w| w.id);
    if let Some(id) = id {
        *cache = Some((Instant::now(), id));
    }
    id
}

/// A screen read of `rect` (global display points): below the pick
/// overlay when it is up so its dim veil cannot tint the sample;
/// otherwise the display's framebuffer composite directly.
fn sample_image(rect: CGRect, x: i32, y: i32) -> Option<CGImage> {
    if let Some(id) = overlay_window_id() {
        create_image(
            rect,
            kCGWindowListOptionOnScreenBelowWindow,
            id,
            kCGWindowImageDefault,
        )
    } else {
        CGDisplay::new(display_at(x, y)?).image_for_rect(rect)
    }
}

/// The `PREVIEW_PATCH`-point square centred on `(cx, cy)` as RGBA —
/// Retina displays return a denser bitmap (points × scale factor); the
/// centre pixel is always the sampled point.
pub fn screen_patch(cx: i32, cy: i32) -> Option<RgbaImage> {
    let half = (PREVIEW_PATCH / 2) as f64;
    let rect = CGRect::new(
        &CGPoint::new(f64::from(cx) - half, f64::from(cy) - half),
        &CGSize::new(PREVIEW_PATCH as f64, PREVIEW_PATCH as f64),
    );
    let img = sample_image(rect, cx, cy)?;
    super::super::macos::cg_image_to_rgba(&img, img.width(), img.height())
}

/// The validating read for a click or `C`: a fresh patch at `(x, y)`
/// whose centre colour is literally the sample the loupe highlights —
/// [`super::patch_centre`] reduces it, so applied/copied and displayed
/// values can never diverge. macOS framebuffer reads are cheap enough
/// to stay synchronous, so this is exactly [`screen_patch`].
pub fn pick_patch(cx: i32, cy: i32) -> Option<RgbaImage> {
    screen_patch(cx, cy)
}
