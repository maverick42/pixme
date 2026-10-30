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
            crate::app::wake_viewports(&repaint_ctx);
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

/// Opens `dir` in Explorer — the Settings "open folder" buttons.
pub fn open_folder(dir: &std::path::Path) {
    let _ = std::process::Command::new("explorer").arg(dir).status();
}

/// Portable layout: `<exe dir>\data` + `<exe dir>\logs` when the
/// writability probe passes (creation of both dirs plus a uniquely
/// named probe file); a read-only install (`C:\Program Files`, a
/// network share without write rights) falls back to
/// `%LOCALAPPDATA%\PixMe\{data,logs}` and reports `fallback_from` so
/// the user gets the deferred notice.
///
/// The exe dir comes from `current_exe()` — never the working
/// directory, which shortcuts freely change — with `\\?\` /
/// `\\?\UNC\` verbatim prefixes stripped. Note: a network or
/// removable-drive install keeps working, but SQLite there relies on
/// its default rollback journal (no WAL — see docs/decisions.md).
pub fn resolve_base_dirs() -> crate::paths::BaseDirs {
    let fallback = local_app_data();
    let Some(exe_dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|d| crate::paths::strip_verbatim(d)))
    else {
        // No exe path at all — degraded but survivable.
        return crate::paths::BaseDirs {
            data: fallback.join("data"),
            logs: fallback.join("logs"),
            fallback_from: None,
        };
    };
    let writable = crate::paths::probe_writable(&exe_dir.join("data"), &exe_dir.join("logs"));
    crate::paths::pick_dirs(&exe_dir, &fallback, writable)
}

/// `%LOCALAPPDATA%\PixMe` — the non-portable fallback root.
fn local_app_data() -> std::path::PathBuf {
    directories::BaseDirs::new()
        .map(|b| b.data_local_dir().join("pixme"))
        .or_else(|| std::env::var_os("LOCALAPPDATA").map(|v| Path::new(&v).join("pixme")))
        .unwrap_or_else(|| std::env::temp_dir().join("pixme"))
}

/// "Windows 10.0.19045" style — `cmd /c ver` output trimmed to the
/// version field; ~30 ms once at startup, no extra dependency.
pub fn os_version() -> String {
    let out = std::process::Command::new("cmd")
        .args(["/c", "ver"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_default();
    // "Microsoft Windows [Version 10.0.19045.4780]" → "Windows 10.0.19045.4780".
    out.rsplit_once('[')
        .and_then(|(_, v)| {
            v.strip_prefix("Version ")
                .map(|v| v.trim_end_matches(']').to_owned())
        })
        .map(|v| format!("Windows {v}"))
        .unwrap_or_else(|| "Windows ?".to_owned())
}

/// Shows the native Windows print dialog (`PrintDlgW` — printer picker,
/// page range, copies) and prints `img` fit to the printable page via
/// GDI `StretchDIBits` on the chosen printer's DC — no helper process.
/// `job_title` names the job in the spooler queue. Runs modally on the
/// calling (main) thread. `Err` covers both cancel and failure.
pub fn print_image(img: &image::RgbaImage, job_title: &str) -> Result<(), String> {
    use std::ffi::c_void;

    // PRINTDLG flags.
    const PD_ALLPAGES: u32 = 0x0;
    const PD_NOSELECTION: u32 = 0x4;
    const PD_NOPAGENUMS: u32 = 0x8;
    const PD_RETURNDC: u32 = 0x100;
    const PD_USEDEVMODECOPIESANDCOLLATE: u32 = 0x40000;
    // GetDeviceCaps indices.
    const PHYSICALWIDTH: i32 = 110;
    const PHYSICALHEIGHT: i32 = 111;
    const PHYSICALOFFSETX: i32 = 112;
    const PHYSICALOFFSETY: i32 = 113;
    const DIB_RGB_COLORS: u32 = 0;
    const BI_RGB: u32 = 0;
    const SRCCOPY: u32 = 0x00CC0020;
    const GDI_ERROR: i32 = -1;

    #[repr(C)]
    struct PrintDlg {
        lstruct_size: u32,
        hwnd_owner: isize,
        h_dev_mode: isize,
        h_dev_names: isize,
        h_dc: isize,
        flags: u32,
        n_from_page: u16,
        n_to_page: u16,
        n_min_page: u16,
        n_max_page: u16,
        n_copies: u16,
        h_instance: isize,
        l_cust_data: isize,
        lpfn_print_hook: usize,
        lpfn_setup_hook: usize,
        lp_print_template_name: *const u16,
        lp_setup_template_name: *const u16,
        h_print_template: isize,
        h_setup_template: isize,
    }

    #[repr(C)]
    struct DocInfo {
        cb_size: i32,
        lpsz_doc_name: *const u16,
        lpsz_output: *const u16,
        lpsz_datatype: *const u16,
        fw_type: u32,
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

    #[repr(C)]
    struct BitmapInfo {
        bmi_header: BitmapInfoHeader,
        bmi_colors: [u32; 1],
    }

    #[link(name = "comdlg32")]
    unsafe extern "system" {
        fn PrintDlgW(pd: *mut PrintDlg) -> i32;
        fn CommDlgExtendedError() -> u32;
    }
    #[link(name = "gdi32")]
    unsafe extern "system" {
        fn GetDeviceCaps(hdc: isize, index: i32) -> i32;
        fn StartDocW(hdc: isize, di: *const DocInfo) -> i32;
        fn StartPage(hdc: isize) -> i32;
        fn EndPage(hdc: isize) -> i32;
        fn EndDoc(hdc: isize) -> i32;
        fn AbortDoc(hdc: isize) -> i32;
        #[allow(clippy::too_many_arguments)]
        fn StretchDIBits(
            hdc: isize,
            x_dest: i32,
            y_dest: i32,
            w_dest: i32,
            h_dest: i32,
            x_src: i32,
            y_src: i32,
            w_src: i32,
            h_src: i32,
            bits: *const c_void,
            bmi: *const BitmapInfo,
            usage: u32,
            rop: u32,
        ) -> i32;
        fn DeleteDC(hdc: isize) -> i32;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GlobalFree(mem: isize) -> isize;
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetActiveWindow() -> isize;
        fn GetForegroundWindow() -> isize;
    }

    let title_w: Vec<u16> = job_title.encode_utf16().chain(std::iter::once(0)).collect();

    unsafe {
        // The dialog needs an owner hwnd so it stays above the calling
        // window; the viewer (or pin) is active while printing.
        let mut owner = GetActiveWindow();
        if owner == 0 {
            owner = GetForegroundWindow();
        }
        let mut pd = PrintDlg {
            lstruct_size: std::mem::size_of::<PrintDlg>() as u32,
            hwnd_owner: owner,
            h_dev_mode: 0,
            h_dev_names: 0,
            h_dc: 0,
            flags: PD_RETURNDC
                | PD_ALLPAGES
                | PD_NOSELECTION
                | PD_NOPAGENUMS
                | PD_USEDEVMODECOPIESANDCOLLATE,
            n_from_page: 1,
            n_to_page: 1,
            n_min_page: 1,
            n_max_page: u16::MAX,
            n_copies: 1,
            h_instance: 0,
            l_cust_data: 0,
            lpfn_print_hook: 0,
            lpfn_setup_hook: 0,
            lp_print_template_name: std::ptr::null(),
            lp_setup_template_name: std::ptr::null(),
            h_print_template: 0,
            h_setup_template: 0,
        };
        if PrintDlgW(&mut pd) == 0 {
            return Err(if CommDlgExtendedError() == 0 {
                "print dialog cancelled".into()
            } else {
                format!("PrintDlg failed ({:#x})", CommDlgExtendedError())
            });
        }
        // The common dialog allocates these; the printer DC is ours to
        // delete after the job is spooled.
        struct Global(isize);
        impl Drop for Global {
            fn drop(&mut self) {
                unsafe {
                    if self.0 != 0 {
                        GlobalFree(self.0);
                    }
                }
            }
        }
        let _dev_mode = Global(pd.h_dev_mode);
        let _dev_names = Global(pd.h_dev_names);

        let hdc = pd.h_dc;
        let page_w = GetDeviceCaps(hdc, PHYSICALWIDTH).max(1);
        let page_h = GetDeviceCaps(hdc, PHYSICALHEIGHT).max(1);
        let off_x = GetDeviceCaps(hdc, PHYSICALOFFSETX).max(0);
        let off_y = GetDeviceCaps(hdc, PHYSICALOFFSETY).max(0);
        let (iw, ih) = (img.width().max(1), img.height().max(1));
        // Fit the whole image on one page, centred inside the printable
        // margins (same intent as NSFitPagination on macOS).
        let scale = (page_w as f32 / iw as f32).min(page_h as f32 / ih as f32);
        let dw = ((iw as f32 * scale).round() as i32).max(1);
        let dh = ((ih as f32 * scale).round() as i32).max(1);
        let dx = off_x + (page_w - dw).max(0) / 2;
        let dy = off_y + (page_h - dh).max(0) / 2;

        // GDI 32-bpp DIBs read B,G,R,A per pixel — swap R and B, alpha
        // forced opaque (BI_RGB ignores the byte anyway).
        let mut bgra = Vec::with_capacity((iw * ih * 4) as usize);
        for px in img.pixels() {
            bgra.extend_from_slice(&[px[2], px[1], px[0], 255]);
        }
        let bmi = BitmapInfo {
            bmi_header: BitmapInfoHeader {
                bi_size: std::mem::size_of::<BitmapInfoHeader>() as u32,
                bi_width: iw as i32,
                // Negative height = top-down row order.
                bi_height: -(ih as i32),
                bi_planes: 1,
                bi_bit_count: 32,
                bi_compression: BI_RGB,
                bi_size_image: bgra.len() as u32,
                bi_x_pels_per_meter: 0,
                bi_y_pels_per_meter: 0,
                bi_clr_used: 0,
                bi_clr_important: 0,
            },
            bmi_colors: [0],
        };
        let doc = DocInfo {
            cb_size: std::mem::size_of::<DocInfo>() as i32,
            lpsz_doc_name: title_w.as_ptr(),
            lpsz_output: std::ptr::null(),
            lpsz_datatype: std::ptr::null(),
            fw_type: 0,
        };

        let printed = StartDocW(hdc, &doc) > 0
            && StartPage(hdc) > 0
            && StretchDIBits(
                hdc,
                dx,
                dy,
                dw,
                dh,
                0,
                0,
                iw as i32,
                ih as i32,
                bgra.as_ptr() as *const c_void,
                &bmi,
                DIB_RGB_COLORS,
                SRCCOPY,
            ) != GDI_ERROR
            && EndPage(hdc) > 0;
        if printed {
            EndDoc(hdc);
        } else {
            AbortDoc(hdc);
        }
        DeleteDC(hdc);

        if printed {
            Ok(())
        } else {
            Err("GDI print job failed".into())
        }
    }
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
/// selection overlay owns pointer input. Returns `false` — `EndMenu` posts
/// no key into our own input, nothing to swallow.
pub fn dismiss_active_menus() -> bool {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn EndMenu() -> i32;
    }
    unsafe {
        EndMenu();
    }
    false
}

/// No-op on Windows (`with_always_on_top()` already places the overlay at
/// `HWND_TOPMOST`).
pub fn elevate_overlay() {}
