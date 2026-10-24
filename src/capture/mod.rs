//! Screen capture: monitor enumeration and the three capture kinds.
//!
//! Contract only — implementations land in phase 3 on top of `xcap`. All OS
//! details (Screen Recording permission on macOS, per-monitor DPI on Windows)
//! stay behind this module and `crate::platform`.

pub mod region;
pub mod screen;
pub mod window;

/// A captured frame, in **physical** pixels, with the monitor it came from.
#[derive(Clone, Debug)]
pub struct Capture {
    /// RGBA pixels, `width × height` physical pixels.
    pub image: image::RgbaImage,
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
#[derive(Clone, Copy, Debug, Default, PartialEq)]
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

/// One connected monitor.
#[derive(Clone, Debug)]
pub struct MonitorInfo {
    /// Stable identifier.
    pub id: u32,
    /// OS display name.
    pub name: String,
    /// Bounds in physical pixels.
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

/// The capture back-end contract. One implementation per OS family, obtained
/// from [`crate::platform`].
pub trait Capturer: Send + Sync {
    /// All connected monitors.
    fn monitors(&self) -> Result<Vec<MonitorInfo>, CaptureError>;
    /// Captures `kind` on `monitor`.
    fn capture(&self, kind: CaptureKind, monitor: &MonitorInfo) -> Result<Capture, CaptureError>;
}
