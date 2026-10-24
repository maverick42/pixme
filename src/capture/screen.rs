//! Full-screen capture of one monitor. Implemented in phase 3 via `xcap`.

use super::{Capture, CaptureError, MonitorInfo};

/// Captures the full `monitor` frame.
pub fn capture(_monitor: &MonitorInfo) -> Result<Capture, CaptureError> {
    Err(CaptureError::Unsupported("full-screen capture"))
}
