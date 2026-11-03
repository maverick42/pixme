//! OS-specific services: one implementation per OS, selected at compile time.
//! All `#[cfg(target_os)]` code lives here — nowhere else in the crate.
//!
//! Both OSes present a resident icon whose menu carries "Open PixMe" (and
//! "Quit" on Windows): the macOS menu-bar status item and the Windows
//! notification-area tray icon. Menu clicks on either OS are pushed
//! into [`PENDING`] and drained by [`crate::app::Shared::pump`] on the next UI
//! pass. macOS additionally installs the native application menus.

pub mod drag_out;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

use std::sync::{Mutex, MutexGuard};

use crate::app::Command;
use eframe::egui;

/// What the OS needs PixMe to handle.
pub trait PlatformServices: Send + Sync {
    /// Ensures the OS permission required for screen capture is granted
    /// (macOS Screen Recording; no-op on Windows). `Err` carries a
    /// remediation hint key for i18n.
    fn ensure_capture_permission(&self) -> Result<(), crate::capture::CaptureError>;
    /// Requests the OS to open its settings page for the missing permission,
    /// when the OS supports that (macOS does).
    fn open_permission_settings(&self) {}

    /// The current state of the OS capture permission for the startup
    /// banner — `Some(granted)` on macOS (Screen Recording preflight),
    /// `None` on Windows (no such permission).
    fn capture_permission_status(&self) -> Option<bool> {
        None
    }

    /// Whether the OS provides a native application menu bar (macOS: yes —
    /// menus live at the top of the screen; Windows: no — the viewer renders
    /// its own egui menu strip inside the window instead).
    fn has_native_app_menu(&self) -> bool;
}

/// The implementation for the running OS.
#[cfg(target_os = "macos")]
pub fn current() -> &'static dyn PlatformServices {
    &macos::MacOs
}

/// The implementation for the running OS.
#[cfg(target_os = "windows")]
pub fn current() -> &'static dyn PlatformServices {
    &windows::Windows
}

/// Installs the OS-level UI (status/tray icon, menus, event handler).
/// Owns its handles internally (main thread only, for the app lifetime).
#[cfg(target_os = "macos")]
pub use macos::install_ui;
/// Installs the OS-level UI (see above).
#[cfg(target_os = "windows")]
pub use windows::install_ui;

/// Rewrites every OS menu label in the current language.
#[cfg(target_os = "macos")]
pub use macos::relabel_menus;
/// Rewrites every OS menu label (see above).
#[cfg(target_os = "windows")]
pub use windows::relabel_menus;

/// OS tweaks to `eframe::NativeOptions` before the event loop starts
/// (macOS: accessory activation policy so no Dock icon appears).
#[cfg(target_os = "macos")]
pub use macos::apply_native_options;
/// OS tweaks to `eframe::NativeOptions` (Windows: none).
#[cfg(target_os = "windows")]
pub use windows::apply_native_options;

/// One on-screen window as reported by the OS window list — the fields
/// the pick overlay needs, gathered in a single enumeration pass.
/// `xcap::Window` accessor methods each re-enumerate the whole list on
/// macOS, so asking it for ~10 fields × ~50 windows stalls the UI for
/// seconds; this record comes from one `CGWindowListCopyWindowInfo` call.
pub struct WindowRecord {
    /// OS window id (`kCGWindowNumber` on macOS, `HWND` on Windows).
    pub id: u32,
    /// Owning process id.
    pub pid: u32,
    /// Owning application name.
    pub app_name: String,
    /// Window title.
    pub title: String,
    /// Global logical bounds (same space as `MonitorInfo::rect`).
    pub rect: crate::capture::PhysicalRect,
    /// Window-server layer: 0 = normal, 3 = floating palette, 8 = modal;
    /// Dock/menubar/status items sit at 20+. Always 0 on Windows.
    pub layer: i32,
}

/// All on-screen windows, front-to-back, in one enumeration pass.
#[cfg(target_os = "macos")]
pub use macos::window_list;
/// All on-screen windows (see above).
#[cfg(target_os = "windows")]
pub use windows::window_list;

/// A connected monitor plus its OS-stable identifier — the key used by
/// the persisted `monitors` prefs map (custom name and, later, saved
/// geometry). The stable id survives disconnect/reconnect cycles
/// (CGDisplay UUID on macOS, monitor PNP DeviceID on Windows); when the
/// OS reports none, a geometry key stands in.
#[derive(Clone, Debug)]
pub struct MonitorIdentity {
    /// Stable key into `Settings::monitors`.
    pub stable_id: String,
    /// The live monitor record (xcap enumeration).
    pub info: crate::capture::MonitorInfo,
}

/// The OS-stable id for `info`, or `None` — the caller falls back to a
/// geometry key.
#[cfg(target_os = "macos")]
pub(crate) use macos::stable_monitor_id;
/// The OS-stable id for `info` (see above).
#[cfg(target_os = "windows")]
pub(crate) use windows::stable_monitor_id;

/// The connected monitors with their stable identities, for the status
/// strip's monitor row. Enumeration failures yield an empty list (the
/// strip simply shows no monitor buttons).
pub fn monitor_identities() -> Vec<MonitorIdentity> {
    crate::capture::screen::monitors()
        .unwrap_or_default()
        .into_iter()
        .map(|info| {
            let stable_id = stable_monitor_id(&info).unwrap_or_else(|| {
                format!(
                    "rect:{},{},{}x{}",
                    info.rect.x, info.rect.y, info.rect.width, info.rect.height
                )
            });
            MonitorIdentity { stable_id, info }
        })
        .collect()
}

/// Lock helper: recover a poisoned mutex rather than crash a menu callback.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Commands queued by OS menu clicks (status item / tray / app menus).
static PENDING: Mutex<Vec<Command>> = Mutex::new(Vec::new());

/// Called by OS menu-event handlers on the main thread.
pub(crate) fn queue_menu_command(command: Command) {
    lock(&PENDING).push(command);
}

/// Commands queued since the last call, drained by `Shared::pump`.
pub(crate) fn take_pending_commands() -> Vec<Command> {
    std::mem::take(&mut *lock(&PENDING))
}

/// Maps a menu-item id to an app [`Command`] — shared by the macOS status
/// item + app menus and the Windows tray menu.
pub(crate) fn command_from_menu_id(id: &str) -> Option<Command> {
    Some(match id {
        "open_viewer" => Command::OpenViewer,
        "settings" => Command::OpenSettings,
        "import_clipboard" => Command::ImportClipboard,
        "edit.undo" => Command::UndoActive,
        "edit.redo" => Command::RedoActive,
        "edit.copy" => Command::CopyActive,
        "file.open" => Command::OpenFile,
        "file.save" => Command::SaveActive,
        "file.save_all" => Command::SaveAll,
        "file.rename" => Command::RenameActive,
        "file.delete" => Command::DeleteFileActive,
        "file.open_last_dir" => Command::OpenLastDir,
        "file.print" => Command::PrintActive,
        "quit" => Command::Quit,
        id if id.starts_with("recent:") => {
            Command::OpenPath(std::path::PathBuf::from(&id["recent:".len()..]))
        }
        _ => return None,
    })
}

/// Greys out menu items whose [`Command`]'s
/// [`crate::app::is_command_enabled`] ruling forbids them under `modal`.
/// Called from [`crate::app::Shared::pump`], i.e. always on the main
/// thread, where the thread_local muda handles live.
#[cfg(target_os = "macos")]
pub use macos::sync_menu_enabled;
/// Greys out menu items (see above).
#[cfg(target_os = "windows")]
pub use windows::sync_menu_enabled;

/// Rebuilds the File ▸ Recents submenu when the list changed (see above).
#[cfg(target_os = "macos")]
pub use macos::sync_recent_menu;
/// No-op on Windows: its menus are egui-drawn every frame.
#[cfg(target_os = "windows")]
#[inline]
pub fn sync_recent_menu(_recents: &[std::path::PathBuf]) {}

/// Strips `WS_CAPTION` from undecorated floating windows to prevent
/// the compositor offset (PhotoCraft #2246 fix; no-op on macOS).
#[cfg(target_os = "windows")]
pub use windows::strip_undecorated_frame;
/// No-op on macOS (see above).
#[cfg(target_os = "macos")]
#[inline]
pub fn strip_undecorated_frame() {}

/// Reveals a file selected in the OS file manager after saving.
#[cfg(target_os = "macos")]
pub use macos::reveal_in_file_manager;
/// Reveals a file selected in the OS file manager (see above).
#[cfg(target_os = "windows")]
pub use windows::reveal_in_file_manager;

/// The OS choice of base directories for app data and logs
/// (`crate::paths::BaseDirs`) — exe-adjacent + LOCALAPPDATA fallback on
/// Windows, Application Support + ~/Library/Logs on macOS.
#[cfg(target_os = "macos")]
pub use macos::resolve_base_dirs;
/// The OS choice of base directories (see above).
#[cfg(target_os = "windows")]
pub use windows::resolve_base_dirs;

/// The OS version string for the startup banner / diagnostics
/// ("macOS 15.x", "Windows 10.0.x").
#[cfg(target_os = "macos")]
pub use macos::os_version;
/// The OS version string (see above).
#[cfg(target_os = "windows")]
pub use windows::os_version;

/// Opens `dir` in the OS file manager (Finder / Explorer).
#[cfg(target_os = "macos")]
pub use macos::open_folder;
/// Opens `dir` in the OS file manager (see above).
#[cfg(target_os = "windows")]
pub use windows::open_folder;

/// The display unit [`crate::capture::MonitorInfo::rect`] is expressed in:
/// **physical pixels on Windows** (xcap reads `DEVMODEW`), **points on
/// macOS** (`CGDisplayBounds`). winit's `available_monitors()` reports
/// physical pixels on both OSes, so matching an xcap monitor to a winit
/// index goes through this unit conversion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeSpace {
    /// Raw device pixels — `scale_factor` is only informational.
    Physical,
    /// Logical points — multiply by `scale_factor` for physical pixels.
    Points,
}

/// The [`NativeSpace`] of `MonitorInfo::rect` on this OS.
#[cfg(target_os = "windows")]
pub const NATIVE_SPACE: NativeSpace = NativeSpace::Physical;
/// The [`NativeSpace`] of `MonitorInfo::rect` on this OS.
#[cfg(not(target_os = "windows"))]
pub const NATIVE_SPACE: NativeSpace = NativeSpace::Points;

/// `MonitorInfo::rect` expressed in **physical pixels** — the space
/// winit's `available_monitors()` reports on both OSes (Windows gives it
/// directly; macOS points are multiplied by `scale_factor` and rounded).
pub fn monitor_physical_rect(
    rect: &crate::capture::PhysicalRect,
    scale_factor: f32,
    space: NativeSpace,
) -> (i32, i32, u32, u32) {
    match space {
        NativeSpace::Physical => (rect.x, rect.y, rect.width, rect.height),
        NativeSpace::Points => {
            let s = f64::from(scale_factor.max(0.0));
            (
                (f64::from(rect.x) * s).round() as i32,
                (f64::from(rect.y) * s).round() as i32,
                (f64::from(rect.width) * s).round().max(1.0) as u32,
                (f64::from(rect.height) * s).round().max(1.0) as u32,
            )
        }
    }
}

/// The index (winit `available_monitors()` order) of the monitor matching
/// `monitor`'s physical bounds, within `tol` px — xcap and winit do not
/// enumerate in the same order, and PhotoCraft's picker matches the same
/// way. `placements` are `(x, y, width, height)` in physical pixels.
pub fn winit_monitor_index(
    monitor: &crate::capture::MonitorInfo,
    placements: &[(i32, i32, u32, u32)],
    tol: i32,
) -> Option<usize> {
    let (x, y, w, h) = monitor_physical_rect(&monitor.rect, monitor.scale_factor, NATIVE_SPACE);
    let (w, h) = (w as i64, h as i64);
    placements.iter().position(|&(px, py, pw, ph)| {
        (px - x).abs() <= tol
            && (py - y).abs() <= tol
            && (i64::from(pw) - w).abs() <= i64::from(tol)
            && (i64::from(ph) - h).abs() <= i64::from(tol)
    })
}

/// `monitor`'s rect expressed in the egui "ui points" of a window whose
/// `native_pixels_per_point` is `window_ppp` — the space
/// `ViewportCommand::OuterPosition` and reported `outer_rect`/`inner_rect`
/// speak for THAT window. Always convert with the receiving window's own
/// scale, never the monitor's: egui-winit multiplies command values back
/// by the window's ppp, so this yields exact physical targets even when
/// the window is still on another display (mixed-DPI).
pub fn monitor_ui_rect(monitor: &crate::capture::MonitorInfo, window_ppp: f32) -> egui::Rect {
    let min = native_to_ui(
        egui::pos2(monitor.rect.x as f32, monitor.rect.y as f32),
        window_ppp,
    );
    let size = native_to_ui(
        egui::pos2(monitor.rect.width as f32, monitor.rect.height as f32),
        window_ppp,
    )
    .to_vec2();
    egui::Rect::from_min_size(min, size)
}

/// The window's own `native_pixels_per_point`, or `fallback` while egui
/// has not reported one yet (first frames after creation).
pub fn window_ppp(ctx: &egui::Context, fallback: f32) -> f32 {
    ctx.input(|i| i.viewport().native_pixels_per_point)
        .filter(|p| p.is_finite() && *p > 0.0)
        .unwrap_or(fallback)
}

/// egui window "ui points" (`ViewportInfo::outer_rect`) → OS-native global
/// units, the space `MonitorInfo::rect` lives in (CGDisplay bounds on
/// macOS, virtual-screen pixels on Windows).
///
/// `native_pixels_per_point` is ALWAYS the receiving window's own scale —
/// never the target monitor's: egui-winit multiplies command values back
/// by the window's ppp, so this conversion lands on exact native units
/// whatever monitor the window currently sits on.
///
/// macOS: winit derives window positions from logical points
/// (`LogicalPosition::to_physical`), so egui's division by
/// `native_pixels_per_point` lands back on native points — identity.
/// Windows: winit reports true physical pixels, so egui points must be
/// scaled back up to match xcap's physical-pixel rects.
pub fn ui_to_native(pos: egui::Pos2, native_pixels_per_point: f32) -> egui::Pos2 {
    #[cfg(target_os = "windows")]
    {
        pos * native_pixels_per_point
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = native_pixels_per_point;
        pos
    }
}

/// OS-native global units (`MonitorInfo::rect` space) → egui window
/// "ui points". The inverse of [`ui_to_native`].
pub fn native_to_ui(pos: egui::Pos2, native_pixels_per_point: f32) -> egui::Pos2 {
    #[cfg(target_os = "windows")]
    {
        pos / native_pixels_per_point
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = native_pixels_per_point;
        pos
    }
}

/// Shows the native OS print dialog for `img` (fit to the page),
/// `job_title` naming the job. Modal — main thread only.
#[cfg(target_os = "macos")]
pub use macos::print_image;
/// Shows the native OS print dialog for `img` (see above).
#[cfg(target_os = "windows")]
pub use windows::print_image;

/// Global mouse position in the OS-native display space (the same space
/// `MonitorInfo::rect` lives in).
#[cfg(target_os = "macos")]
pub use macos::cursor_position;
/// Global mouse position in the OS-native display space (see above).
#[cfg(target_os = "windows")]
pub use windows::cursor_position;

/// The short tick sounded at each countdown second (and when a delayed
/// capture is armed) — the OS alert sound.
#[cfg(target_os = "macos")]
pub use macos::tick;
/// The short tick (see above).
#[cfg(target_os = "windows")]
pub use windows::tick;

/// Snapshot of the current OS mouse cursor, ready to be composited onto a
/// captured frame.
#[derive(Clone, Debug)]
pub struct CursorSnapshot {
    /// RGBA bitmap of the cursor at the target monitor's scale factor.
    pub image: image::RgbaImage,
    /// Global cursor position in the OS-native display space (the same space
    /// `MonitorInfo::rect` lives in).
    pub position: (f32, f32),
    /// Hotspot offset in physical pixels from the top-left of `image`.
    pub hotspot_px: (i32, i32),
}

/// Captures the current OS mouse cursor image, its global position and its
/// hotspot scaled to `scale_factor`. Returns `None` if the cursor is hidden
/// or cannot be read from the OS.
#[cfg(target_os = "macos")]
pub use macos::cursor_snapshot;
/// Captures the current OS mouse cursor (see above).
#[cfg(target_os = "windows")]
pub use windows::cursor_snapshot;

/// Hides the OS cursor so a screen capture does not bake it into the frame
/// (the OS composites the cursor on its own). Counter-based — always pair
/// with `unhide_cursor`.
#[cfg(target_os = "macos")]
pub use macos::{
    capture_chrome_enter, capture_chrome_exit, dismiss_active_menus, elevate_overlay, hide_cursor,
    left_mouse_down, unhide_cursor,
};
/// Hides / unhides the OS cursor and dismisses active popup menus around a
/// capture (see above). `capture_chrome_*` arms/disarms the capture kiosk
/// (menu bar + Dock hidden on macOS; no-op on Windows). `left_mouse_down`
/// polls the physical button so edge-swallowed presses still drag.
#[cfg(target_os = "windows")]
pub use windows::{
    capture_chrome_enter, capture_chrome_exit, dismiss_active_menus, elevate_overlay, hide_cursor,
    left_mouse_down, unhide_cursor,
};

/// Re-grabs the capture overlay's focus while `Capturing` — Windows
/// only: Alt+Tab can steal it and Esc would then hit another app. The
/// macOS capture kiosk already locks the session, making this a no-op.
#[cfg(target_os = "macos")]
pub use macos::refocus_capture_overlay;
/// Re-grabs the capture overlay's focus while `Capturing` (see above).
#[cfg(target_os = "windows")]
pub use windows::refocus_capture_overlay;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::{MonitorInfo, PhysicalRect};

    fn monitor(x: i32, y: i32, w: u32, h: u32, scale: f32) -> MonitorInfo {
        MonitorInfo {
            id: 1,
            name: "test".into(),
            rect: PhysicalRect {
                x,
                y,
                width: w,
                height: h,
            },
            scale_factor: scale,
            is_primary: false,
        }
    }

    /// Windows convention: xcap rects are already physical pixels — the
    /// scale factor never enters the geometry.
    #[test]
    fn windows_physical_rect_is_passthrough() {
        let m = monitor(3840, 0, 1920, 1200, 1.0);
        assert_eq!(
            monitor_physical_rect(&m.rect, m.scale_factor, NativeSpace::Physical),
            (3840, 0, 1920, 1200)
        );
        let m = monitor(0, 0, 3840, 2160, 1.5);
        assert_eq!(
            monitor_physical_rect(&m.rect, m.scale_factor, NativeSpace::Physical),
            (0, 0, 3840, 2160),
            "Windows monitor rects are physical pixels regardless of scale"
        );
    }

    /// macOS convention: xcap rects are CGDisplay points — the physical
    /// rect (what winit reports) is points × scale.
    #[test]
    fn macos_points_rect_scales_to_physical() {
        let m = monitor(0, 0, 1728, 1117, 2.0);
        assert_eq!(
            monitor_physical_rect(&m.rect, m.scale_factor, NativeSpace::Points),
            (0, 0, 3456, 2234)
        );
        let m = monitor(1728, -667, 3008, 1692, 2.0);
        assert_eq!(
            monitor_physical_rect(&m.rect, m.scale_factor, NativeSpace::Points),
            (3456, -1334, 6016, 3384)
        );
    }

    /// The user's actual setup: a 4K primary at 150 % next to a
    /// 1920×1200 laptop at 100 % — both Windows-physical.
    #[cfg(target_os = "windows")]
    #[test]
    fn winit_index_matches_mixed_dpi_layout() {
        let placements = [(0, 0, 3840, 2160), (3840, 0, 1920, 1200)];
        let primary = monitor(0, 0, 3840, 2160, 1.5);
        let laptop = monitor(3840, 0, 1920, 1200, 1.0);
        assert_eq!(winit_monitor_index(&primary, &placements, 1), Some(0));
        assert_eq!(winit_monitor_index(&laptop, &placements, 1), Some(1));
        // A monitor winit doesn't know (unplugged since) has no index.
        let gone = monitor(0, 2160, 1920, 1080, 1.0);
        assert_eq!(winit_monitor_index(&gone, &placements, 1), None);
    }

    /// macOS twin of the mixed-DPI layout test: xcap rects are
    /// CGDisplay POINTS, winit placements physical px — matching goes
    /// through the ×scale conversion (BenQ 5K + Sidecar iPad, both 2×).
    #[cfg(target_os = "macos")]
    #[test]
    fn winit_index_matches_point_space_layout() {
        let placements = [(0, 0, 5120, 3414), (-2266, 1638, 2266, 1488)];
        let benq = monitor(0, 0, 2560, 1707, 2.0);
        let sidecar = monitor(-1133, 819, 1133, 744, 2.0);
        assert_eq!(winit_monitor_index(&benq, &placements, 1), Some(0));
        assert_eq!(winit_monitor_index(&sidecar, &placements, 1), Some(1));
        // A monitor winit doesn't know (unplugged since) has no index.
        let gone = monitor(0, 1707, 1920, 1080, 2.0);
        assert_eq!(winit_monitor_index(&gone, &placements, 1), None);
    }

    /// `monitor_ui_rect` must land on the physical target whatever scale
    /// the receiving window currently has: egui-winit multiplies command
    /// values by the window's own ppp.
    #[cfg(target_os = "windows")]
    #[test]
    fn ui_rect_depends_on_window_scale_not_monitor_scale() {
        // Windows: laptop monitor (physical 3840,0..5760,1200) seen by a
        // window still on the 150 % 4K (ppp 1.5).
        let laptop = monitor(3840, 0, 1920, 1200, 1.0);
        let r = monitor_ui_rect(&laptop, 1.5);
        assert_eq!(r.min, egui::pos2(2560.0, 0.0));
        assert_eq!(r.size(), egui::vec2(1280.0, 800.0));
        // Sending these points, winit computes min × 1.5 = (3840, 0) px —
        // exactly the laptop's physical origin.
        let seen_by_laptop = monitor_ui_rect(&laptop, 1.0);
        assert_eq!(seen_by_laptop.min, egui::pos2(3840.0, 0.0));
    }

    /// macOS path (Points space): identity — egui points ARE the
    /// monitor points, the window's scale never enters the geometry.
    #[cfg(target_os = "macos")]
    #[test]
    fn ui_rect_is_point_space_on_macos() {
        let sidecar = monitor(-1133, 819, 1133, 744, 2.0);
        let r = monitor_ui_rect(&sidecar, 1.5);
        assert_eq!(r.min, egui::pos2(-1133.0, 819.0));
        assert_eq!(r.size(), egui::vec2(1133.0, 744.0));
        // Same result whatever the receiving window's ppp.
        assert_eq!(r, monitor_ui_rect(&sidecar, 1.0));
    }

    #[test]
    fn window_ppp_falls_back_until_reported() {
        let ctx = egui::Context::default();
        assert_eq!(window_ppp(&ctx, 1.5), 1.5);
    }
}
