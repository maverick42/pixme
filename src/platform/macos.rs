//! macOS implementation. Phase 3: Screen Recording permission via
//! `CGPreflightScreenCaptureAccess`, Retina point/pixel conversion.

use super::PlatformServices;
use crate::capture::CaptureError;

/// macOS services.
pub struct MacOs;

impl PlatformServices for MacOs {
    fn ensure_capture_permission(&self) -> Result<(), CaptureError> {
        // Phase 3: CGPreflightScreenCaptureAccess / CGRequestScreenCaptureAccess.
        Ok(())
    }
}
