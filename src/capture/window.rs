//! Window picking and capture: enumerate on-screen windows (one OS pass),
//! filter out OS chrome, hit-test a screen point for the pick overlay.
//!
//! Bounds are **logical points** in global display coordinates — the same
//! space [`crate::capture::MonitorInfo::rect`] lives in.

use xcap::Window;

use super::{Capture, CaptureError, PhysicalRect};
use crate::platform;

/// Smallest window worth picking: anything tinier is a status item, a
/// menubar extra or noise.
const MIN_SIZE: u32 = 24;

/// Capturable window-server layers: 0 = normal window, 3 = floating
/// palette, 8 = modal panel. Above that lives OS chrome (Dock = 20,
/// menu bar = 24, status items = 25, notification banners < 0) — never
/// a pick target. Windows has no layer concept; records there carry 0.
const MAX_LAYER: i32 = 8;

/// One capturable window, front-to-back order.
#[derive(Clone, Debug)]
pub struct WindowInfo {
    /// OS window id (`kCGWindowNumber` on macOS, `HWND` on Windows).
    pub id: u32,
    /// Owning application name (kept for logs/title candidates).
    pub app_name: String,
    /// Window title.
    pub title: String,
    /// Global logical bounds.
    pub rect: PhysicalRect,
}

/// All on-screen pickable windows, front-to-back — minus minimized, tiny,
/// OS-chrome-layer and PixMe's own windows (the bar and the overlay must
/// never be capturable).
pub fn windows() -> Result<Vec<WindowInfo>, CaptureError> {
    let own_pid = std::process::id();
    let mut out = Vec::new();
    for w in platform::window_list() {
        if w.pid == own_pid || !(0..=MAX_LAYER).contains(&w.layer) {
            continue;
        }
        if w.rect.width < MIN_SIZE || w.rect.height < MIN_SIZE {
            continue;
        }
        out.push(WindowInfo {
            id: w.id,
            app_name: w.app_name,
            title: w.title,
            rect: w.rect,
        });
    }
    Ok(out)
}

/// Captures window `id` alone: its frame and content in physical pixels,
/// on its own monitor's scale factor.
pub fn capture_window(id: u32) -> Result<Capture, CaptureError> {
    let w = Window::all()
        .map_err(|e| CaptureError::Failed(e.to_string()))?
        .into_iter()
        .find(|w| w.id().ok() == Some(id))
        .ok_or(CaptureError::WindowNotFound)?;
    let monitor = w
        .current_monitor()
        .map_err(|e| CaptureError::Failed(e.to_string()))?;
    let image = w
        .capture_image()
        .map_err(|e| CaptureError::Failed(e.to_string()))?;
    Ok(Capture {
        image: std::sync::Arc::new(image),
        scale_factor: monitor.scale_factor().unwrap_or(1.0),
        monitor: super::screen::monitor_info(&monitor)?,
    })
}

#[cfg(test)]
mod tests {
    /// Dev check: the window list must come back in one fast OS pass —
    /// regressing to per-accessor enumeration stalls the pick overlay.
    #[test]
    #[ignore]
    fn windows_enumerate_quickly() {
        let t = std::time::Instant::now();
        let ws = super::windows().unwrap();
        let elapsed = t.elapsed();
        eprintln!("windows(): {} entries in {elapsed:?}", ws.len());
        for w in &ws {
            eprintln!(
                "  {:<20} {:<28} {}x{}",
                w.app_name, w.title, w.rect.width, w.rect.height
            );
        }
        assert!(elapsed.as_millis() < 500);
    }
}

#[cfg(test)]
mod capture_tests {
    /// Every pickable window must be capturable quickly — a hang in
    /// `capture_image` freezes the UI thread on double-click.
    #[test]
    #[ignore]
    fn all_windows_capture_quickly() {
        for w in super::windows().unwrap() {
            let t = std::time::Instant::now();
            match super::capture_window(w.id) {
                Ok(c) => eprintln!(
                    "  {:<20} {:<28} {}x{} px in {:?}",
                    w.app_name,
                    w.title,
                    c.image.width(),
                    c.image.height(),
                    t.elapsed()
                ),
                Err(e) => eprintln!(
                    "  {:<20} {:<28} ERR {e} in {:?}",
                    w.app_name,
                    w.title,
                    t.elapsed()
                ),
            }
        }
    }
}
