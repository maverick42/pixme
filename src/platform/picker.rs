//! Full-screen colour pick — the "eyedropper over the desktop" armed by
//! the Pipette tool or a swatch's pick button. It samples any pixel on
//! the screen (other apps, the desktop, the menu bar, every monitor)
//! while the Draw window stays put.
//!
//! There is no per-OS session type: the interaction itself is a
//! transparent borderless always-on-top viewport per monitor
//! (`ui::draw::show_pick_overlay`) — a single union-spanning window
//! cannot cover a non-rectangular multi-monitor layout — which catches
//! every click so the application under the cursor never sees the
//! validating press, and paints our own loupe + cursor. This module
//! only provides the OS-level pixel reads and shared pick constants.

use eframe::egui::Color32;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "macos")]
pub use macos::{screen_patch, screen_pixel};
#[cfg(target_os = "windows")]
pub use windows::{screen_patch, screen_pixel};

/// Raises the sampler worker while a pick overlay is up — the Windows
/// desktop read can block on the compositor for tens of milliseconds per
/// call (more under RDP), so it must never run on the UI thread. No-op on
/// macOS, whose framebuffer reads are fast enough to stay synchronous.
#[cfg(target_os = "macos")]
pub fn start_worker() {}
/// Stops the sampler worker (see [`start_worker`]). No-op on macOS.
#[cfg(target_os = "macos")]
pub fn stop_worker() {}
/// Raises the sampler worker while a pick overlay is up (see above).
#[cfg(target_os = "windows")]
pub use windows::start_worker;
/// Stops the sampler worker (see above).
#[cfg(target_os = "windows")]
pub use windows::stop_worker;
/// `true` once per Esc press since the last call — polled physically by
/// the worker, so it lands even while a foreign window owns key focus.
/// No-op on macOS (its overlay takes focus and receives key events).
#[cfg(target_os = "windows")]
pub use windows::take_cancelled;
/// `true` once per `C` press since the last call (see [`take_cancelled`]).
#[cfg(target_os = "windows")]
pub use windows::take_copy;
/// `true` once per Esc press since the last call — no-op on macOS.
#[cfg(target_os = "macos")]
pub fn take_cancelled() -> bool {
    false
}
/// `true` once per `C` press since the last call — no-op on macOS.
#[cfg(target_os = "macos")]
pub fn take_copy() -> bool {
    false
}

/// Side of the square screen patch sampled for the pick loupe, in
/// OS-native screen units (points on macOS, pixels on Windows). The
/// returned image may be denser on Retina — the centre pixel is always
/// the sampled point.
pub const PREVIEW_PATCH: u32 = 11;

/// AppKit needs a nonzero alpha to intercept clicks. DWM transparency
/// does not: Windows keeps a normal hit-testable HWND without a veil
/// tinting the desktop or capture protection hiding the loupe over RDP.
#[cfg(target_os = "macos")]
pub const OVERLAY_VEIL_ALPHA: u8 = 20;
#[cfg(target_os = "windows")]
pub const OVERLAY_VEIL_ALPHA: u8 = 0;

/// Window title shared by every per-monitor pick overlay viewport —
/// matched by `elevate_pick_overlay` and by the per-OS screen samplers
/// so our own veil is excluded from pixel reads.
pub const PICK_OVERLAY_TITLE: &str = "PixMe Pick";

/// The `egui::Id` the pick overlay writes its [`PickOutcome`] into and
/// the Draw session drains — the two live in different viewports, so
/// ctx data is the channel.
pub const PICK_SCREEN_RESULT: &str = "draw.pick_screen_result";
/// `Instant` of the last `C` copy during a pick — shared via ctx data
/// so the overlay loupe flashes the same "copied!" hint as the canvas
/// loupe.
pub const PICK_COPIED_AT: &str = "draw.pick_copied_at";

/// The end state of a pick — written by the overlay viewport into the
/// shared ctx data, drained by the Draw session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickOutcome {
    /// A click landed: the colour under the cursor.
    Picked(Color32),
    /// `C` pressed while the overlay held key focus: the sampled colour —
    /// the session applies AND copies it, but the pick does not end.
    CopyPressed(Color32),
    /// Esc or a right click — nothing was applied.
    Cancelled,
    /// The OS refused to hand out screen pixels (macOS without Screen
    /// Recording) — the UI points at System Settings.
    Denied,
}

/// A `COLORREF` (`0x00BBGGRR`, low byte = red) as an opaque `Color32`.
#[cfg(any(target_os = "windows", test))]
pub(crate) fn colorref_to_color32(v: u32) -> Color32 {
    Color32::from_rgb(
        (v & 0xFF) as u8,
        ((v >> 8) & 0xFF) as u8,
        ((v >> 16) & 0xFF) as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colorref_is_bgr_ordered() {
        // COLORREF packs red in the LOW byte.
        assert_eq!(
            colorref_to_color32(0x00FF0000),
            Color32::from_rgb(0, 0, 255)
        );
        assert_eq!(
            colorref_to_color32(0x0000FF00),
            Color32::from_rgb(0, 255, 0)
        );
        assert_eq!(
            colorref_to_color32(0x000000FF),
            Color32::from_rgb(255, 0, 0)
        );
        assert_eq!(colorref_to_color32(0x00FFFFFF), Color32::WHITE);
    }
}
