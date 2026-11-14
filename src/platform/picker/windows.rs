//! Windows screen sampling for the full-screen pick: `GetDC(NULL)` +
//! `GetPixel` reads of the real framebuffer — virtual-screen pixels,
//! negative coordinates for monitors left/above the primary included.
//! `GetPixel` reads the composed desktop, so the pick overlay's
//! ~8 % veil would tint every sample unless our window opts out: the
//! overlay is marked `WDA_EXCLUDEFROMCAPTURE` on first sight, which
//! makes every capture API (screen DC reads included) see straight
//! through it. The pick's click-swallowing lives in that overlay
//! (`ui::draw::show_pick_overlay`), not in any hook.

use std::ffi::c_void;
use std::ptr;
use std::sync::Mutex;

use eframe::egui::Color32;
use image::{Rgba, RgbaImage};

use super::{PREVIEW_PATCH, colorref_to_color32};

const CLR_INVALID: u32 = 0xFFFF_FFFF;
/// `WDA_EXCLUDEFROMCAPTURE` (Windows 10 2004+): the window is absent
/// from every capture read — older versions just fail the call, which
/// leaves today's behaviour, never a regression.
const WDA_EXCLUDEFROMCAPTURE: u32 = 0x11;

#[repr(C)]
struct POINT {
    x: i32,
    y: i32,
}

#[link(name = "user32")]
unsafe extern "system" {
    fn GetDC(hwnd: *mut c_void) -> *mut c_void;
    fn ReleaseDC(hwnd: *mut c_void, hdc: *mut c_void) -> i32;
    fn WindowFromPoint(pt: POINT) -> *mut c_void;
    fn GetWindowThreadProcessId(hwnd: *mut c_void, pid: *mut u32) -> u32;
    fn GetWindowTextW(hwnd: *mut c_void, buf: *mut u16, len: i32) -> i32;
    fn SetWindowDisplayAffinity(hwnd: *mut c_void, affinity: u32) -> i32;
    fn IsWindow(hwnd: *mut c_void) -> i32;
}

#[link(name = "gdi32")]
unsafe extern "system" {
    fn GetPixel(hdc: *mut c_void, x: i32, y: i32) -> u32;
}

/// Mark the "PixMe Pick" overlay `WDA_EXCLUDEFROMCAPTURE` so our reads
/// see the pixels UNDERNEATH its dim veil. `WindowFromPoint` hands us
/// the topmost HWND at the cursor — ours while the overlay is up — and
/// both the pid and title are checked before the affinity is set, so
/// another of our windows (or a foreign one) is never touched. The
/// HWND is cached; `IsWindow` re-arms the lookup when it is recreated.
fn exclude_overlay_from_capture(x: i32, y: i32) {
    static CACHED: Mutex<Option<usize>> = Mutex::new(None);
    let Ok(mut cached) = CACHED.lock() else {
        return;
    };
    unsafe {
        if let Some(hwnd) = *cached {
            if IsWindow(hwnd as *mut c_void) != 0 {
                return;
            }
            *cached = None;
        }
        let hwnd = WindowFromPoint(POINT { x, y });
        if hwnd.is_null() {
            return;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid != std::process::id() {
            return;
        }
        let mut title = [0u16; 16];
        let n = GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32) as usize;
        if String::from_utf16_lossy(&title[..n.min(title.len())]) != "PixMe Pick" {
            return;
        }
        SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);
        *cached = Some(hwnd as usize);
    }
}

/// The colour of the pixel at `(x, y)` in virtual-screen coordinates —
/// `GetDC(NULL)` spans every monitor, negative coordinates included.
pub fn screen_pixel(x: i32, y: i32) -> Option<Color32> {
    exclude_overlay_from_capture(x, y);
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
    exclude_overlay_from_capture(cx, cy);
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
