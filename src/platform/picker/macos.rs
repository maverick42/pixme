//! macOS screen sampling for the full-screen pick: `CGDisplay` reads of
//! the small rect around the cursor — the same framebuffer path
//! `capture_frame` uses, so the same Screen Recording permission gates
//! it. Without it `image_for_rect` returns `None` and the pick reports
//! [`PickOutcome::Denied`] rather than an ambiguous black.

use core_graphics::display::CGDisplay;
use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use eframe::egui::Color32;
use image::RgbaImage;

use super::PREVIEW_PATCH;

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

/// The `PREVIEW_PATCH`-point square centred on `(cx, cy)` as RGBA —
/// Retina displays return a denser bitmap (points × scale factor); the
/// centre pixel is always the sampled point.
pub fn screen_patch(cx: i32, cy: i32) -> Option<RgbaImage> {
    let id = display_at(cx, cy)?;
    let half = (PREVIEW_PATCH / 2) as f64;
    let rect = CGRect::new(
        &CGPoint::new(f64::from(cx) - half, f64::from(cy) - half),
        &CGSize::new(PREVIEW_PATCH as f64, PREVIEW_PATCH as f64),
    );
    let img = CGDisplay::new(id).image_for_rect(rect)?;
    super::super::macos::cg_image_to_rgba(&img, img.width(), img.height())
}

/// The colour of the screen pixel at `(x, y)` — a 1-pt rect read. The
/// `PREVIEW_PATCH` rect clips at screen edges, where its centre pixel
/// stops being the cursor's, so the validating read stays exact.
pub fn screen_pixel(x: i32, y: i32) -> Option<Color32> {
    let id = display_at(x, y)?;
    let rect = CGRect::new(
        &CGPoint::new(f64::from(x), f64::from(y)),
        &CGSize::new(1.0, 1.0),
    );
    let img = CGDisplay::new(id).image_for_rect(rect)?;
    let rgba = super::super::macos::cg_image_to_rgba(&img, img.width(), img.height())?;
    let p = rgba.get_pixel(0, 0).0;
    Some(Color32::from_rgba_unmultiplied(p[0], p[1], p[2], 255))
}
