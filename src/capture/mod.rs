//! Screen capture: monitor enumeration and the capture kinds.
//!
//! All OS details (Screen Recording permission on macOS, per-monitor DPI on
//! Windows) stay behind this module and `crate::platform`.

use serde::{Deserialize, Serialize};

pub mod screen;
pub mod window;

/// A captured frame, in **physical** pixels, with the monitor it came from.
#[derive(Clone, Debug)]
pub struct Capture {
    /// RGBA pixels, `width × height` physical pixels. Shared behind an
    /// `Arc` so thumbnail workers (and document clones) borrow the same
    /// buffer instead of duplicating tens of megabytes per job.
    pub image: std::sync::Arc<image::RgbaImage>,
    /// Device pixels per logical point on the source monitor
    /// (2.0 on Retina, per-monitor DPI on Windows).
    pub scale_factor: f32,
    /// Monitor the capture came from.
    pub monitor: MonitorInfo,
}

/// What to capture.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CaptureKind {
    /// Everything on one monitor.
    FullScreen,
    /// A user-picked rectangle, in physical pixels of that monitor.
    Region(PhysicalRect),
    /// One top-level window.
    Window(WindowId),
}

/// A rectangle in physical pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PhysicalRect {
    /// Left edge, px.
    pub x: i32,
    /// Top edge, px.
    pub y: i32,
    /// Width, px.
    pub width: u32,
    /// Height, px.
    pub height: u32,
}

/// Opaque identifier of a top-level OS window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WindowId(pub u64);

/// What kind of display a monitor is — informational only (status-strip
/// icon + tooltip); never drives capture, selection or persistence.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MonitorKind {
    /// A laptop's integrated panel (CGDisplayIsBuiltin / INTERNAL tech).
    BuiltIn,
    /// A regular desktop monitor.
    External,
    /// A tablet used as a display (iPad via Sidecar).
    Tablet,
    /// A virtual/indirect display driver.
    Virtual,
    /// Type not reliably determined — never guessed.
    #[default]
    Unknown,
}

/// One connected monitor.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MonitorInfo {
    /// Stable identifier.
    pub id: u32,
    /// OS display name.
    pub name: String,
    /// Bounds in the OS-native display space — **physical pixels on
    /// Windows** (xcap reads `DEVMODEW` position and `dmPelsWidth/
    /// dmPelsHeight`), **points on macOS** (`CGDisplayBounds`). See
    /// `platform::NATIVE_SPACE`; captured *images* are always
    /// `rect × scale_factor` physical pixels on macOS and already
    /// `rect` physical pixels on Windows.
    pub rect: PhysicalRect,
    /// Device pixels per logical point.
    pub scale_factor: f32,
    /// The primary/main monitor.
    pub is_primary: bool,
}

/// Sorts monitors into spatial order — the canonical list order for every
/// consumer (status strip, inventories, fallback picks). Key: horizontal
/// centre of `rect`, then top edge. Vertically stacked monitors share a
/// centre-x band and therefore stay adjacent inside their column; there is
/// no 1-D order that faithfully renders arbitrary 2-D layouts, but this key
/// keeps the left→right reading correct for rows and columns.
pub fn sort_monitors_by_position(monitors: &mut [MonitorInfo]) {
    monitors.sort_by_key(|m| (m.rect.x as i64 + m.rect.width as i64 / 2, m.rect.y));
}

/// Capture failure surfaced to the user (message via i18n at the UI layer).
#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    /// The OS denied screen recording (macOS TCC, etc.).
    #[error("screen capture permission denied")]
    PermissionDenied,
    /// The requested window no longer exists.
    #[error("window not found")]
    WindowNotFound,
    /// `xcap` reported a failure.
    #[error("capture failed: {0}")]
    Failed(String),
    /// Code path not implemented yet.
    #[error("not implemented: {0}")]
    Unsupported(&'static str),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mon(id: u32, x: i32, y: i32, width: u32, height: u32) -> MonitorInfo {
        MonitorInfo {
            id,
            name: format!("m{id}"),
            rect: PhysicalRect {
                x,
                y,
                width,
                height,
            },
            scale_factor: 1.0,
            is_primary: false,
        }
    }

    fn ids(monitors: &[MonitorInfo]) -> Vec<u32> {
        monitors.iter().map(|m| m.id).collect()
    }

    #[test]
    fn sort_orders_left_of_primary_first() {
        // Sidecar iPad to the left of the primary screen: negative x.
        let mut v = vec![mon(1, 0, 0, 2560, 1440), mon(2, -1133, 200, 1133, 744)];
        sort_monitors_by_position(&mut v);
        assert_eq!(ids(&v), [2, 1]);
    }

    #[test]
    fn sort_orders_three_in_a_row_with_portrait_middle() {
        // Landscape — portrait — landscape: the narrow middle keeps its slot.
        let mut v = vec![
            mon(3, 4000, 0, 2560, 1440),
            mon(1, 0, 0, 2560, 1440),
            mon(2, 2560, -200, 1440, 2560),
        ];
        sort_monitors_by_position(&mut v);
        assert_eq!(ids(&v), [1, 2, 3]);
    }

    #[test]
    fn sort_keeps_stacked_monitors_in_their_column() {
        // A screen stacked above the middle one stays adjacent to it.
        let mut v = vec![
            mon(4, 6560, 0, 2560, 1440),
            mon(2, 2560, 0, 2560, 1440),
            mon(1, 0, 0, 2560, 1440),
            mon(3, 2560, -1440, 2560, 1440),
        ];
        sort_monitors_by_position(&mut v);
        assert_eq!(ids(&v), [1, 3, 2, 4]);
    }

    #[test]
    fn sort_orders_pure_vertical_stack_top_to_bottom() {
        let mut v = vec![mon(2, 0, 1440, 2560, 1440), mon(1, 0, 0, 2560, 1440)];
        sort_monitors_by_position(&mut v);
        assert_eq!(ids(&v), [1, 2]);
    }

    #[test]
    fn sort_orders_grid_column_major() {
        // 2×2 grid: left column top→bottom, then right column.
        let mut v = vec![
            mon(4, 2560, 1440, 2560, 1440),
            mon(2, 0, 1440, 2560, 1440),
            mon(3, 2560, 0, 2560, 1440),
            mon(1, 0, 0, 2560, 1440),
        ];
        sort_monitors_by_position(&mut v);
        assert_eq!(ids(&v), [1, 2, 3, 4]);
    }

    #[test]
    fn sort_single_monitor_is_stable() {
        let mut v = vec![mon(1, 0, 0, 2560, 1440)];
        sort_monitors_by_position(&mut v);
        assert_eq!(ids(&v), [1]);
    }
}
