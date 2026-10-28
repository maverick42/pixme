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
