//! Windows implementation: notification-area tray icon with the same two
//! menu actions as the macOS status item ("Take a capture", "Open PixMe").
//! There is no global menu bar on Windows — the viewer renders its own egui
//! menu strip instead (see `ui::viewer`), so menus stay reachable with the
//! same commands.
//!
//! Tray/menu handles are `Rc`-backed (not `Send`): they live in a
//! `thread_local` owned by the main thread instead of app state.
//!
//! Phase 3: per-monitor DPI awareness (`PROCESS_PER_MONITOR_DPI_AWARE`).

use std::cell::RefCell;

use rust_i18n::t;
use tray_icon::TrayIcon;
use tray_icon::menu::{Menu, MenuEvent, MenuItem};

use super::PlatformServices;
use crate::capture::CaptureError;
use crate::ui::icons;

/// Windows services.
pub struct Windows;

impl PlatformServices for Windows {
    fn ensure_capture_permission(&self) -> Result<(), CaptureError> {
        // Windows needs no screen-capture permission.
        Ok(())
    }

    fn has_native_app_menu(&self) -> bool {
        // Menus are drawn inside the viewer window (egui menu strip).
        false
    }
}

/// Windows parity with the permanent macOS Dock icon: the invisible anchor
/// viewport keeps a taskbar button (clicking it focuses the app, shows
/// nothing — same dead behaviour as the launcher's Dock icon). The viewer
/// still gets its own taskbar button while open.
pub fn apply_native_options(options: &mut eframe::NativeOptions) {
    options.viewport = options
        .viewport
        .clone()
        .with_taskbar(true)
        .with_icon(icons::app_icon_data());
}

/// Tray icon edge in pixels (16 px slot × ~2 for high-DPI trays).
const TRAY_ICON_PX: u32 = 32;

/// Menu handles kept alive for the app lifetime, plus the i18n keys needed
/// to relabel them when the language changes.
struct Installed {
    _tray: Option<TrayIcon>,
    _menu: Menu,
    items: Vec<(MenuItem, &'static str, &'static str)>,
}

thread_local! {
    static INSTALLED: RefCell<Option<Installed>> = const { RefCell::new(None) };
}

/// Installs the tray icon and the menu-event handler forwarding clicks as
/// [`crate::app::Command`]s.
pub fn install_ui(ctx: &egui::Context) {
    let repaint_ctx = ctx.clone();
    MenuEvent::set_event_handler(Some(move |event| {
        if let Some(command) = super::command_from_menu_id(event.id().0.as_str()) {
            super::queue_menu_command(command);
            repaint_ctx.request_repaint();
        }
    }));

    let menu = Menu::new();
    let mut items = Vec::new();
    for (id, key) in [
        ("take_capture", "menubar.take_capture"),
        ("open_viewer", "menubar.open_viewer"),
    ] {
        let item = MenuItem::with_id(id, t!(key).as_ref(), true, None);
        if let Err(err) = menu.append(&item) {
            tracing::warn!("cannot append tray menu item: {err}");
        }
        items.push((item, key, id));
    }

    let mut builder = TrayIcon::builder()
        .with_menu(Box::new(menu.clone()))
        .with_menu_on_left_click(true)
        .with_tooltip("PixMe");
    // The Windows tray has no template mode: use the full-colour artwork
    // (readable on light and dark taskbars alike).
    match tray_icon::Icon::from_rgba(
        icons::app_icon_rgba(TRAY_ICON_PX),
        TRAY_ICON_PX,
        TRAY_ICON_PX,
    ) {
        Ok(icon) => builder = builder.with_icon(icon),
        Err(err) => tracing::warn!("tray icon unavailable: {err}"),
    }
    let tray = match builder.build() {
        Ok(tray) => Some(tray),
        Err(err) => {
            tracing::warn!("cannot create tray icon: {err}");
            None
        }
    };

    INSTALLED.with(|cell| {
        *cell.borrow_mut() = Some(Installed {
            _tray: tray,
            _menu: menu,
            items,
        });
    });
}

/// All on-screen windows, front-to-back. On Windows `xcap` enumerates
/// real top-level windows with direct `EnumWindows`/`GetWindow*` calls —
/// no per-accessor re-enumeration like the macOS backend, so reading
/// through it is cheap. `layer` is always 0 (no layer concept).
pub fn window_list() -> Vec<super::WindowRecord> {
    let mut out = Vec::new();
    for w in xcap::Window::all().unwrap_or_default() {
        let Ok(rect) = (|| {
            Ok::<crate::capture::PhysicalRect, xcap::XCapError>(crate::capture::PhysicalRect {
                x: w.x()?,
                y: w.y()?,
                width: w.width()?,
                height: w.height()?,
            })
        })() else {
            continue;
        };
        out.push(super::WindowRecord {
            id: w.id().unwrap_or_default(),
            pid: w.pid().unwrap_or_default(),
            app_name: w.app_name().unwrap_or_default(),
            title: w.title().unwrap_or_default(),
            rect,
            layer: 0,
        });
    }
    out
}

/// Rewrites the tray menu labels in the current language.
pub fn relabel_menus() {
    INSTALLED.with(|cell| {
        if let Some(installed) = &*cell.borrow() {
            for (item, key, _id) in &installed.items {
                item.set_text(t!(*key).as_ref());
            }
        }
    });
}

/// Greys out tray menu items whose command is forbidden under `modal`
/// (same [`crate::app::is_command_enabled`] table as the macOS menus).
/// Called from `Shared::pump`, always on the main thread.
pub fn sync_menu_enabled(modal: crate::app::ModalState) {
    INSTALLED.with(|cell| {
        if let Some(installed) = &*cell.borrow() {
            for (item, _key, id) in &installed.items {
                if let Some(command) = super::command_from_menu_id(id) {
                    item.set_enabled(crate::app::is_command_enabled(&command, modal));
                }
            }
        }
    });
}

/// Reveals `path` selected in Explorer (`explorer /select,<path>`).
pub fn reveal_in_file_manager(path: &std::path::Path) {
    let _ = std::process::Command::new("explorer")
        .arg(format!("/select,{}", path.display()))
        .status();
}

/// Prints `path` on the default printer via `mspaint /p`. V1 has no
/// print dialog — a richer path (printer picker, margins) lands later.
pub fn print_file(path: &std::path::Path) -> Result<(), String> {
    std::process::Command::new("mspaint")
        .arg("/p")
        .arg(path)
        .status()
        .map_err(|e| e.to_string())
        .and_then(|s| {
            if s.success() {
                Ok(())
            } else {
                Err(format!("mspaint exited with {s}"))
            }
        })
}

/// Global mouse position in the OS-native display space (virtual-screen
/// physical pixels — the exact space `MonitorInfo::rect` uses).
pub fn cursor_position() -> Option<(f32, f32)> {
    #[repr(C)]
    struct Point {
        x: i32,
        y: i32,
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetCursorPos(point: *mut Point) -> i32;
    }
    let mut pt = Point { x: 0, y: 0 };
    if unsafe { GetCursorPos(&mut pt) } != 0 {
        Some((pt.x as f32, pt.y as f32))
    } else {
        None
    }
}

/// One short tick — the system "Asterisk" sound via `MessageBeep`
/// (respects the sound scheme and volume).
pub fn tick() {
    const MB_ICONASTERISK: u32 = 0x40;
    #[link(name = "user32")]
    unsafe extern "system" {
        fn MessageBeep(u_type: u32) -> i32;
    }
    unsafe {
        MessageBeep(MB_ICONASTERISK);
    }
}

/// Captures the current OS mouse cursor via `GetCursorInfo` + `GetIconInfo`
/// + `DrawIconEx` into a 32-bit DIB section. Returns `None` when the cursor
/// is hidden or cannot be queried.
pub fn cursor_snapshot(_scale_factor: f32) -> Option<super::CursorSnapshot> {
    use core::ffi::c_void;

    const CURSOR_SHOWING: u32 = 0x0000_0001;
    const DI_NORMAL: u32 = 0x0003;
    const SM_CXCURSOR: i32 = 13;
    const SM_CYCURSOR: i32 = 14;

    #[repr(C)]
    struct Point {
        x: i32,
        y: i32,
    }
    #[repr(C)]
    struct CursorInfo {
        cb_size: u32,
        flags: u32,
        h_cursor: *mut c_void,
        pt_screen_pos: Point,
    }
    #[repr(C)]
    struct IconInfo {
        f_icon: i32,
        x_hotspot: u32,
        y_hotspot: u32,
        hbm_mask: *mut c_void,
        hbm_color: *mut c_void,
    }
    #[repr(C)]
    struct Bitmap {
        bm_type: i32,
        bm_width: i32,
        bm_height: i32,
        bm_width_bytes: i32,
        bm_planes: u16,
        bm_bits_pixel: u16,
        bm_bits: *mut c_void,
    }
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

    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetCursorInfo(pci: *mut CursorInfo) -> i32;
        fn GetIconInfo(h_icon: *mut c_void, piconinfo: *mut IconInfo) -> i32;
        fn GetSystemMetrics(index: i32) -> i32;
        fn DrawIconEx(
            hdc: *mut c_void,
            x_left: i32,
            y_top: i32,
            h_icon: *mut c_void,
            cx_width: i32,
            cy_width: i32,
            istep_if_ani_cur: u32,
            hbr_flicker_free_draw: *mut c_void,
            di_flags: u32,
        ) -> i32;
    }
    #[link(name = "gdi32")]
    unsafe extern "system" {
        fn CreateCompatibleDC(hdc: *mut c_void) -> *mut c_void;
        fn DeleteDC(hdc: *mut c_void) -> i32;
        fn SelectObject(hdc: *mut c_void, h: *mut c_void) -> *mut c_void;
        fn DeleteObject(ho: *mut c_void) -> i32;
        fn GetObjectW(h: *mut c_void, c: i32, pv: *mut c_void) -> i32;
        fn CreateDIBSection(
            hdc: *mut c_void,
            pbmi: *const BitmapInfoHeader,
            usage: u32,
            ppv_bits: *mut *mut u8,
            h_section: *mut c_void,
            offset: u32,
        ) -> *mut c_void;
    }

    unsafe {
        let mut ci = CursorInfo {
            cb_size: size_of::<CursorInfo>() as u32,
            flags: 0,
            h_cursor: core::ptr::null_mut(),
            pt_screen_pos: Point { x: 0, y: 0 },
        };
        if GetCursorInfo(&mut ci) == 0 || (ci.flags & CURSOR_SHOWING) == 0 || ci.h_cursor.is_null()
        {
            return None;
        }
        let mut ii = IconInfo {
            f_icon: 0,
            x_hotspot: 0,
            y_hotspot: 0,
            hbm_mask: core::ptr::null_mut(),
            hbm_color: core::ptr::null_mut(),
        };
        if GetIconInfo(ci.h_cursor, &mut ii) == 0 {
            return None;
        }
        let mut w = GetSystemMetrics(SM_CXCURSOR).max(32);
        let mut h = GetSystemMetrics(SM_CYCURSOR).max(32);
        let mut bm = core::mem::zeroed::<Bitmap>();
        if !ii.hbm_color.is_null()
            && GetObjectW(
                ii.hbm_color,
                size_of::<Bitmap>() as i32,
                (&mut bm as *mut Bitmap).cast(),
            ) != 0
            && bm.bm_width > 0
            && bm.bm_height > 0
        {
            w = bm.bm_width;
            h = bm.bm_height;
        }
        if !ii.hbm_mask.is_null() {
            DeleteObject(ii.hbm_mask);
        }
        if !ii.hbm_color.is_null() {
            DeleteObject(ii.hbm_color);
        }

        let render_dib = |fill: u8| -> Option<Vec<u8>> {
            let hdc = CreateCompatibleDC(core::ptr::null_mut());
            if hdc.is_null() {
                return None;
            }
            let bmi = BitmapInfoHeader {
                bi_size: size_of::<BitmapInfoHeader>() as u32,
                bi_width: w,
                bi_height: -h, // top-down
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
            let hbm = CreateDIBSection(hdc, &bmi, 0, &mut bits, core::ptr::null_mut(), 0);
            if hbm.is_null() || bits.is_null() {
                DeleteDC(hdc);
                return None;
            }
            let len = (w as usize) * (h as usize) * 4;
            core::slice::from_raw_parts_mut(bits, len).fill(fill);
            let old = SelectObject(hdc, hbm);
            let ok = DrawIconEx(
                hdc,
                0,
                0,
                ci.h_cursor,
                w,
                h,
                0,
                core::ptr::null_mut(),
                DI_NORMAL,
            );
            let out = (ok != 0).then(|| core::slice::from_raw_parts(bits, len).to_vec());
            SelectObject(hdc, old);
            DeleteObject(hbm);
            DeleteDC(hdc);
            out
        };

        let on_black = render_dib(0)?;
        let on_white = render_dib(255)?;
        let mut rgba = vec![0u8; on_black.len()];
        let any_native_alpha = on_black.as_chunks::<4>().0.iter().any(|px| px[3] != 0);
        for ((dst, b), wpx) in rgba
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(on_black.as_chunks::<4>().0)
            .zip(on_white.as_chunks::<4>().0)
        {
            if any_native_alpha {
                let a = b[3] as u32;
                if a > 0 {
                    dst[0] = ((b[2] as u32 * 255 + a / 2) / a).min(255) as u8;
                    dst[1] = ((b[1] as u32 * 255 + a / 2) / a).min(255) as u8;
                    dst[2] = ((b[0] as u32 * 255 + a / 2) / a).min(255) as u8;
                    dst[3] = a as u8;
                }
            } else if b[0] != wpx[0] || b[1] != wpx[1] || b[2] != wpx[2] {
                // Monochrome/XOR cursor (e.g. I-beam): stays transparent.
            } else {
                dst[0] = b[2];
                dst[1] = b[1];
                dst[2] = b[0];
                dst[3] = 255;
            }
        }
        let image = image::RgbaImage::from_raw(w as u32, h as u32, rgba)?;
        Some(super::CursorSnapshot {
            image,
            position: (ci.pt_screen_pos.x as f32, ci.pt_screen_pos.y as f32),
            hotspot_px: (ii.x_hotspot as i32, ii.y_hotspot as i32),
        })
    }
}

/// Hides the OS cursor so a screen capture does not bake it into the frame
/// (`BitBlt`-style captures composite it on their own). `ShowCursor` is a
/// counter — always balance [`hide_cursor`] with [`unhide_cursor`].
pub fn hide_cursor() {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn ShowCursor(show: i32) -> i32;
    }
    unsafe {
        ShowCursor(0);
    }
}

/// Balances [`hide_cursor`] (see above).
pub fn unhide_cursor() {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn ShowCursor(show: i32) -> i32;
    }
    unsafe {
        ShowCursor(1);
    }
}

/// Ends any active Win32 popup menu once the screen has been frozen so the
/// selection overlay owns pointer input.
pub fn dismiss_active_menus() {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn EndMenu() -> i32;
    }
    unsafe {
        EndMenu();
    }
}

/// No-op on Windows (`with_always_on_top()` already places the overlay at
/// `HWND_TOPMOST`).
pub fn elevate_overlay() {}
