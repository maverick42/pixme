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
use std::path::Path;

use rust_i18n::t;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconEvent};

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
}

/// The 1×1 invisible anchor viewport gets NO taskbar button: on Windows a
/// resident app lives in the notification area (the tray icon), and an
/// extra taskbar entry would surface an empty ghost window in the taskbar
/// and Alt+Tab. The viewer still gets its own taskbar button while open.
pub fn apply_native_options(options: &mut eframe::NativeOptions) {
    options.viewport = options.viewport.clone().with_icon(icons::app_icon_data());
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
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        if let Some(command) = super::command_from_menu_id(event.id().0.as_str()) {
            super::queue_menu_command(command);
            crate::app::wake_viewports(&repaint_ctx);
        }
    }));
    let tray_ctx = ctx.clone();
    TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
        // Left click toggles the viewer; the menu stays on the right
        // click (`with_menu_on_left_click(false)` below).
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Down,
            ..
        } = event
        {
            super::queue_menu_command(crate::app::Command::ToggleViewer);
            crate::app::wake_viewports(&tray_ctx);
        }
    }));

    fn add(
        menu: &Menu,
        items: &mut Vec<(MenuItem, &'static str, &'static str)>,
        id: &'static str,
        key: &'static str,
    ) {
        let item = MenuItem::with_id(id, t!(key).as_ref(), true, None);
        if let Err(err) = menu.append(&item) {
            tracing::warn!("cannot append tray menu item: {err}");
        }
        items.push((item, key, id));
    }

    let menu = Menu::new();
    let mut items = Vec::new();
    add(&menu, &mut items, "open_viewer", "menubar.open_viewer");
    add(&menu, &mut items, "settings", "menu.app.preferences");
    {
        // Greyed until an updater exists — its id maps to no command.
        let item = MenuItem::with_id(
            "check_updates",
            t!("menubar.check_updates").as_ref(),
            false,
            None,
        );
        if let Err(err) = menu.append(&item) {
            tracing::warn!("cannot append tray menu item: {err}");
        }
        items.push((item, "menubar.check_updates", "check_updates"));
    }
    add(
        &menu,
        &mut items,
        "reset_monitors",
        "menubar.reset_monitors",
    );
    if let Err(err) = menu.append(&PredefinedMenuItem::separator()) {
        tracing::warn!("cannot append tray menu separator: {err}");
    }
    add(&menu, &mut items, "quit", "menubar.quit");

    let mut builder = TrayIcon::builder()
        .with_menu(Box::new(menu.clone()))
        // Left click toggles the viewer (TrayIconEvent); the menu is
        // for the right click.
        .with_menu_on_left_click(false)
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
pub fn sync_menu_enabled(
    modal: crate::app::ModalState,
    _has_doc: bool,
    _has_file: bool,
    _has_last_dir: bool,
) {
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

// Strips `WS_CAPTION` and shapes undecorated floating windows (#2246).
//
// ## Known Upstream Bugs (winit / DWM)
//
// 1. **Content offset (31 px down)**: `winit` 0.30 forces `WS_CAPTION | WS_BORDER`
//    on all windows, even with `decorations: false`, and relies on `WM_NCCALCSIZE`
//    to hide them. On Windows with Intel graphics drivers or OpenGL, the desktop
//    compositor still reserves space for the caption, shifting content ~31 px
//    down and clipping it at the window bottom (emilk/egui#2770, libsdl#12105).
//    This reproduces the fix from PhotoCraft (`window_frame.rs`, issue #2246):
//    stripping `WS_CHROME` removes the non-client area so no offset occurs.
//
// 2. **DWM 1 px top line**: on Windows 11, DWM draws an accent line at y=0 on
//    borderless windows (documented in `winit::platform::windows::WindowExtWindows::set_undecorated_shadow`:
//    *"Enabling the shadow causes a thin 1px line to appear on the top of the window"*).
//    Applying a native Win32 window region (`SetWindowRgn`) tells DWM the true
//    geometry of the window so nothing is drawn outside it:
//    - For the countdown (`"PixMe Countdown"`): it carries a 14 pt
//      transparent `SHADOW_MARGIN`, so clipping y=0..2 removes the DWM
//      line without touching the card or its soft shadow.
//    - For pinned captures (`"PixMe Pin"`): sets a rounded-rect region (`16 px`),
//      matching the card's rounded corners and preserving the full border stroke.
//    - For the capture overlay (`"PixMe Overlay"`): clipping is impossible —
//      the top pixel rows carry real frozen-frame content that must stay
//      visible and selectable. `DWMWA_BORDER_COLOR = DWMWA_COLOR_NONE`
//      suppresses the border itself instead (Windows 11; ignored elsewhere).
//
// 3. **Ghost window in the frozen frame**: xcap grabs the screen with a GDI
//    `BitBlt` of the composed desktop. On slower machines the composition can
//    still hold a PixMe window's pixels when the freeze fires ~2 frames after
//    its viewport is torn down — the viewer then appears as a watermark under
//    the dim veil. `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` redacts
//    PixMe windows from the readback — but it is toggled TRANSIENTLY, only
//    for the capture's duration (see [`capture_exclusion`]): a permanent
//    exclusion hid PixMe from Teams and every other capture tool. Every role
//    is excluded EXCEPT the pins: a pinned capture is meant to be screen
//    content and must photograph. Best-effort — `WDA_EXCLUDEFROMCAPTURE`
//    needs Windows 10 2004+; the call fails harmlessly on older builds.
//
// Note: whenever winit touches window flags (focus change, new child window),
// it triggers `SWP_FRAMECHANGED` which resets the window region. The guard
// reapplies `SetWindowRgn` whenever styles are stripped or dimensions change.
thread_local! {
    /// Hwnds classified as pins — they represent screen content and
    /// must stay capturable. Populated by the window pass in
    /// [`strip_undecorated_frame`]; a pin not yet classified (created
    /// since the last pass) is still skipped by its live "PixMe Pin"
    /// title in [`set_capture_exclusion`].
    static PIN_HWNDS: std::cell::RefCell<std::collections::HashSet<isize>> =
        std::cell::RefCell::new(std::collections::HashSet::new());
}

/// Enables/disables `WDA_EXCLUDEFROMCAPTURE` on every PixMe window
/// except pins, by enumerating this thread's windows LIVE — a window
/// created since the last `strip_undecorated_frame` pass is covered
/// too (no registry staleness). Returns how many hwnds were toggled.
/// Best-effort: ignored on Windows builds before 10 2004.
fn set_capture_exclusion(exclude: bool) -> u32 {
    const WDA_EXCLUDEFROMCAPTURE: u32 = 0x0000_0011;
    const WDA_NONE: u32 = 0;
    #[link(name = "user32")]
    unsafe extern "system" {
        fn SetWindowDisplayAffinity(hwnd: isize, affinity: u32) -> i32;
        fn EnumThreadWindows(
            id_thread: u32,
            lpfn: unsafe extern "system" fn(isize, isize) -> i32,
            lparam: isize,
        ) -> i32;
        fn GetCurrentThreadId() -> u32;
        fn GetWindowTextW(hwnd: isize, s: *mut u16, n: i32) -> i32;
    }
    struct Ctx {
        affinity: u32,
        applied: u32,
    }
    unsafe extern "system" fn visit(hwnd: isize, lp: isize) -> i32 {
        let ctx = unsafe { &mut *(lp as *mut Ctx) };
        let pinned = PIN_HWNDS.with(|s| s.borrow().contains(&hwnd)) || {
            // A pin created since the last classification pass still
            // carries its live title — skip it the same way.
            let mut buf = [0u16; 64];
            let n = unsafe { GetWindowTextW(hwnd, buf.as_mut_ptr(), 64) } as usize;
            String::from_utf16_lossy(&buf[..n]) == "PixMe Pin"
        };
        if !pinned {
            unsafe { SetWindowDisplayAffinity(hwnd, ctx.affinity) };
            ctx.applied += 1;
        }
        1
    }
    let mut ctx = Ctx {
        affinity: if exclude {
            WDA_EXCLUDEFROMCAPTURE
        } else {
            WDA_NONE
        },
        applied: 0,
    };
    unsafe {
        EnumThreadWindows(GetCurrentThreadId(), visit, &mut ctx as *mut Ctx as isize);
    }
    ctx.applied
}

/// RAII guard armed by [`capture_exclusion`]: the exclusion lifts on
/// drop — error and panic paths included.
pub struct CaptureExclusionGuard {
    armed_at: std::time::Instant,
}

/// Activates `WDA_EXCLUDEFROMCAPTURE` on every PixMe window except
/// pins, then waits out one DWM composition (`DwmFlush`) so the flag
/// is guaranteed effective before the caller's `BitBlt` — GDI reads
/// the composed frame, so a still-tearing-down viewer would otherwise
/// ghost into the freeze. The exclusion lifts when the returned guard
/// drops: screen sharing and other capture tools keep seeing PixMe.
pub fn capture_exclusion() -> CaptureExclusionGuard {
    let armed_at = std::time::Instant::now();
    let hwnds = set_capture_exclusion(true);
    let flush_t = std::time::Instant::now();
    #[link(name = "dwmapi")]
    unsafe extern "system" {
        fn DwmFlush() -> i32;
    }
    let hr = unsafe { DwmFlush() };
    tracing::debug!(
        hwnds,
        flush_ms = flush_t.elapsed().as_secs_f64() * 1e3,
        flush_hr = hr,
        "capture_exclusion_armed"
    );
    CaptureExclusionGuard { armed_at }
}

impl Drop for CaptureExclusionGuard {
    fn drop(&mut self) {
        let lifted = set_capture_exclusion(false);
        tracing::debug!(
            active_ms = self.armed_at.elapsed().as_secs_f64() * 1e3,
            lifted_hwnds = lifted,
            "capture_exclusion_lifted"
        );
    }
}

pub fn strip_undecorated_frame() {
    use std::collections::HashMap;

    const GWL_STYLE: i32 = -16;
    const GWL_EXSTYLE: i32 = -20;
    const WS_EX_TOPMOST: isize = 0x8;
    const WS_CAPTION: isize = 0x00C0_0000;
    const WS_CHROME: isize = WS_CAPTION | 0x0008_0000 | 0x0002_0000 | 0x0001_0000; // caption, sysmenu, min, max
    const WS_EX_WINDOWEDGE: isize = 0x0000_0400;
    // SWP_NOSIZE | SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED | SWP_NOCOPYBITS
    const SWP_FLAGS: u32 = 0x0001 | 0x0002 | 0x0004 | 0x0010 | 0x0020 | 0x0100;

    #[repr(C)]
    struct WinRect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn EnumThreadWindows(
            tid: u32,
            f: unsafe extern "system" fn(isize, isize) -> i32,
            p: isize,
        ) -> i32;
        fn GetWindowLongPtrW(hwnd: isize, idx: i32) -> isize;
        fn SetWindowLongPtrW(hwnd: isize, idx: i32, v: isize) -> isize;
        fn SetWindowPos(
            hwnd: isize,
            after: isize,
            x: i32,
            y: i32,
            cx: i32,
            cy: i32,
            flags: u32,
        ) -> i32;
        fn GetWindowTextW(hwnd: isize, s: *mut u16, n: i32) -> i32;
        fn SetWindowTextW(hwnd: isize, s: *const u16) -> i32;
        fn GetWindowRect(hwnd: isize, r: *mut WinRect) -> i32;
        fn SetWindowRgn(hwnd: isize, hrgn: isize, redraw: i32) -> i32;
        fn IsWindowVisible(hwnd: isize) -> i32;
        fn ShowWindow(hwnd: isize, cmd: i32) -> i32;
        fn GetCurrentThreadId() -> u32;
    }

    #[link(name = "gdi32")]
    unsafe extern "system" {
        fn CreateRectRgn(x1: i32, y1: i32, x2: i32, y2: i32) -> isize;
        fn CreateRoundRectRgn(x1: i32, y1: i32, x2: i32, y2: i32, cx: i32, cy: i32) -> isize;
    }

    #[link(name = "dwmapi")]
    unsafe extern "system" {
        fn DwmSetWindowAttribute(hwnd: isize, attr: u32, value: *const u32, size: u32) -> i32;
    }

    /// `DWMWA_BORDER_COLOR` (dwmapi.h).
    const DWMWA_BORDER_COLOR: u32 = 34;
    /// `DWMWA_COLOR_NONE`: suppresses the DWM border entirely.
    const DWMWA_COLOR_NONE: u32 = 0xFFFF_FFFE;
    // `SetWindowDisplayAffinity` is declared in the file-level
    // `set_capture_exclusion` — the exclusion is transient now.

    const SW_SHOWNOACTIVATE: i32 = 4;
    const SW_SHOW: i32 = 5;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Role {
        Countdown,
        Pin,
        Overlay,
        /// The invisible 1×1 root anchor ("PixMe" + WS_EX_TOPMOST — the
        /// viewer is not topmost, so no other "PixMe" window qualifies).
        Anchor,
        /// The per-monitor colour-pick veil ("PixMe Pick"): transparent,
        /// topmost, monitor-pinned — revealed here like every chromeless
        /// window.
        Pick,
    }

    thread_local! {
        static REGIONS: RefCell<HashMap<isize, (i32, i32)>> = RefCell::new(HashMap::new());
        static ROLES: RefCell<HashMap<isize, Role>> = RefCell::new(HashMap::new());
    }

    unsafe extern "system" fn visit(hwnd: isize, _lp: isize) -> i32 {
        let exstyle = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) };
        let topmost = (exstyle & WS_EX_TOPMOST) != 0;

        let style = unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) };

        let mut title_buf = [0u16; 64];
        let n = unsafe { GetWindowTextW(hwnd, title_buf.as_mut_ptr(), 64) as usize };
        let title = String::from_utf16_lossy(&title_buf[..n]);

        let (role, first_seen) = ROLES.with(|roles| {
            let mut m = roles.borrow_mut();
            if let Some(&r) = m.get(&hwnd) {
                return (Some(r), false);
            }
            // Only topmost windows get a role: the viewer shares the
            // "PixMe" title with the anchor but is NOT topmost — letting
            // it through the title switch would tag it Anchor.
            if !topmost {
                return (None, false);
            }
            if title == "PixMe Countdown" {
                m.insert(hwnd, Role::Countdown);
                unsafe { SetWindowTextW(hwnd, [0u16].as_ptr()) };
                (Some(Role::Countdown), true)
            } else if title == "PixMe Pin" {
                m.insert(hwnd, Role::Pin);
                unsafe { SetWindowTextW(hwnd, [0u16].as_ptr()) };
                (Some(Role::Pin), true)
            } else if title == "PixMe Overlay" {
                m.insert(hwnd, Role::Overlay);
                unsafe { SetWindowTextW(hwnd, [0u16].as_ptr()) };
                (Some(Role::Overlay), true)
            } else if title == crate::platform::picker::PICK_OVERLAY_TITLE {
                m.insert(hwnd, Role::Pick);
                unsafe { SetWindowTextW(hwnd, [0u16].as_ptr()) };
                (Some(Role::Pick), true)
            } else if title == "PixMe" {
                m.insert(hwnd, Role::Anchor);
                unsafe { SetWindowTextW(hwnd, [0u16].as_ptr()) };
                (Some(Role::Anchor), true)
            } else {
                (None, false)
            }
        });

        // Pins represent screen content: registered so the transient
        // capture exclusion always skips them (the live title check
        // inside `set_capture_exclusion` covers a pin not yet
        // classified).
        if role == Some(Role::Pin) {
            PIN_HWNDS.with(|s| {
                s.borrow_mut().insert(hwnd);
            });
        }

        if !topmost {
            return 1;
        }

        let Some(role) = role else {
            return 1;
        };

        if matches!(role, Role::Overlay | Role::Pick) && first_seen {
            // The taskbar is itself TOPMOST: a later-created topmost
            // window wins the band anyway, but re-asserting precedence
            // here guarantees the frozen frame stays selectable edge to
            // edge (the capture must reach the taskbar's clock).
            unsafe {
                SetWindowPos(hwnd, -1, 0, 0, 0, 0, 0x0001 | 0x0002 | 0x0010);
            }
        }

        if matches!(role, Role::Overlay | Role::Pick) && (first_seen || (style & WS_CHROME) != 0) {
            // Suppress DWM's 1 px accent line at the window's top edge —
            // SetWindowRgn (the countdown/pin fix) would clip real
            // frozen-frame rows. Best-effort: Windows 11 only.
            unsafe {
                DwmSetWindowAttribute(
                    hwnd,
                    DWMWA_BORDER_COLOR,
                    &DWMWA_COLOR_NONE,
                    std::mem::size_of::<u32>() as u32,
                );
            }
        }

        let mut need_reapply_region = false;
        if (style & WS_CHROME) != 0 {
            // A window caught with chrome while VISIBLE is a title-bar
            // flash in the act: creation race or mid-life re-decoration
            // (winit re-applies WS_CAPTION on frame changes).
            if unsafe { IsWindowVisible(hwnd) } != 0 {
                tracing::debug!(hwnd, ?role, "chrome_stripped_on_visible_window");
            }
            unsafe {
                SetWindowLongPtrW(hwnd, GWL_STYLE, style & !WS_CHROME);
                if (exstyle & WS_EX_WINDOWEDGE) != 0 {
                    SetWindowLongPtrW(hwnd, GWL_EXSTYLE, exstyle & !WS_EX_WINDOWEDGE);
                }
                SetWindowPos(hwnd, 0, 0, 0, 0, 0, SWP_FLAGS);
            }
            need_reapply_region = true;
        }

        let mut r = WinRect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        if unsafe { GetWindowRect(hwnd, &mut r) } != 0 {
            let w = r.right - r.left;
            let h = r.bottom - r.top;
            if w > 0 && h > 0 {
                let size_changed = REGIONS.with(|regs| {
                    let mut m = regs.borrow_mut();
                    if m.get(&hwnd).copied() != Some((w, h)) {
                        m.insert(hwnd, (w, h));
                        true
                    } else {
                        false
                    }
                });

                // The anchor's empty region is reapplied EVERY pass: a
                // winit `SWP_FRAMECHANGED` (DPI change, new child window,
                // focus) resets the region without touching (w,h) —
                // checking size alone would leave the ring restored.
                // Free on a 1×1 window.
                if need_reapply_region || size_changed || role == Role::Anchor {
                    let rgn = match role {
                        Role::Countdown => {
                            // Clip the top 2 pixels of empty transparent margin to eliminate
                            // the DWM top accent line.
                            unsafe { CreateRectRgn(0, 2, w + 1, h + 1) }
                        }
                        Role::Pin => {
                            // Shape the window to the rounded-corner bounds of the pin
                            // image; the top 2px clip removes the DWM accent line and
                            // eats into the window's TOP_MARGIN transparent rows, so
                            // no image pixel is cropped.
                            unsafe { CreateRoundRectRgn(0, 2, w + 1, h + 1, 16, 16) }
                        }
                        Role::Anchor => {
                            // Empty region: Windows 11 draws a rounded DWM
                            // border + top accent line on borderless windows,
                            // which on this degenerate 1×1 window shows as a
                            // small ring floating on the desktop. The anchor
                            // paints nothing — clip everything away. The size
                            // guard keeps a recycled hwnd (Windows does reuse
                            // them) from ever being blanked.
                            if w <= 8 && h <= 8 {
                                unsafe { CreateRectRgn(0, 0, 0, 0) }
                            } else {
                                0
                            }
                        }
                        // Overlay & pick cover their monitor edge to
                        // edge: no clip region (a clip would eat real
                        // rows or open click-through holes).
                        Role::Overlay | Role::Pick => 0,
                    };
                    if rgn != 0 {
                        unsafe {
                            SetWindowRgn(hwnd, rgn, 1);
                        }
                    }
                }
            }
        }

        // Chrome-less viewports are created hidden (`with_visible(false)`):
        // egui's `Visible(true)` would go through winit's `apply_diff`,
        // which re-applies WS_CAPTION via `to_window_styles` right after
        // ShowWindow — a title-bar flash at reveal. Showing the window
        // here, once stripped, bypasses winit's flag machinery entirely.
        // The overlay is excluded: it must be placed (SetMonitor) before
        // being shown and reveals itself through egui — its borderless-
        // fullscreen flag makes that path caption-free.
        if role != Role::Overlay && unsafe { IsWindowVisible(hwnd) } == 0 {
            let cmd = match role {
                Role::Countdown | Role::Anchor => SW_SHOWNOACTIVATE, // .with_active(false)
                _ => SW_SHOW,
            };
            unsafe {
                ShowWindow(hwnd, cmd);
            }
        }

        1
    }

    unsafe {
        EnumThreadWindows(GetCurrentThreadId(), visit, 0);
    }
}

/// Reveals `path` selected in Explorer (`explorer /select,<path>`).
pub fn reveal_in_file_manager(path: &std::path::Path) {
    let _ = std::process::Command::new("explorer")
        .arg(format!("/select,{}", path.display()))
        .spawn();
}

/// Opens `dir` in Explorer — the Settings "open folder" buttons.
pub fn open_folder(dir: &std::path::Path) {
    // `.spawn()`, never `.status()`: explorer.exe detaches its window and
    // often lingers, so waiting on it blocks the UI thread.
    let _ = std::process::Command::new("explorer").arg(dir).spawn();
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
        .and_then(|exe| exe.parent().map(crate::paths::strip_verbatim))
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
    use std::os::windows::process::CommandExt;
    // cmd.exe is console-subsystem: spawned from this GUI-subsystem exe
    // without CREATE_NO_WINDOW, Windows allocates it a console window —
    // a black flash at every launch.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let out = std::process::Command::new("cmd")
        .args(["/c", "ver"])
        .creation_flags(CREATE_NO_WINDOW)
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
        // HDC is a HANDLE — `*mut c_void` like the cursor-snapshot block
        // (one signature for one symbol: clashing_extern_declarations).
        fn DeleteDC(hdc: *mut c_void) -> i32;
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
        // `pd.h_dc` is a real HDC; isize and `*mut c_void` are the same
        // 64-bit register slot on Win64.
        DeleteDC(hdc as *mut c_void);

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

/// Whether the left mouse button is currently held — `GetAsyncKeyState`
/// reads the physical state, not window-delivered events, so a press the
/// OS edge zones eat (e.g. the taskbar's own hit band) still registers.
/// The overlay polls this for drags started at the screen edge.
pub fn left_mouse_down() -> bool {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetAsyncKeyState(vkey: i32) -> i16;
    }
    const VK_LBUTTON: i32 = 0x01;
    unsafe { GetAsyncKeyState(VK_LBUTTON) & 0x8000u16 as i16 != 0 }
}

/// One short tick — a soft click synthesized once in memory and played
/// through `PlaySoundW` (`SND_MEMORY` keeps it file-free and independent
/// of the user's sound scheme). `MessageBeep` falls back if playback
/// fails.
pub fn tick() {
    use std::sync::OnceLock;

    const SND_ASYNC: u32 = 0x0001;
    const SND_NODEFAULT: u32 = 0x0002;
    const SND_MEMORY: u32 = 0x0004;
    const MB_OK: u32 = 0x0;

    #[link(name = "winmm")]
    unsafe extern "system" {
        fn PlaySoundW(sound: *const u8, hmod: isize, flags: u32) -> i32;
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn MessageBeep(u_type: u32) -> i32;
    }

    static WAV: OnceLock<Vec<u8>> = OnceLock::new();
    let wav = WAV.get_or_init(tick_wav);
    if unsafe { PlaySoundW(wav.as_ptr(), 0, SND_MEMORY | SND_ASYNC | SND_NODEFAULT) } == 0 {
        unsafe { MessageBeep(MB_OK) };
    }
}

/// Builds the tick sample: a 2.2 kHz sine burst under an exponential
/// decay (~250 ms) — a soft metronome-like click, 22.05 kHz 16-bit mono
/// PCM in a standard RIFF/WAVE wrapper. NOTE: anything under ~100 ms is
/// swallowed silently by RDP audio redirection on some setups, hence the
/// generous decay tail.
fn tick_wav() -> Vec<u8> {
    const RATE: u32 = 22_050;
    const MS: usize = 300;
    let frames = RATE as usize * MS / 1000;
    let data_bytes = (frames * 2) as u32;
    let mut wav = Vec::with_capacity(44 + data_bytes as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_bytes).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
    wav.extend_from_slice(&RATE.to_le_bytes());
    wav.extend_from_slice(&(RATE * 2).to_le_bytes()); // byte rate
    wav.extend_from_slice(&2u16.to_le_bytes()); // block align
    wav.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_bytes.to_le_bytes());
    for i in 0..frames {
        let t = i as f32 / RATE as f32;
        let env = (-25.0 * t).exp();
        let s = (t * 2.0 * std::f32::consts::PI * 2200.0).sin() * env * 0.8;
        wav.extend_from_slice(&((s * i16::MAX as f32) as i16).to_le_bytes());
    }
    wav
}

/// Captures the current OS mouse cursor via `GetCursorInfo` +
/// `GetIconInfo` + `DrawIconEx` into a 32-bit DIB section. Returns `None`
/// when the cursor is hidden or cannot be queried.
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
                if let Some(den) = core::num::NonZeroU32::new(a) {
                    dst[0] = ((b[2] as u32 * 255 + a / 2) / den).min(255) as u8;
                    dst[1] = ((b[1] as u32 * 255 + a / 2) / den).min(255) as u8;
                    dst[2] = ((b[0] as u32 * 255 + a / 2) / den).min(255) as u8;
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

/// Full frame of `monitor`'s display as RGBA pixels — xcap's own
/// capture path. macOS bypasses it for a framebuffer read instead
/// (see the macOS implementation).
pub fn capture_frame(monitor: &xcap::Monitor) -> Result<image::RgbaImage, String> {
    monitor.capture_image().map_err(|e| e.to_string())
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
/// `HWND_TOPMOST`; `strip_undecorated_frame` re-asserts it above the
/// taskbar on creation).
pub fn elevate_overlay() {}

/// No-op on Windows — a topmost borderless window already covers the
/// taskbar, there is no higher window band to chase (unlike the macOS
/// Dock/menu-bar window levels).
pub fn elevate_pick_overlay() {}

/// No-op on Windows — there is no system chrome to hide during a
/// capture: the taskbar is simply covered by the topmost overlay (its
/// frozen pixels stay selectable underneath).
pub fn capture_chrome_enter() {}

/// No-op on Windows (see [`capture_chrome_enter`]).
pub fn capture_chrome_exit() {}

/// The monitor's PNP DeviceID (`EnumDisplayDevicesW` with
/// `EDD_GET_DEVICE_INTERFACE_NAME`) — stable per physical monitor on the
/// same port; it may change when the cable moves ports, which the
/// geometry-key fallback covers poorly (documented limitation). `None`
/// when no entry matches the xcap device name (`\\.\DISPLAYn`).
pub(crate) fn stable_monitor_id(monitor: &crate::capture::MonitorInfo) -> Option<String> {
    const EDD_GET_DEVICE_INTERFACE_NAME: u32 = 0x0000_0001;

    // DISPLAY_DEVICEW — cb, DeviceName[32], DeviceString[128],
    // StateFlags, DeviceID[128], DeviceKey[128].
    #[repr(C)]
    struct DisplayDevice {
        cb: u32,
        device_name: [u16; 32],
        device_string: [u16; 128],
        state_flags: u32,
        device_id: [u16; 128],
        device_key: [u16; 128],
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn EnumDisplayDevicesW(
            dev: *const u16,
            num: u32,
            dd: *mut DisplayDevice,
            flags: u32,
        ) -> i32;
    }

    fn blank() -> DisplayDevice {
        DisplayDevice {
            cb: std::mem::size_of::<DisplayDevice>() as u32,
            device_name: [0; 32],
            device_string: [0; 128],
            state_flags: 0,
            device_id: [0; 128],
            device_key: [0; 128],
        }
    }
    fn utf16_trim(buf: &[u16]) -> String {
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..end])
    }

    for i in 0.. {
        let mut dd = blank();
        if unsafe { EnumDisplayDevicesW(std::ptr::null(), i, &mut dd, 0) } == 0 {
            break;
        }
        // xcap's monitor name is the GDI device name ("\\.\DISPLAY1")
        // read from MONITORINFOEXW.szDevice — the adapter to drill into.
        if utf16_trim(&dd.device_name) != monitor.name {
            continue;
        }
        for j in 0.. {
            let mut md = blank();
            if unsafe {
                EnumDisplayDevicesW(
                    dd.device_name.as_ptr(),
                    j,
                    &mut md,
                    EDD_GET_DEVICE_INTERFACE_NAME,
                )
            } == 0
            {
                break;
            }
            let id = utf16_trim(&md.device_id);
            if !id.is_empty() {
                return Some(id);
            }
        }
        break;
    }
    None
}
/// Re-grabs the capture overlay's focus while `Capturing` — Alt+Tab (or
/// any OS mechanism) can steal it, then Esc would hit another app.
/// Best-effort: Windows may refuse the raise (`SetForegroundWindow`
/// rules), but clicks anywhere still reach the fullscreen overlay.
pub fn refocus_capture_overlay(ctx: &egui::Context, id: egui::ViewportId) {
    ctx.send_viewport_cmd_to(id, egui::ViewportCommand::Focus);
}

/// Classifies `info` for the status-strip icon via
/// `QueryDisplayConfig`/`DisplayConfigGetDeviceInfo`: the path target's
/// `outputTechnology` distinguishes internal laptop panels
/// (INTERNAL / LVDS / embedded-DisplayPort), indirect/virtual drivers
/// (Spacedesk, Miracast receivers...) and real video transports. A path
/// that can't be resolved yields `Unknown` — never a guessed type.
pub(crate) fn monitor_kind(info: &crate::capture::MonitorInfo) -> crate::capture::MonitorKind {
    use crate::capture::MonitorKind;

    const QDC_ONLY_ACTIVE_PATHS: u32 = 0x0000_0002;
    const DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME: u32 = 1;
    // DISPLAYCONFIG_VIDEO_OUTPUT_TECHNOLOGY values (wingdi.h).
    const TECH_LVDS: u32 = 6;
    const TECH_DISPLAYPORT_EMBEDDED: u32 = 11;
    const TECH_MIRACAST: u32 = 15;
    const TECH_INDIRECT_WIRED: u32 = 16;
    const TECH_INDIRECT_WIRELESS: u32 = 17;
    const TECH_INDIRECT_VIRTUAL: u32 = 18;
    const TECH_INTERNAL: u32 = 0x8000_0000;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct DisplayConfigPathSourceInfo {
        adapter_id: i64,
        id: u32,
        mode_info_idx: u32,
        status_flags: u32,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct DisplayConfigPathTargetInfo {
        adapter_id: i64,
        id: u32,
        mode_info_idx: u32,
        output_technology: u32,
        rotation: u32,
        scaling: u32,
        refresh_num: u32,
        refresh_den: u32,
        scan_line_ordering: u32,
        target_available: i32,
        status_flags: u32,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct DisplayConfigPathInfo {
        source_info: DisplayConfigPathSourceInfo,
        target_info: DisplayConfigPathTargetInfo,
        flags: u32,
    }
    #[repr(C)]
    struct DisplayConfigDeviceInfoHeader {
        ty: u32,
        size: u32,
        adapter_id: i64,
        id: u32,
    }
    #[repr(C)]
    struct DisplayConfigSourceDeviceName {
        header: DisplayConfigDeviceInfoHeader,
        view_gdi_device_name: [u16; 32],
    }
    // DISPLAYCONFIG_MODE_INFO: QueryDisplayConfig fills one 64-byte
    // struct per mode — header (infoType, id, adapterId LUID) plus the
    // 48-byte target/source/desktop-image union. The modes are unused
    // here but the buffer must still be real: sizing it in `c_void`
    // elements overflows the allocation eightfold.
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct DisplayConfigModeInfo {
        info_type: u32,
        id: u32,
        adapter_id: i64,
        payload: [u64; 6],
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetDisplayConfigBufferSizes(flags: u32, n_path: *mut u32, n_mode: *mut u32) -> i32;
        fn QueryDisplayConfig(
            flags: u32,
            n_path: *mut u32,
            paths: *mut DisplayConfigPathInfo,
            n_mode: *mut u32,
            modes: *mut c_void,
            topology: *mut c_void,
        ) -> i32;
        fn DisplayConfigGetDeviceInfo(packet: *mut DisplayConfigDeviceInfoHeader) -> i32;
    }

    use std::ffi::c_void;

    let mut n_path = 0u32;
    let mut n_mode = 0u32;
    if unsafe { GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut n_path, &mut n_mode) } != 0
    {
        return MonitorKind::Unknown;
    }
    let mut paths = vec![
        DisplayConfigPathInfo {
            source_info: DisplayConfigPathSourceInfo {
                adapter_id: 0,
                id: 0,
                mode_info_idx: 0,
                status_flags: 0,
            },
            target_info: DisplayConfigPathTargetInfo {
                adapter_id: 0,
                id: 0,
                mode_info_idx: 0,
                output_technology: 0,
                rotation: 0,
                scaling: 0,
                refresh_num: 0,
                refresh_den: 0,
                scan_line_ordering: 0,
                target_available: 0,
                status_flags: 0,
            },
            flags: 0,
        };
        n_path as usize
    ];
    let mut modes = vec![
        DisplayConfigModeInfo {
            info_type: 0,
            id: 0,
            adapter_id: 0,
            payload: [0; 6],
        };
        n_mode as usize
    ];
    if unsafe {
        QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut n_path,
            paths.as_mut_ptr(),
            &mut n_mode,
            modes.as_mut_ptr() as *mut c_void,
            std::ptr::null_mut(),
        )
    } != 0
    {
        return MonitorKind::Unknown;
    }
    paths.truncate(n_path as usize);

    for path in &paths {
        let mut src = DisplayConfigSourceDeviceName {
            header: DisplayConfigDeviceInfoHeader {
                ty: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                size: std::mem::size_of::<DisplayConfigSourceDeviceName>() as u32,
                adapter_id: path.source_info.adapter_id,
                id: path.source_info.id,
            },
            view_gdi_device_name: [0u16; 32],
        };
        if unsafe { DisplayConfigGetDeviceInfo(&mut src.header) } != 0 {
            continue;
        }
        let end = src
            .view_gdi_device_name
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(32);
        if String::from_utf16_lossy(&src.view_gdi_device_name[..end]) != info.name {
            continue;
        }
        return match path.target_info.output_technology {
            TECH_INTERNAL | TECH_LVDS | TECH_DISPLAYPORT_EMBEDDED => MonitorKind::BuiltIn,
            TECH_INDIRECT_WIRED
            | TECH_INDIRECT_WIRELESS
            | TECH_INDIRECT_VIRTUAL
            | TECH_MIRACAST => MonitorKind::Virtual,
            // Real video transports: VGA/S-Video/Composite/Component/DVI/
            // HDMI/D-JPN/SDI/DisplayPort/UDI/SDTV dongle/DP USB tunnel.
            0..=5 | 8..=10 | 12..=14 | 19 => MonitorKind::External,
            _ => MonitorKind::Unknown,
        };
    }
    MonitorKind::Unknown
}

/// The number of connected displays — polled to detect monitor
/// hot-(un)plug so the viewer can re-anchor off a vanished screen.
pub fn display_count() -> u32 {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetSystemMetrics(index: i32) -> i32;
    }
    // SM_CMONITORS = 80.
    unsafe { GetSystemMetrics(80) as u32 }
}

// ---------------------------------------------------------------------
// Second-instance wake channel: a named auto-reset event.
// ---------------------------------------------------------------------
//
// A second `pixme.exe` launch (desktop-shortcut double-click) surfaces
// the RUNNING instance's viewer instead of showing the "already
// running" dialog. The channel is `Local\PixMeWake-<hash>` — `Local\`
// scopes the object to the user session and the kernel destroys it
// with the last handle, so a crash can never leave a stale object.
// Chosen over a socket or named pipe: no port, no filesystem artifact,
// no signaler thread, and the listener's `WaitForSingleObject` is a
// parked wait — zero wakeups while nothing happens.

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateEventW(
        attrs: *const std::ffi::c_void,
        manual: i32,
        initial: i32,
        name: *const u16,
    ) -> isize;
    fn OpenEventW(access: u32, inherit: i32, name: *const u16) -> isize;
    fn SetEvent(event: isize) -> i32;
    fn CloseHandle(handle: isize) -> i32;
    fn WaitForSingleObject(handle: isize, ms: u32) -> u32;
    fn GetLastError() -> u32;
}

#[link(name = "user32")]
unsafe extern "system" {
    fn AllowSetForegroundWindow(pid: u32) -> i32;
}

/// `SetEvent` needs no more than `EVENT_MODIFY_STATE`.
const EVENT_MODIFY_STATE: u32 = 0x0002;
/// `WaitForSingleObject`'s signaled outcome.
const WAIT_OBJECT_0: u32 = 0;
/// `AllowSetForegroundWindow(ASFW_ANY)` hands the foreground right to
/// EVERY process — the only grant usable when the resident's pid is
/// unknown to the relaunched process.
const ASFW_ANY: u32 = 0xFFFF_FFFF;
/// `WaitForSingleObject(-1)` — parked until the event fires.
const INFINITE: u32 = 0xFFFF_FFFF;

/// The event name for `key` (the same per-installation key the
/// single-instance mutex uses). FNV-1a keeps the name far under the
/// 260-char object-name limit whatever the install path's length, and
/// contains no backslash of its own.
fn wakeup_event_name(key: &str) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for &b in key.as_bytes() {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!(r"Local\PixMeWake-{hash:016x}")
}

fn wide(name: &str) -> Vec<u16> {
    name.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Keeps the wake listener alive: dropping stops the waiter thread and
/// closes the event. The app holds it for the whole run; tests drop it
/// to join the thread instead of leaking a parked wait.
pub struct WakeupServer {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    event: isize,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for WakeupServer {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        // Break the parked wait so the thread sees `stop` and exits.
        unsafe { SetEvent(self.event) };
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        unsafe { CloseHandle(self.event) };
    }
}

/// Resident side: creates the wake event and parks a thread on it —
/// each signal runs `on_wake` (which queues the surface command like a
/// tray click). `None` when the event cannot be created: `main`'s
/// translated dialog stays the fallback for a relaunch.
pub fn serve_instance_wakeup<F>(key: &str, on_wake: F) -> Option<WakeupServer>
where
    F: Fn() + Send + 'static,
{
    let name = wide(&wakeup_event_name(key));
    // Auto-reset (FALSE manual), initially non-signaled, default
    // security — `Local\` already scopes the name to this session.
    let event = unsafe { CreateEventW(std::ptr::null(), 0, 0, name.as_ptr()) };
    if event == 0 {
        tracing::warn!("wake event unavailable: {}", unsafe { GetLastError() });
        return None;
    }
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let thread = {
        let stop = std::sync::Arc::clone(&stop);
        std::thread::spawn(move || {
            loop {
                match unsafe { WaitForSingleObject(event, INFINITE) } {
                    WAIT_OBJECT_0 if !stop.load(std::sync::atomic::Ordering::Relaxed) => on_wake(),
                    // Stopped (`Drop` signaled) or the handle went bad —
                    // either way the listener ends here.
                    _ => break,
                }
            }
        })
    };
    Some(WakeupServer {
        stop,
        event,
        thread: Some(thread),
    })
}

/// Signaler side, called by the SECOND process once the instance guard
/// reports the lock taken: grants the resident this process's
/// foreground rights, then sets its wake event. `false` = no listener —
/// the caller falls back to the translated `app.already_running`
/// dialog.
pub fn signal_instance_wakeup(key: &str) -> bool {
    // The resident holds the guard from `main` but only installs the
    // listener once its event loop is up — a quick relaunch can land in
    // that gap, so retry briefly before conceding to the dialog.
    const ATTEMPTS: u32 = 60;
    const PAUSE: std::time::Duration = std::time::Duration::from_millis(50);
    let name = wide(&wakeup_event_name(key));
    for _ in 0..ATTEMPTS {
        let event = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr()) };
        if event != 0 {
            // The user launched THIS process — we hold the foreground
            // right SetForegroundWindow checks. ASFW_ANY hands it to
            // every process so the resident can raise its viewer even
            // from behind other apps.
            unsafe { AllowSetForegroundWindow(ASFW_ANY) };
            let set = unsafe { SetEvent(event) } != 0;
            unsafe { CloseHandle(event) };
            return set;
        }
        std::thread::sleep(PAUSE);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A key built from a TEMPORARY directory — never `paths`' real
    /// `instance_key()`, so a test can never reach the live PixMe's
    /// event even when a session is running.
    fn temp_key(tag: &str) -> String {
        std::env::temp_dir()
            .join(format!("pixme-wake-{tag}-{}", std::process::id()))
            .join("instance.lock")
            .to_string_lossy()
            .replace('\\', "/")
    }

    /// The resident's listener fires on each signal; two different
    /// installations (two keys) never see each other's event.
    #[test]
    fn wake_event_fires_the_resident_listener() {
        let key = temp_key("live");
        let fired = Arc::new(AtomicUsize::new(0));
        let server = {
            let fired = Arc::clone(&fired);
            serve_instance_wakeup(&key, move || {
                fired.fetch_add(1, Ordering::Relaxed);
            })
        }
        .expect("event creation must succeed in tests");

        assert!(signal_instance_wakeup(&key));
        for _ in 0..80 {
            if fired.load(Ordering::Relaxed) > 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        assert_eq!(fired.load(Ordering::Relaxed), 1);
        // A different install path = a different event: never signaled.
        assert_ne!(
            wakeup_event_name(&key),
            wakeup_event_name(&temp_key("other"))
        );
        drop(server);
    }

    /// Name derivation is stable, session-scoped and path-derived —
    /// the properties the per-installation contract needs.
    #[test]
    fn event_name_is_session_scoped_and_per_key() {
        let a = wakeup_event_name(&temp_key("a"));
        assert!(a.starts_with(r"Local\PixMeWake-"));
        assert_eq!(a, wakeup_event_name(&temp_key("a")));
        assert_ne!(a, wakeup_event_name(&temp_key("b")));
        assert!(a.len() < 200, "name must stay under the object limit: {a}");
    }
}
