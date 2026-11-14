//! Windows screen sampling for the full-screen pick: `GetDC(NULL)` +
//! `GetPixel` reads of the real framebuffer — virtual-screen pixels,
//! negative coordinates for monitors left/above the primary included.
//! `GetPixel` reads the composed desktop, so the pick overlay's
//! ~8 % veil would tint every sample unless our window opts out: the
//! overlay is marked `WDA_EXCLUDEFROMCAPTURE` on first sight, which
//! makes every capture API (screen DC reads included) see straight
//! through it. The pick's click-swallowing lives in that overlay
//! (`ui::draw::show_pick_overlay`), not in any hook.

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
struct Point {
    x: i32,
    y: i32,
}

// Handles are `isize` here like everywhere else in `src/platform/` —
// one signature for one symbol (clashing_extern_declarations):
// `GetWindowTextW` and `SetWindowDisplayAffinity` are also declared in
// `windows.rs` with `isize` hwnds.
#[link(name = "user32")]
unsafe extern "system" {
    fn GetDC(hwnd: isize) -> isize;
    fn ReleaseDC(hwnd: isize, hdc: isize) -> i32;
    fn WindowFromPoint(pt: Point) -> isize;
    fn GetWindowThreadProcessId(hwnd: isize, pid: *mut u32) -> u32;
    fn GetWindowTextW(hwnd: isize, buf: *mut u16, len: i32) -> i32;
    fn SetWindowDisplayAffinity(hwnd: isize, affinity: u32) -> i32;
    fn IsWindow(hwnd: isize) -> i32;
}

#[link(name = "gdi32")]
unsafe extern "system" {
    fn GetPixel(hdc: isize, x: i32, y: i32) -> u32;
}

/// Mark the "PixMe Pick" overlays `WDA_EXCLUDEFROMCAPTURE` so our reads
/// see the pixels UNDERNEATH their dim veil — there is one overlay
/// window per monitor, so every HWND met at the cursor is marked once
/// and kept in the set. `WindowFromPoint` hands us the topmost HWND —
/// ours while a pick is up — and both the pid and title are checked
/// before the affinity is set, so another of our windows (or a foreign
/// one) is never touched.
fn exclude_overlay_from_capture(x: i32, y: i32) {
    static CACHED: Mutex<Vec<usize>> = Mutex::new(Vec::new());
    let Ok(mut cached) = CACHED.lock() else {
        return;
    };
    unsafe {
        let hwnd = WindowFromPoint(Point { x, y });
        if hwnd == 0 {
            return;
        }
        let h = hwnd as usize;
        if cached.contains(&h) {
            return;
        }
        cached.retain(|&w| IsWindow(w as isize) != 0);
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
        cached.push(h);
    }
}

/// The colour of the pixel at `(x, y)` in virtual-screen coordinates —
/// `GetDC(NULL)` spans every monitor, negative coordinates included.
pub fn screen_pixel(x: i32, y: i32) -> Option<Color32> {
    exclude_overlay_from_capture(x, y);
    unsafe {
        let dc = GetDC(0);
        if dc == 0 {
            return None;
        }
        let v = GetPixel(dc, x, y);
        ReleaseDC(0, dc);
        (v != CLR_INVALID).then(|| colorref_to_color32(v))
    }
}

/// The `PREVIEW_PATCH`×`PREVIEW_PATCH` pixels centred on `(cx, cy)`, for
/// the pick loupe. `CLR_INVALID` cells (off-screen corners) come out
/// transparent black.
pub fn screen_patch(cx: i32, cy: i32) -> Option<RgbaImage> {
    exclude_overlay_from_capture(cx, cy);
    unsafe {
        let dc = GetDC(0);
        if dc == 0 {
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
        ReleaseDC(0, dc);
        Some(patch)
    }
}
