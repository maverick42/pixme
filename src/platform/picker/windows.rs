//! Windows screen sampling for the full-screen pick.
//!
//! Desktop pixel reads (`GetDC(NULL)` + `BitBlt`/`GetPixel`) synchronize
//! with the compositor, which can take tens of milliseconds per call —
//! more under RDP or with the fullscreen overlay composed on top. Those
//! reads therefore live on a dedicated worker thread: while a pick is
//! armed it follows the OS cursor and keeps a fresh `PREVIEW_PATCH` patch
//! in a shared slot. The UI-thread-facing [`screen_patch`] and
//! [`screen_pixel`] are non-blocking reads of that slot, so no paint or
//! click ever waits on the compositor.
//!
//! The overlay is DWM-transparent (alpha 0, still hit-testable) and needs
//! no capture protection — a `WDA_EXCLUDEFROMCAPTURE` veil would also
//! hide it from RDP sessions. Only the offset loupe is painted, away from
//! the sampled pixels.

use core::ffi::c_void;
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use eframe::egui::Color32;
use image::{Rgba, RgbaImage};

use super::PREVIEW_PATCH;

const CLR_INVALID: u32 = 0xFFFF_FFFF;
const SRCCOPY: u32 = 0x00CC_0020;
const SM_XVIRTUALSCREEN: i32 = 76;
const SM_YVIRTUALSCREEN: i32 = 77;
const SM_CXVIRTUALSCREEN: i32 = 78;
const SM_CYVIRTUALSCREEN: i32 = 79;
/// How long the click path waits for the worker to sample the exact
/// click position (a rare, discrete action — never the paint path).
const CLICK_WAIT: Duration = Duration::from_millis(120);

#[repr(C)]
struct BitmapInfoHeader {
    bi_size: u32,
    bi_width: i32,
    bi_height: i32,
    bi_planes: u16,
    bi_bit_count: u16,
    bi_compression: u32,
    bi_size_image: u32,
    bi_x_pels_per_meter: i32,
    bi_y_pels_per_meter: i32,
    bi_clr_used: u32,
    bi_clr_important: u32,
}

// Handles match the signatures declared in `src/platform/windows.rs` —
// one signature per symbol across the crate (`clashing_extern_declarations`).
#[link(name = "user32")]
unsafe extern "system" {
    fn GetDC(hwnd: isize) -> isize;
    fn ReleaseDC(hwnd: isize, hdc: isize) -> i32;
    fn GetSystemMetrics(index: i32) -> i32;
    fn GetAsyncKeyState(v_key: i32) -> i16;
}

#[link(name = "gdi32")]
unsafe extern "system" {
    fn GetPixel(hdc: isize, x: i32, y: i32) -> u32;
    fn CreateCompatibleDC(hdc: *mut c_void) -> *mut c_void;
    fn DeleteDC(hdc: *mut c_void) -> i32;
    fn SelectObject(hdc: *mut c_void, h: *mut c_void) -> *mut c_void;
    fn DeleteObject(ho: *mut c_void) -> i32;
    fn CreateDIBSection(
        hdc: *mut c_void,
        pbmi: *const BitmapInfoHeader,
        usage: u32,
        ppv_bits: *mut *mut u8,
        h_section: *mut c_void,
        offset: u32,
    ) -> *mut c_void;
    fn BitBlt(
        hdc: *mut c_void,
        x: i32,
        y: i32,
        cx: i32,
        cy: i32,
        hdc_src: *mut c_void,
        x1: i32,
        y1: i32,
        rop: u32,
    ) -> i32;
    fn GdiFlush() -> i32;
}

/// The latest worker sample: the patch centred on the cursor, the exact
/// colour of its centre pixel and the position it was taken at.
struct Snapshot {
    at: Instant,
    pos: (i32, i32),
    patch: RgbaImage,
    colour: Color32,
}

static SNAP: OnceLock<(Mutex<Option<Snapshot>>, Condvar)> = OnceLock::new();
/// `true` while the worker must keep sampling. The overlay starts it when
/// it raises and stops it when it closes (see `start_worker`/`stop_worker`).
static ALIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static WORKER: OnceLock<Mutex<Option<std::thread::JoinHandle<()>>>> = OnceLock::new();
/// Cancel/copy presses latched by the worker's key poll. bit0 = Esc,
/// bit1 = C. The overlay is raised exactly while the cursor roams over
/// OTHER windows, so key focus is ambiguous there: `GetAsyncKeyState`
/// sees the physical press regardless of which window owns the queue —
/// egui key events still work too (both are OR'd by the caller).
static KEYS: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
const VK_ESCAPE: i32 = 0x1B;
const VK_C: i32 = 0x43;

fn snap() -> &'static (Mutex<Option<Snapshot>>, Condvar) {
    SNAP.get_or_init(|| (Mutex::new(None), Condvar::new()))
}

/// Raises the sampler worker while the pick overlay is up: it follows the
/// cursor and keeps the snapshot fresh, so the UI thread never performs a
/// blocking compositor read.
pub fn start_worker() {
    let running = ALIVE.swap(true, std::sync::atomic::Ordering::SeqCst);
    if running {
        return;
    }
    let mut guard = WORKER
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    if guard.is_some() {
        return;
    }
    // Keys latched during the previous worker's shutdown tail must not
    // fire into the next pick (an Esc pressed as the overlay died would
    // otherwise cancel it instantly).
    KEYS.store(0, std::sync::atomic::Ordering::SeqCst);
    let handle = std::thread::Builder::new()
        .name("pixme-pick-sampler".into())
        .spawn(sampler_loop)
        .expect("spawn pick sampler");
    *guard = Some(handle);
    tracing::debug!("pick sampler: started");
}

/// Stops the sampler worker once the overlay is gone. The worker exits on
/// its own after the current read: the UI thread never joins a thread
/// that may be mid-compositor-read (that is the freeze to avoid).
pub fn stop_worker() {
    ALIVE.store(false, std::sync::atomic::Ordering::SeqCst);
    let mut guard = WORKER
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    if let Some(handle) = guard.take() {
        std::mem::drop(handle);
    }
}

fn sampler_loop() {
    let mut last = None::<(i32, i32)>;
    let mut prev = 0i16;
    while ALIVE.load(std::sync::atomic::Ordering::SeqCst) {
        let pos =
            crate::platform::cursor_position().map(|(x, y)| (x.round() as i32, y.round() as i32));
        // Sample ONLY when the cursor moved: continuous desktop reads
        // (even on a worker thread) keep the compositor busy and freeze
        // RDP sessions. A stationary cursor's patch stays valid.
        if pos.is_some_and(|p| last != Some(p)) {
            let (x, y) = pos.expect("checked");
            let patch = read_patch(x, y);
            if let Some(patch) = patch {
                let centre = patch.get_pixel(PREVIEW_PATCH / 2, PREVIEW_PATCH / 2).0;
                let colour = Color32::from_rgba_unmultiplied(centre[0], centre[1], centre[2], 255);
                let (m, cv) = snap();
                {
                    let mut slot = m.lock().unwrap_or_else(|p| p.into_inner());
                    *slot = Some(Snapshot {
                        at: Instant::now(),
                        pos: (x, y),
                        patch,
                        colour,
                    });
                }
                cv.notify_all();
                last = Some((x, y));
            }
        }
        // Edge-detect Esc/C (MSB = currently down) so they latch even
        // while another window holds key focus — the common case while
        // the cursor roams outside our windows.
        let now = unsafe { (GetAsyncKeyState(VK_ESCAPE) < 0, GetAsyncKeyState(VK_C) < 0) };
        let mut bits = 0u8;
        if now.0 && prev & 1 == 0 {
            bits |= 1;
        }
        if now.1 && prev & 2 == 0 {
            bits |= 2;
        }
        prev = (now.0 as i16) | ((now.1 as i16) << 1);
        if bits != 0 {
            tracing::debug!(bits, "pick sampler: key latched");
            KEYS.fetch_or(bits, std::sync::atomic::Ordering::SeqCst);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    tracing::debug!("pick sampler: exited");
}

/// `true` once per Esc press seen by the worker (drains the latch).
/// macOS relies on the focused-window key path — no-op there.
pub fn take_cancelled() -> bool {
    KEYS.fetch_and(!1u8, std::sync::atomic::Ordering::SeqCst) & 1 != 0
}

/// `true` once per `C` press seen by the worker (drains the latch).
/// macOS relies on the focused-window key path — no-op there.
pub fn take_copy() -> bool {
    KEYS.fetch_and(!2u8, std::sync::atomic::Ordering::SeqCst) & 2 != 0
}

fn cursor_pos() -> Option<(i32, i32)> {
    crate::platform::cursor_position().map(|(x, y)| (x.round() as i32, y.round() as i32))
}

/// The worker's latest snapshot — the paint path reads it without ever
/// waiting (a 33 ms cadence must not block on the compositor). The worker
/// samples on every cursor move, so this trails by at most one read. The
/// snapshot is COPIED, never consumed: the pick keeps painting from it
/// until the cursor moves again.
fn latest_snapshot() -> Option<Snapshot> {
    let (m, _) = snap();
    let slot = m.lock().unwrap_or_else(|p| p.into_inner());
    slot.as_ref().map(|s| Snapshot {
        at: s.at,
        pos: s.pos,
        patch: s.patch.clone(),
        colour: s.colour,
    })
}

/// The colour under the cursor — read from the worker's snapshot. The
/// click path waits briefly for a sample at the exact click position
/// (bounded; the worker reads on cursor move).
pub fn screen_pixel(x: i32, y: i32) -> Option<Color32> {
    let (m, cv) = snap();
    let slot = m.lock().unwrap_or_else(|p| p.into_inner());
    if slot.as_ref().is_some_and(|s| s.pos == (x, y)) {
        return slot.as_ref().map(|s| s.colour);
    }
    let (g, _t) = cv
        .wait_timeout(slot, CLICK_WAIT)
        .unwrap_or_else(|p| p.into_inner());
    g.as_ref().map(|s| s.colour)
}

/// The `PREVIEW_PATCH`×`PREVIEW_PATCH` pixels centred on the cursor — the
/// worker's cached patch, again never a blocking read here.
pub fn screen_patch(_cx: i32, _cy: i32) -> Option<RgbaImage> {
    latest_snapshot().map(|s| s.patch)
}

/// A raw `PREVIEW_PATCH`×`PREVIEW_PATCH` read centred on `(cx, cy)` in
/// virtual-screen coordinates, executed by the sampler thread. One
/// `BitBlt` fills a top-down 32-bpp DIB; off-screen cells come out
/// transparent black.
fn read_patch(cx: i32, cy: i32) -> Option<RgbaImage> {
    let side = PREVIEW_PATCH as i32;
    let half = side / 2;
    let req_x0 = cx - half;
    let req_y0 = cy - half;
    let req_x1 = req_x0 + side;
    let req_y1 = req_y0 + side;

    unsafe {
        let vx0 = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let vy0 = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let vx1 = vx0 + GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let vy1 = vy0 + GetSystemMetrics(SM_CYVIRTUALSCREEN);
        let sx0 = req_x0.max(vx0);
        let sy0 = req_y0.max(vy0);
        let sx1 = req_x1.min(vx1);
        let sy1 = req_y1.min(vy1);
        let sw = sx1 - sx0;
        let sh = sy1 - sy0;

        let mut patch = RgbaImage::new(PREVIEW_PATCH, PREVIEW_PATCH);
        if sw <= 0 || sh <= 0 {
            return Some(patch);
        }

        let screen_dc = GetDC(0);
        if screen_dc == 0 {
            return None;
        }
        let mem_dc = CreateCompatibleDC(screen_dc as *mut c_void);
        if mem_dc.is_null() {
            ReleaseDC(0, screen_dc);
            return None;
        }
        let bmi = BitmapInfoHeader {
            bi_size: size_of::<BitmapInfoHeader>() as u32,
            bi_width: side,
            bi_height: -side, // top-down
            bi_planes: 1,
            bi_bit_count: 32,
            bi_compression: 0,
            bi_size_image: 0,
            bi_x_pels_per_meter: 0,
            bi_y_pels_per_meter: 0,
            bi_clr_used: 0,
            bi_clr_important: 0,
        };
        let mut bits: *mut u8 = core::ptr::null_mut();
        let hbm = CreateDIBSection(mem_dc, &bmi, 0, &mut bits, core::ptr::null_mut(), 0);
        if hbm.is_null() || bits.is_null() {
            if !hbm.is_null() {
                DeleteObject(hbm);
            }
            DeleteDC(mem_dc);
            ReleaseDC(0, screen_dc);
            return None;
        }
        let old = SelectObject(mem_dc, hbm);
        if old.is_null() || old as isize == -1 {
            DeleteObject(hbm);
            DeleteDC(mem_dc);
            ReleaseDC(0, screen_dc);
            return None;
        }
        let dx0 = sx0 - req_x0;
        let dy0 = sy0 - req_y0;
        let ok = BitBlt(
            mem_dc,
            dx0,
            dy0,
            sw,
            sh,
            screen_dc as *mut c_void,
            sx0,
            sy0,
            SRCCOPY,
        );
        // DIB memory must not be read before GDI's pending writes finish.
        let flushed = GdiFlush();
        if ok != 0 && flushed != 0 {
            let slice =
                core::slice::from_raw_parts(bits, (PREVIEW_PATCH * PREVIEW_PATCH * 4) as usize);
            for dy in dy0..(dy0 + sh) {
                for dx in dx0..(dx0 + sw) {
                    let i = ((dy * side + dx) * 4) as usize;
                    // 32-bpp BI_RGB DIB is B, G, R, X in memory.
                    patch.put_pixel(dx as u32, dy as u32, bgra_pixel(&slice[i..i + 4]));
                }
            }
        }
        SelectObject(mem_dc, old);
        DeleteObject(hbm);
        DeleteDC(mem_dc);
        ReleaseDC(0, screen_dc);
        (ok != 0 && flushed != 0).then_some(patch)
    }
}

fn bgra_pixel(bytes: &[u8]) -> Rgba<u8> {
    Rgba([bytes[2], bytes[1], bytes[0], 255])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dib_pixels_are_bgr_with_unused_alpha() {
        assert_eq!(
            bgra_pixel(&[0x80, 0x3A, 0xCD, 0]),
            Rgba([0xCD, 0x3A, 0x80, 255])
        );
        assert_eq!(bgra_pixel(&[0, 0, 0, 42]), Rgba([0, 0, 0, 255]));
        assert_eq!(size_of::<BitmapInfoHeader>(), 40);
    }

    #[test]
    #[ignore = "requires an interactive Windows desktop"]
    fn desktop_patch_has_fixed_size_at_screen_edge() {
        let x = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
        let y = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
        let patch = read_patch(x, y).expect("desktop read failed");
        assert_eq!(patch.dimensions(), (PREVIEW_PATCH, PREVIEW_PATCH));
        assert_eq!(patch.get_pixel(0, 0).0, [0, 0, 0, 0]);
        assert_eq!(
            patch.get_pixel(PREVIEW_PATCH / 2, PREVIEW_PATCH / 2).0[3],
            255
        );
    }
}
