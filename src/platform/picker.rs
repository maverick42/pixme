//! Full-screen colour pick — the "eyedropper over the desktop" armed by
//! the Pipette tool or a swatch's pick button. It samples any pixel on
//! the screen (other apps, the desktop, the menu bar, every monitor)
//! while the Draw window stays put.
//!
//! There is no per-OS session type: the interaction itself is a
//! transparent borderless always-on-top viewport covering the virtual
//! screen (`ui::draw::show_pick_overlay`), which catches every click —
//! the application under the cursor never sees the validating press —
//! and paints our own loupe + cursor. This module only provides the
//! OS-level pixel reads and the union rect the overlay covers.

use eframe::egui::Color32;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "macos")]
pub use macos::{screen_patch, screen_pixel};
#[cfg(target_os = "windows")]
pub use windows::{screen_patch, screen_pixel};

/// Side of the square screen patch sampled for the pick loupe, in
/// OS-native screen units (points on macOS, pixels on Windows). The
/// returned image may be denser on Retina — the centre pixel is always
/// the sampled point.
pub const PREVIEW_PATCH: u32 = 11;

/// The `egui::Id` the pick overlay writes its [`PickOutcome`] into and
/// the Draw session drains — the two live in different viewports, so
/// ctx data is the channel.
pub const PICK_SCREEN_RESULT: &str = "draw.pick_screen_result";
/// `(Instant, bool)` heartbeat the Draw session rewrites every pass —
/// the overlay self-closes when it turns false or goes stale (>500 ms),
/// so a vanished Draw session can never strand a click-eating veil.
pub const PICK_OVERLAY_ALIVE: &str = "draw.pick_overlay_alive";

/// The end state of a pick — written by the overlay viewport into the
/// shared ctx data, drained by the Draw session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickOutcome {
    /// A click landed: the colour under the cursor.
    Picked(Color32),
    /// Esc or a right click — nothing was applied.
    Cancelled,
    /// The OS refused to hand out screen pixels (macOS without Screen
    /// Recording) — the UI points at System Settings.
    Denied,
}

/// The union of every connected monitor's rect in OS-native screen units
/// `[left, top, right, bottom)` — the area the pick overlay must cover so
/// no click escapes it.
pub fn screens_union_native() -> Option<(i32, i32, i32, i32)> {
    let mut it = crate::capture::screen::monitors().ok()?.into_iter();
    let first = it.next()?;
    let (mut l, mut t, mut r, mut b) = (
        first.rect.x,
        first.rect.y,
        first.rect.x + first.rect.width as i32,
        first.rect.y + first.rect.height as i32,
    );
    for m in it {
        l = l.min(m.rect.x);
        t = t.min(m.rect.y);
        r = r.max(m.rect.x + m.rect.width as i32);
        b = b.max(m.rect.y + m.rect.height as i32);
    }
    Some((l, t, r, b))
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

    #[test]
    fn union_covers_every_monitor() {
        // With monitors unavailable in CI this is a no-op; otherwise the
        // union must contain each individual rect.
        if let Some((l, t, r, b)) = screens_union_native() {
            for m in crate::capture::screen::monitors().unwrap_or_default() {
                assert!(m.rect.x >= l && m.rect.y >= t);
                assert!(m.rect.x + m.rect.width as i32 <= r);
                assert!(m.rect.y + m.rect.height as i32 <= b);
            }
        }
    }
}
