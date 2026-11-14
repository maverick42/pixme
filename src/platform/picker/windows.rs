//! Windows screen sampling for the full-screen pick: `GetDC(NULL)` +
//! `GetPixel` reads of the real framebuffer — never of a layered/overlay
//! window, so the sampled colour is exact. The process is already
//! Per-Monitor DPI aware (winit / Phase-3), so the coordinates are plain
//! virtual-screen pixels — negative positions for monitors left/above
//! the primary included. The pick's click-swallowing lives in the
//! transparent fullscreen overlay (`ui::draw::show_pick_overlay`), not
//! in any hook: the press lands on our window by construction.

use std::ffi::c_void;
use std::ptr;

use eframe::egui::Color32;
use image::{Rgba, RgbaImage};

use super::{PREVIEW_PATCH, colorref_to_color32};

const CLR_INVALID: u32 = 0xFFFF_FFFF;

#[link(name = "user32")]
unsafe extern "system" {
    fn GetDC(hwnd: *mut c_void) -> *mut c_void;
    fn ReleaseDC(hwnd: *mut c_void, hdc: *mut c_void) -> i32;
}

#[link(name = "gdi32")]
unsafe extern "system" {
    fn GetPixel(hdc: *mut c_void, x: i32, y: i32) -> u32;
}

/// The colour of the pixel at `(x, y)` in virtual-screen coordinates —
/// `GetDC(NULL)` spans every monitor, negative coordinates included.
/// Reads the real screen, never a layered/overlay window, so the sampled
/// colour is exact.
pub fn screen_pixel(x: i32, y: i32) -> Option<Color32> {
    unsafe {
        let dc = GetDC(ptr::null_mut());
        if dc.is_null() {
            return None;
        }
        let v = GetPixel(dc, x, y);
        ReleaseDC(ptr::null_mut(), dc);
        (v != CLR_INVALID).then(|| colorref_to_color32(v))
    }
}

/// The `PREVIEW_PATCH`×`PREVIEW_PATCH` pixels centred on `(cx, cy)`, for
/// the pick loupe. `CLR_INVALID` cells (off-screen corners) come out
/// transparent black.
pub fn screen_patch(cx: i32, cy: i32) -> Option<RgbaImage> {
    unsafe {
        let dc = GetDC(ptr::null_mut());
        if dc.is_null() {
            return None;
        }
        let half = (PREVIEW_PATCH / 2) as i32;
        let mut patch = RgbaImage::new(PREVIEW_PATCH, PREVIEW_PATCH);
        for dy in 0..PREVIEW_PATCH as i32 {
            for dx in 0..PREVIEW_PATCH as i32 {
                let v = GetPixel(dc, cx + dx - half, cy + dy - half);
                let px = if v == CLR_INVALID {
                    Rgba([0, 0, 0, 0])
                } else {
                    let c = colorref_to_color32(v);
                    Rgba([c.r(), c.g(), c.b(), 255])
                };
                patch.put_pixel(dx as u32, dy as u32, px);
            }
        }
        ReleaseDC(ptr::null_mut(), dc);
        Some(patch)
    }
}
