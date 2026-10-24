//! Single top-level window capture. Implemented in phase 3 via `xcap`.

use super::{Capture, CaptureError, WindowId};

/// Captures the window identified by `id`.
pub fn capture(_id: WindowId) -> Result<Capture, CaptureError> {
    Err(CaptureError::Unsupported("window capture"))
}
