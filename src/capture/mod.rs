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
