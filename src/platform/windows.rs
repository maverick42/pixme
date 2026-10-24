//! Windows implementation. Phase 3: per-monitor DPI awareness
//! (`PROCESS_PER_MONITOR_DPI_AWARE`), no capture permission needed.

use super::PlatformServices;
use crate::capture::CaptureError;

/// Windows services.
pub struct Windows;

impl PlatformServices for Windows {
    fn ensure_capture_permission(&self) -> Result<(), CaptureError> {
        // Windows needs no screen-capture permission.
        Ok(())
    }
}
