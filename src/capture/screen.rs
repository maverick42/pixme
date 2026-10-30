//! Monitor capture via `xcap`: enumerate monitors, grab the full frame,
//! crop a region out of it. All OS specifics (Screen Recording permission,
//! Retina physical pixels) stay behind this module.

use xcap::Monitor;

use super::{Capture, CaptureError, MonitorInfo, PhysicalRect};

/// All connected monitors as [`MonitorInfo`] (physical-pixel bounds).
pub fn monitors() -> Result<Vec<MonitorInfo>, CaptureError> {
    let list = Monitor::all().map_err(|e| CaptureError::Failed(e.to_string()))?;
    let mut out = Vec::with_capacity(list.len());
    for m in &list {
        out.push(MonitorInfo {
            id: m.id().map_err(|e| CaptureError::Failed(e.to_string()))?,
            name: m.name().map_err(|e| CaptureError::Failed(e.to_string()))?,
            rect: PhysicalRect {
                x: m.x().map_err(|e| CaptureError::Failed(e.to_string()))?,
                y: m.y().map_err(|e| CaptureError::Failed(e.to_string()))?,
                width: m.width().map_err(|e| CaptureError::Failed(e.to_string()))?,
                height: m
                    .height()
                    .map_err(|e| CaptureError::Failed(e.to_string()))?,
            },
            scale_factor: m
                .scale_factor()
                .map_err(|e| CaptureError::Failed(e.to_string()))?,
            is_primary: m
                .is_primary()
                .map_err(|e| CaptureError::Failed(e.to_string()))?,
        });
    }
    Ok(out)
}

/// The monitor to use when no pick is given: the primary one (rules start
/// with a single monitor; multi-monitor selection lands later).
pub fn primary_monitor() -> Result<MonitorInfo, CaptureError> {
    let all = monitors()?;
    all.iter()
        .find(|m| m.is_primary)
        .or_else(|| all.first())
        .cloned()
        .ok_or(CaptureError::Failed("no monitor".into()))
}

/// The monitor whose bounds contain the logical point `(x, y)` — the same
/// global display space `MonitorInfo::rect` lives in. `None` when the point
/// falls outside every monitor (gaps in a non-rectangular arrangement).
pub fn monitor_at(x: f32, y: f32) -> Option<MonitorInfo> {
    monitors().ok()?.into_iter().find(|m| {
        let r = &m.rect;
        let (x0, y0) = (r.x as f32, r.y as f32);
        let (x1, y1) = (
            (r.x + r.width as i32) as f32,
            (r.y + r.height as i32) as f32,
        );
        x >= x0 && x < x1 && y >= y0 && y < y1
    })
}

/// One `xcap` monitor as our [`MonitorInfo`] (logical-point bounds).
pub fn monitor_info(m: &Monitor) -> Result<MonitorInfo, CaptureError> {
    Ok(MonitorInfo {
        id: m.id().unwrap_or_default(),
        name: m.name().unwrap_or_default(),
        rect: PhysicalRect {
            x: m.x().unwrap_or_default(),
            y: m.y().unwrap_or_default(),
            width: m.width().unwrap_or_default(),
            height: m.height().unwrap_or_default(),
        },
        scale_factor: m.scale_factor().unwrap_or(1.0),
        is_primary: m.is_primary().unwrap_or(false),
    })
}

/// Captures the full frame of monitor `id` (physical pixels).
pub fn capture_monitor(id: u32) -> Result<Capture, CaptureError> {
    let monitor = Monitor::all()
        .map_err(|e| CaptureError::Failed(e.to_string()))?
        .into_iter()
        .find(|m| m.id().ok() == Some(id))
        .ok_or(CaptureError::Failed("monitor disappeared".into()))?;
    let info = monitor_info(&monitor)?;
    let image = monitor
        .capture_image()
        .map_err(|e| CaptureError::Failed(e.to_string()))?;
    Ok(Capture {
        image: std::sync::Arc::new(image),
        scale_factor: info.scale_factor,
        monitor: info,
    })
}

/// Captures the full frame of every connected monitor in one pass — the
/// region overlay freezes all screens up front so the crosshair can follow
/// the cursor across displays until the user clicks to start a selection.
/// When `include_pointer` is `true`, a snapshot of the OS cursor is taken
/// NOW (before the selection crosshair exists) and returned alongside —
/// the caller stamps it onto the final crop at the click point, so the
/// crosshair itself never leaks into the captured image.
/// Any individual monitor that fails to capture is skipped; `Err` is
/// returned only when no monitor could be captured at all.
pub fn capture_all_monitors(
    include_pointer: bool,
) -> Result<(Vec<Capture>, Option<crate::platform::CursorSnapshot>), CaptureError> {
    let list = Monitor::all().map_err(|e| CaptureError::Failed(e.to_string()))?;
    let cursor = include_pointer
        .then(|| {
            let info = list.first().and_then(|m| monitor_info(m).ok())?;
            crate::platform::cursor_snapshot(info.scale_factor)
        })
        .flatten();
    let mut out = Vec::with_capacity(list.len());
    // The OS composites the cursor into `CGWindowListCreateImage` frames:
    // hide it for the duration of the pass so raw frames are always
    // cursor-free — the stamp, when wanted, comes from `cursor` at the
    // selection's click point.
    crate::platform::hide_cursor();
    for m in list {
        let Ok(info) = monitor_info(&m) else {
            continue;
        };
        if let Ok(image) = m.capture_image() {
            out.push(Capture {
                image: std::sync::Arc::new(image),
                scale_factor: info.scale_factor,
                monitor: info,
            });
        }
    }
    crate::platform::unhide_cursor();
    if out.is_empty() {
        return Err(CaptureError::Failed("no monitor captured".into()));
    }
    Ok((out, cursor))
}

/// True when `(x, y)` lies inside `r` (in `MonitorInfo::rect` display units).
pub(crate) fn rect_contains(r: &PhysicalRect, x: f32, y: f32) -> bool {
    let (x0, y0) = (r.x as f32, r.y as f32);
    let (x1, y1) = (
        (r.x + r.width as i32) as f32,
        (r.y + r.height as i32) as f32,
    );
    x >= x0 && x < x1 && y >= y0 && y < y1
}

/// Alpha-composites `snap` onto `image` at the cursor's position inside
/// `bounds` (both `bounds` and `snap.position` live in the OS-native display
/// space; when `image` is higher-DPI than `bounds`, their ratio scales the
/// local coordinates into physical pixels).
pub(crate) fn stamp_cursor(
    image: &mut image::RgbaImage,
    bounds: &PhysicalRect,
    scale_factor: f32,
    snap: &crate::platform::CursorSnapshot,
) {
    stamp_cursor_at(image, bounds, scale_factor, snap, snap.position);
}

/// Like [`stamp_cursor`], but places the snapshot at an explicit point
/// `pos` (global display space) instead of the snapshot's own recorded
/// position — used to stamp the freeze-time cursor onto the final crop
/// at the selection click point, after the overlay's crosshair took over.
pub(crate) fn stamp_cursor_at(
    image: &mut image::RgbaImage,
    bounds: &PhysicalRect,
    scale_factor: f32,
    snap: &crate::platform::CursorSnapshot,
    pos: (f32, f32),
) {
    let _ = scale_factor;
    let sx = if bounds.width > 0 {
        image.width() as f32 / bounds.width as f32
    } else {
        1.0
    };
    let sy = if bounds.height > 0 {
        image.height() as f32 / bounds.height as f32
    } else {
        1.0
    };
    let px = ((pos.0 - bounds.x as f32) * sx).round() as i64 - i64::from(snap.hotspot_px.0);
    let py = ((pos.1 - bounds.y as f32) * sy).round() as i64 - i64::from(snap.hotspot_px.1);
    image::imageops::overlay(image, &snap.image, px, py);
}

/// Crops `rect` (physical pixels, relative to the monitor origin) out of a
/// full-monitor capture. The rect is clamped to the image first — rounding
/// at the UI edge can push a selection a pixel past the frame.
pub fn crop_region(capture: &Capture, rect: PhysicalRect) -> Capture {
    let (w, h) = (capture.image.width(), capture.image.height());
    let x = rect.x.clamp(0, w.saturating_sub(1) as i32) as u32;
    let y = rect.y.clamp(0, h.saturating_sub(1) as i32) as u32;
    let image = image::imageops::crop_imm(
        &*capture.image,
        x,
        y,
        rect.width.min(w - x).max(1),
        rect.height.min(h - y).max(1),
    )
    .to_image();
    Capture {
        image: std::sync::Arc::new(image),
        scale_factor: capture.scale_factor,
        monitor: capture.monitor.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamp_cursor_places_hotspot_on_scaled_pixel() {
        // Logical bounds 100×50 at origin (-200, 10), captured at 2× DPI (200×100 px).
        let mut frame = image::RgbaImage::from_pixel(200, 100, image::Rgba([0, 0, 0, 255]));
        let mut cursor = image::RgbaImage::from_pixel(4, 4, image::Rgba([0, 0, 0, 0]));
        cursor.put_pixel(1, 2, image::Rgba([250, 120, 40, 255]));
        let snap = crate::platform::CursorSnapshot {
            image: cursor,
            position: (-150.0, 35.0), // local (50, 25) pt → (100, 50) px
            hotspot_px: (1, 2),
        };
        let bounds = PhysicalRect {
            x: -200,
            y: 10,
            width: 100,
            height: 50,
        };
        stamp_cursor(&mut frame, &bounds, 2.0, &snap);
        assert_eq!(*frame.get_pixel(100, 50), image::Rgba([250, 120, 40, 255]));
        assert_eq!(*frame.get_pixel(99, 50), image::Rgba([0, 0, 0, 255]));
    }

    /// `stamp_cursor_at` places the snapshot at an explicit point, not the
    /// snapshot's own recorded position — the freeze-time cursor stamped
    /// at the click point after the crosshair took over.
    #[test]
    fn stamp_cursor_at_uses_explicit_position() {
        let mut frame = image::RgbaImage::from_pixel(200, 100, image::Rgba([0, 0, 0, 255]));
        let mut cursor = image::RgbaImage::from_pixel(4, 4, image::Rgba([0, 0, 0, 0]));
        cursor.put_pixel(1, 2, image::Rgba([250, 120, 40, 255]));
        let snap = crate::platform::CursorSnapshot {
            image: cursor,
            position: (-150.0, 35.0), // freeze-time spot — must NOT be used
            hotspot_px: (1, 2),
        };
        let bounds = PhysicalRect {
            x: -200,
            y: 10,
            width: 100,
            height: 50,
        };
        // The click happened at local (60, 30) pt → (120, 60) px, elsewhere
        // than the recorded freeze position.
        stamp_cursor_at(&mut frame, &bounds, 2.0, &snap, (-140.0, 40.0));
        assert_eq!(*frame.get_pixel(120, 60), image::Rgba([250, 120, 40, 255]));
        // The freeze-time position must stay untouched.
        assert_eq!(*frame.get_pixel(100, 50), image::Rgba([0, 0, 0, 255]));
    }
}
