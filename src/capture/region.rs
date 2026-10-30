//! Rectangular region capture.
//!
//! Phase 3 contract: freeze the screen first (one full-screen [`Capture`]),
//! show a borderless always-on-top viewport with that texture under
//! [`crate::ui::theme::REGION_SCRIM`], let the user drag a rect, then crop the
//! frozen image. The selection overlay never re-captures.

use super::{Capture, CaptureError, PhysicalRect};

/// Crops the frozen full-screen `capture` to `region` (physical px).
/// UI + freeze logic land in phase 3.
pub fn crop(_capture: &Capture, _region: PhysicalRect) -> Result<Capture, CaptureError> {
    Err(CaptureError::Unsupported("region crop"))
}
