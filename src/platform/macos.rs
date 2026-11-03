//! macOS implementation: menu-bar status item, native application menus and
//! the accessory (no-Dock-icon) activation policy.
//!
//! The status item exposes the same two actions as the Windows tray icon —
//! "Take a capture" and "Open PixMe". The real application menus (PixMe /
//! File / Edit / Window) live on the menu bar, and quitting is only
//! possible from the PixMe menu. `tray_icon::menu` re-exports `muda`, so one
//! dependency covers both surfaces.
//!
//! Menu/tray handles are `Rc`-backed (not `Send`): they live in a
//! `thread_local` owned by the main thread instead of app state, which is
//! shared with deferred-viewport callbacks.
//!
//! Phase 3: Screen Recording permission via `CGPreflightScreenCaptureAccess`,
//! Retina point/pixel conversion.

use std::cell::RefCell;

use rust_i18n::t;
use tray_icon::menu::accelerator::{Accelerator, Code, Modifiers};
use tray_icon::menu::{IsMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconEvent};

use super::PlatformServices;
use crate::capture::CaptureError;
use crate::ui::icons;

/// macOS services.
pub struct MacOs;

impl PlatformServices for MacOs {
    fn ensure_capture_permission(&self) -> Result<(), CaptureError> {
        // Phase 3: CGPreflightScreenCaptureAccess / CGRequestScreenCaptureAccess.
        Ok(())
    }

    fn has_native_app_menu(&self) -> bool {
        // The PixMe/Capture/File/Window menus live on the system menu bar.
        true
    }

    fn capture_permission_status(&self) -> Option<bool> {
        // Read-only TCC preflight — never prompts.
        Some(core_graphics::access::ScreenCaptureAccess.preflight())
    }
}

/// The app keeps the default `regular` activation policy: a permanent Dock
/// icon (launcher-style) so PixMe owns the system menu bar whenever one of
/// its windows is focused — accessory apps can never own it. Passes the
/// macOS artwork (`pixme-1024.png`, with Apple icon-grid margins) to
/// `eframe`'s `AppTitleIconSetter`, which installs it via `NSBitmapImageRep`
/// + `setApplicationIconImage:`.
pub fn apply_native_options(options: &mut eframe::NativeOptions) {
    options.viewport = options.viewport.clone().with_icon(icons::dock_icon_data());
}

/// Status-item icon edge in pixels (22 pt menu-bar slot × 2 for Retina —
/// `menubar-template@2x.png` is exactly this size).
const STATUS_ICON_PX: u32 = 44;

/// Menu handles kept alive for the app lifetime, plus the i18n keys needed
/// to relabel them when the language changes.
#[derive(Default)]
struct Labels {
    items: Vec<(MenuItem, &'static str, String)>,
    predef: Vec<(PredefinedMenuItem, &'static str)>,
    subs: Vec<(Submenu, &'static str)>,
}

impl Labels {
    fn relabel(&self) {
        for (item, key, _id) in &self.items {
            item.set_text(t!(*key).as_ref());
        }
        for (item, key) in &self.predef {
            item.set_text(t!(*key).as_ref());
        }
        for (submenu, key) in &self.subs {
            submenu.set_text(t!(*key).as_ref());
        }
    }
}

/// Everything that must outlive `install_ui`.
struct Installed {
    _tray: Option<TrayIcon>,
    _app_menu: Menu,
    labels: Labels,
}

thread_local! {
    static INSTALLED: RefCell<Option<Installed>> = const { RefCell::new(None) };
}

/// Appends `item`, logging instead of failing hard — a missing menu item is
/// recoverable.
fn append(menu: &impl MenuAppend, item: &dyn IsMenuItem) {
    if let Err(err) = menu.append(item) {
        tracing::warn!("cannot append menu item: {err}");
    }
}

fn separator(menu: &impl MenuAppend) {
    append(menu, &PredefinedMenuItem::separator());
}

/// The append operation `Menu` and `Submenu` share.
trait MenuAppend {
    fn append(&self, item: &dyn IsMenuItem) -> tray_icon::menu::Result<()>;
}
impl MenuAppend for Menu {
    fn append(&self, item: &dyn IsMenuItem) -> tray_icon::menu::Result<()> {
        Menu::append(self, item)
    }
}
impl MenuAppend for Submenu {
    fn append(&self, item: &dyn IsMenuItem) -> tray_icon::menu::Result<()> {
        Submenu::append(self, item)
    }
}

/// Builds a labelled item, records its key for relabeling and appends it.
fn item(
    labels: &mut Labels,
    menu: &impl MenuAppend,
    id: &str,
    key: &'static str,
    accelerator: Option<Accelerator>,
) {
    let item = MenuItem::with_id(id, t!(key).as_ref(), true, accelerator);
    append(menu, &item);
    labels.items.push((item, key, id.to_owned()));
}

/// Installs the status item, the application menus and the menu-event
/// handler forwarding clicks as [`crate::app::Command`]s.
pub fn install_ui(ctx: &egui::Context) {
    let repaint_ctx = ctx.clone();
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        if let Some(command) = super::command_from_menu_id(event.id().0.as_str()) {
            super::queue_menu_command(command);
            // Wake every live viewport so pump() runs without waiting
            // for an occluded root anchor or the next mouse event.
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

    let mut labels = Labels::default();
    // Status item: pure toggle surface — left click toggles the viewer
    // (no right-click menu: the app menu carries the same entries).
    let tray = install_status_item();
    install_dock_reopen_hook(ctx.clone());

    // Application menu bar (system menu bar, always present).
    let app_menu = Menu::new();
    let app = Submenu::new("PixMe", true);
    // The About entry opens the settings modal on its About page —
    // one About surface on both OSes, no native panel.
    let about = MenuItem::with_id("about", t!("menu.app.about").as_ref(), true, None);
    append(&app, &about);
    labels
        .items
        .push((about, "menu.app.about", "about".to_owned()));
    // Greyed until the website exists — registered for relabeling but its
    // id maps to no command, so `sync_menu_enabled` leaves it disabled.
    let visit = MenuItem::with_id(
        "visit_site",
        t!("menu.help.visit_site").as_ref(),
        false,
        None,
    );
    append(&app, &visit);
    labels
        .items
        .push((visit, "menu.help.visit_site", "visit_site".to_owned()));
    separator(&app);
    item(
        &mut labels,
        &app,
        "settings",
        "menu.app.preferences",
        Some(Accelerator::new(Modifiers::META, Code::Comma)),
    );
    separator(&app);
    item(
        &mut labels,
        &app,
        "quit",
        "menu.app.quit",
        Some(Accelerator::new(Modifiers::META, Code::KeyQ)),
    );
    append(&app_menu, &app);

    // The Edit submenu carries the OS copy/paste conventions (⌘C/⌘V):
    // key equivalents go through AppKit, so they reach us no matter
    // which egui viewport holds focus.
    let edit = Submenu::new(t!("menu.edit").as_ref(), true);
    labels.subs.push((edit.clone(), "menu.edit"));
    item(
        &mut labels,
        &edit,
        "edit.undo",
        "menu.edit.undo",
        Some(Accelerator::new(Modifiers::META, Code::KeyZ)),
    );
    item(
        &mut labels,
        &edit,
        "edit.redo",
        "menu.edit.redo",
        Some(Accelerator::new(
            Modifiers::META | Modifiers::SHIFT,
            Code::KeyZ,
        )),
    );
    separator(&edit);
    item(&mut labels, &edit, "edit.copy", "menu.edit.copy", None);
    append(&app_menu, &edit);

    let window = Submenu::new(t!("menu.window").as_ref(), true);
    labels.subs.push((window.clone(), "menu.window"));
    for (make, key) in [
        (
            PredefinedMenuItem::minimize as fn(Option<&str>) -> PredefinedMenuItem,
            "menu.window.minimize",
        ),
        (PredefinedMenuItem::zoom, "menu.window.zoom"),
        (PredefinedMenuItem::fullscreen, "menu.window.fullscreen"),
    ] {
        let predef = make(Some(t!(key).as_ref()));
        append(&window, &predef);
        labels.predef.push((predef, key));
    }
    append(&app_menu, &window);

    app_menu.init_for_nsapp();

    INSTALLED.with(|cell| {
        *cell.borrow_mut() = Some(Installed {
            _tray: tray,
            _app_menu: app_menu,
            labels,
        });
    });
}

/// Rewrites every menu label in the current language.
pub fn relabel_menus() {
    INSTALLED.with(|cell| {
        if let Some(installed) = &*cell.borrow() {
            installed.labels.relabel();
        }
    });
}

/// Greys out menu items whose command [`crate::app::is_command_enabled`]
/// forbids under `modal` — app menu bar and status-item menu share the
/// one table. Called from `Shared::pump`, always on the main thread where
/// these thread_local handles live.
pub fn sync_menu_enabled(
    modal: crate::app::ModalState,
    has_doc: bool,
    has_file: bool,
    has_last_dir: bool,
) {
    let enabled = |command: &crate::app::Command| {
        crate::app::is_command_enabled(command, modal)
            && match command {
                // Same `has_doc`/`has_file`/`has_last_dir` gates as the
                // viewer's egui menu rows — `is_command_enabled` sees
                // the modal state only.
                crate::app::Command::SaveActive
                | crate::app::Command::SaveAll
                | crate::app::Command::RenameActive
                | crate::app::Command::DeleteActive
                | crate::app::Command::PrintActive => has_doc,
                crate::app::Command::DeleteFileActive => has_file,
                crate::app::Command::OpenLastDir => has_last_dir,
                _ => true,
            }
    };
    INSTALLED.with(|cell| {
        if let Some(installed) = &*cell.borrow() {
            for (item, _key, id) in &installed.labels.items {
                if let Some(command) = super::command_from_menu_id(id) {
                    item.set_enabled(enabled(&command));
                }
            }
        }
    });
}

/// The menu-bar status item with its template (monochrome) icon.
fn install_status_item() -> Option<TrayIcon> {
    let mut builder = TrayIcon::builder()
        // Left click toggles the viewer (TrayIconEvent); no menu —
        // the app menu bar's PixMe menu carries the same entries.
        .with_menu_on_left_click(false)
        .with_tooltip("PixMe");
    match tray_icon::Icon::from_rgba(icons::menubar_icon_rgba(), STATUS_ICON_PX, STATUS_ICON_PX) {
        Ok(icon) => builder = builder.with_icon_templated(icon),
        Err(err) => tracing::warn!("status item icon unavailable: {err}"),
    }
    match builder.build() {
        Ok(tray) => {
            tracing::info!("status item installed");
            Some(tray)
        }
        Err(err) => {
            tracing::warn!("cannot create status item: {err}");
            None
        }
    }
}

/// All on-screen windows, front-to-back, gathered in ONE
/// `CGWindowListCopyWindowInfo` pass.
///
/// `xcap::Window` re-runs this enumeration inside every accessor
/// (`pid()`, `x()`, `title()`…), so collecting window info through it
/// costs O(n²) system calls and freezes the UI for seconds. The record
/// also carries `kCGWindowLayer`, which `xcap` drops: without it the
/// Dock (20), menu bar (24), status items (25) and notification banners
/// are indistinguishable from real windows.
pub fn window_list() -> Vec<super::WindowRecord> {
    use core_foundation::base::{CFType, CFTypeRef, TCFType, ToVoid};
    use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
    use core_foundation::number::CFNumber;
    use core_foundation::string::CFString;
    use core_graphics::window::{
        copy_window_info, kCGWindowListExcludeDesktopElements, kCGWindowListOptionOnScreenOnly,
    };

    let Some(info) = copy_window_info(
        kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
        0,
    ) else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(info.len() as usize);
    for i in 0..info.len() {
        let Some(ptr) = info.get(i) else { continue };
        // Elements are CFDictionaryRefs borrowed from the array —
        // wrap_under_get_rule keeps ownership with the array.
        let dict: CFDictionary<*const std::ffi::c_void, *const std::ffi::c_void> =
            unsafe { CFDictionary::wrap_under_get_rule(*ptr as CFDictionaryRef) };
        let number = |key: &str| -> Option<i64> {
            dict.find(CFString::new(key).to_void())
                .and_then(|v| unsafe {
                    CFType::wrap_under_get_rule(*v as CFTypeRef).downcast::<CFNumber>()
                })
                .and_then(|n| n.to_i64())
        };
        let text = |key: &str| -> String {
            dict.find(CFString::new(key).to_void())
                .and_then(|v| unsafe {
                    CFType::wrap_under_get_rule(*v as CFTypeRef).downcast::<CFString>()
                })
                .map(|s| s.to_string())
                .unwrap_or_default()
        };
        let Some(id) = number("kCGWindowNumber") else {
            continue;
        };
        // kCGWindowBounds is a nested {X,Y,Width,Height} dictionary.
        let rect = dict
            .find(CFString::new("kCGWindowBounds").to_void())
            .and_then(|v| unsafe {
                CFType::wrap_under_get_rule(*v as CFTypeRef)
                    .downcast::<CFDictionary<*const std::ffi::c_void, *const std::ffi::c_void>>()
            })
            .and_then(|bounds| {
                let b = |k: &str| -> Option<i64> {
                    bounds
                        .find(CFString::new(k).to_void())
                        .and_then(|v| unsafe {
                            CFType::wrap_under_get_rule(*v as CFTypeRef).downcast::<CFNumber>()
                        })
                        .and_then(|n| n.to_i64())
                };
                Some(crate::capture::PhysicalRect {
                    x: b("X")? as i32,
                    y: b("Y")? as i32,
                    width: b("Width")? as u32,
                    height: b("Height")? as u32,
                })
            });
        let Some(rect) = rect else { continue };
        out.push(super::WindowRecord {
            id: id as u32,
            pid: number("kCGWindowOwnerPID").unwrap_or_default() as u32,
            app_name: text("kCGWindowOwnerName"),
            title: text("kCGWindowName"),
            rect,
            layer: number("kCGWindowLayer").unwrap_or_default() as i32,
        });
    }
    out
}

/// System layout: `~/Library/Application Support/pixme` for data,
/// `~/Library/Logs/PixMe` for logs — an `.app` bundle is never written
/// into (it may be read-only), and both locations survive an update of
/// the app itself.
pub fn resolve_base_dirs() -> crate::paths::BaseDirs {
    let data = directories::ProjectDirs::from("", "", "pixme")
        .map(|d| d.data_dir().to_path_buf())
        .unwrap_or_else(|| std::env::temp_dir().join("pixme"));
    let logs = directories::UserDirs::new()
        .map(|u| u.home_dir().join("Library/Logs/PixMe"))
        .unwrap_or_else(|| data.join("logs"));
    crate::paths::BaseDirs {
        data,
        logs,
        fallback_from: None,
    }
}

/// "macOS 15.x" — `sw_vers` ships with the OS; ~20 ms once at startup.
pub fn os_version() -> String {
    std::process::Command::new("sw_vers")
        .arg("-productVersion")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .filter(|v| !v.is_empty())
        .map_or_else(|| "macOS ?".to_owned(), |v| format!("macOS {v}"))
}

/// Opens `dir` in Finder — the Settings "open folder" buttons.
pub fn open_folder(dir: &std::path::Path) {
    let _ = std::process::Command::new("open").arg(dir).status();
}

/// Reveals `path` in Finder (`open -R` selects the file); falls back to
/// opening its parent folder.
pub fn reveal_in_file_manager(path: &std::path::Path) {
    let ok = std::process::Command::new("open")
        .arg("-R")
        .arg(path)
        .status()
        .is_ok_and(|s| s.success());
    if !ok && let Some(parent) = path.parent() {
        let _ = std::process::Command::new("open").arg(parent).status();
    }
}

/// Shows the native macOS print panel (printer picker, preview, copies)
/// for `img` scaled to fit the page — `job_title` names the job in the
/// queue. Runs modally, so it must be called on the main thread. Uses
/// `NSPrintOperation` through the Objective-C runtime directly — no
/// extra crate, matching `cursor_snapshot`'s FFI pattern.
pub fn print_image(img: &image::RgbaImage, job_title: &str) -> Result<(), String> {
    use core_graphics::geometry::{CGPoint, CGRect, CGSize};
    use std::ffi::{CString, c_void};

    #[link(name = "objc", kind = "dylib")]
    unsafe extern "C" {
        fn objc_getClass(name: *const std::ffi::c_char) -> *mut c_void;
        fn sel_registerName(name: *const std::ffi::c_char) -> *mut c_void;
        fn objc_msgSend();
    }

    type MsgSendId = unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void;
    type MsgSendId1 = unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> *mut c_void;
    type MsgSendIdStr =
        unsafe extern "C" fn(*mut c_void, *mut c_void, *const std::ffi::c_char) -> *mut c_void;
    type MsgSendIdBytes =
        unsafe extern "C" fn(*mut c_void, *mut c_void, *const u8, usize) -> *mut c_void;
    type MsgSendIdRect = unsafe extern "C" fn(*mut c_void, *mut c_void, CGRect) -> *mut c_void;
    type MsgSendSize = unsafe extern "C" fn(*mut c_void, *mut c_void) -> CGSize;
    type MsgSendVoid = unsafe extern "C" fn(*mut c_void, *mut c_void);
    type MsgSendVoid1 = unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void);
    type MsgSendVoidLong = unsafe extern "C" fn(*mut c_void, *mut c_void, isize);
    type MsgSendVoidBool = unsafe extern "C" fn(*mut c_void, *mut c_void, bool);
    type MsgSendBool = unsafe extern "C" fn(*mut c_void, *mut c_void) -> bool;
    type MsgSendBoolSel = unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> bool;

    // NSImage decodes PNG bytes — the same format the export path uses.
    let png = crate::store::ser::png_encode(img).map_err(|e| e.to_string())?;
    let title = CString::new(job_title).map_err(|e| e.to_string())?;

    unsafe {
        let send_id: MsgSendId = std::mem::transmute(objc_msgSend as *const ());
        let send_id1: MsgSendId1 = std::mem::transmute(objc_msgSend as *const ());
        let send_str: MsgSendIdStr = std::mem::transmute(objc_msgSend as *const ());
        let send_bytes: MsgSendIdBytes = std::mem::transmute(objc_msgSend as *const ());
        let send_rect: MsgSendIdRect = std::mem::transmute(objc_msgSend as *const ());
        let send_size: MsgSendSize = std::mem::transmute(objc_msgSend as *const ());
        let send_void: MsgSendVoid = std::mem::transmute(objc_msgSend as *const ());
        let send_void1: MsgSendVoid1 = std::mem::transmute(objc_msgSend as *const ());
        let send_long: MsgSendVoidLong = std::mem::transmute(objc_msgSend as *const ());
        let send_flag: MsgSendVoidBool = std::mem::transmute(objc_msgSend as *const ());
        let send_bool: MsgSendBool = std::mem::transmute(objc_msgSend as *const ());
        let send_bool_target: MsgSendBoolSel = std::mem::transmute(objc_msgSend as *const ());

        // Class-method objects below are autoreleased: drain them with a
        // pool on return. `image`/`view`/`pool` are alloc+init (+1) and
        // get an explicit `release` — after `runOperation`, which keeps
        // the view alive for the panel's whole lifetime.
        let pool = send_id(
            send_id(
                objc_getClass(c"NSAutoreleasePool".as_ptr()),
                sel_registerName(c"alloc".as_ptr()),
            ),
            sel_registerName(c"init".as_ptr()),
        );

        let data = send_bytes(
            objc_getClass(c"NSData".as_ptr()),
            sel_registerName(c"dataWithBytes:length:".as_ptr()),
            png.as_ptr(),
            png.len(),
        );
        let image = send_id1(
            send_id(
                objc_getClass(c"NSImage".as_ptr()),
                sel_registerName(c"alloc".as_ptr()),
            ),
            sel_registerName(c"initWithData:".as_ptr()),
            data,
        );
        let size = if image.is_null() {
            CGSize::new(1.0, 1.0)
        } else {
            send_size(image, sel_registerName(c"size".as_ptr()))
        };
        let view = send_rect(
            send_id(
                objc_getClass(c"NSImageView".as_ptr()),
                sel_registerName(c"alloc".as_ptr()),
            ),
            sel_registerName(c"initWithFrame:".as_ptr()),
            CGRect::new(&CGPoint::new(0.0, 0.0), &size),
        );

        let op = if view.is_null() || image.is_null() {
            std::ptr::null_mut()
        } else {
            send_void1(view, sel_registerName(c"setImage:".as_ptr()), image);
            send_id1(
                objc_getClass(c"NSPrintOperation".as_ptr()),
                sel_registerName(c"printOperationWithView:".as_ptr()),
                view,
            )
        };

        let ok = if op.is_null() {
            false
        } else {
            let info = send_id(op, sel_registerName(c"printInfo".as_ptr()));
            // NSFitPagination = 1 on both axes: AppKit scales the view
            // to a single printed page, keeping the image's aspect.
            send_long(
                info,
                sel_registerName(c"setVerticalPagination:".as_ptr()),
                1,
            );
            send_long(
                info,
                sel_registerName(c"setHorizontalPagination:".as_ptr()),
                1,
            );
            let ns_title = send_str(
                objc_getClass(c"NSString".as_ptr()),
                sel_registerName(c"stringWithUTF8String:".as_ptr()),
                title.as_ptr(),
            );
            // `jobTitle` moved from NSPrintInfo to NSPrintOperation on
            // macOS 14 — send it to whichever object answers the selector.
            let title_sel = sel_registerName(c"setJobTitle:".as_ptr());
            let responds_sel = sel_registerName(c"respondsToSelector:".as_ptr());
            let title_target = if send_bool_target(op, responds_sel, title_sel) {
                op
            } else if send_bool_target(info, responds_sel, title_sel) {
                info
            } else {
                std::ptr::null_mut()
            };
            if !title_target.is_null() {
                send_void1(title_target, title_sel, ns_title);
            }
            send_flag(op, sel_registerName(c"setShowsPrintPanel:".as_ptr()), true);
            send_flag(
                op,
                sel_registerName(c"setShowsProgressPanel:".as_ptr()),
                true,
            );
            send_bool(op, sel_registerName(c"runOperation".as_ptr()))
        };

        if !image.is_null() {
            send_void(image, sel_registerName(c"release".as_ptr()));
        }
        if !view.is_null() {
            send_void(view, sel_registerName(c"release".as_ptr()));
        }
        send_void(pool, sel_registerName(c"drain".as_ptr()));

        if ok {
            Ok(())
        } else {
            Err("print panel cancelled or failed".into())
        }
    }
}

/// Global mouse position in the OS-native display space (CGDisplay
/// bounds: top-left of the primary display is `(0, 0)`, points — the
/// exact space `MonitorInfo::rect` uses).
pub fn cursor_position() -> Option<(f32, f32)> {
    use core_graphics::event::CGEvent;
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState).ok()?;
    let loc = CGEvent::new(source).ok()?.location();
    Some((loc.x as f32, loc.y as f32))
}

/// Whether the left mouse button is currently held — read from the HID
/// event source, NOT from window events: the 1 px strip at the very top
/// of the screen is a WindowServer tracking zone that swallows
/// `mouseDown` before delivery, so a press starting there never reaches
/// egui. The overlay polls this so a drag started at the screen edge
/// still creates a selection.
pub fn left_mouse_down() -> bool {
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGEventSourceButtonState(state_id: i32, button: i32) -> bool;
    }
    // kCGEventSourceStateHIDSystemState = 1, kCGMouseButtonLeft = 0.
    unsafe { CGEventSourceButtonState(1, 0) }
}

/// One short tick — AppKit `NSBeep`, a plain C symbol playing the user's
/// chosen system alert sound (respects its volume and mute).
pub fn tick() {
    #[link(name = "AppKit", kind = "framework")]
    unsafe extern "C" {
        fn NSBeep();
    }
    unsafe { NSBeep() }
}

/// Captures the current OS mouse cursor (`NSCursor.currentSystemCursor`, or
/// `NSCursor.currentCursor` as fallback) rendered at `scale_factor` into an
/// RGBA bitmap, along with its global display position and scaled hotspot.
///
/// Uses the Objective-C runtime (`objc_getClass` / `sel_registerName` /
/// `objc_msgSend`, always linked via `libobjc`) and `core-graphics` (already
/// a direct dependency) so no extra crate is needed.
pub fn cursor_snapshot(scale_factor: f32) -> Option<super::CursorSnapshot> {
    use core_graphics::base::kCGImageAlphaPremultipliedLast;
    use core_graphics::color_space::CGColorSpace;
    use core_graphics::context::CGContext;
    use core_graphics::geometry::{CGPoint, CGRect, CGSize};
    use core_graphics::sys::CGImageRef;
    use std::ffi::c_void;

    #[link(name = "objc", kind = "dylib")]
    unsafe extern "C" {
        fn objc_getClass(name: *const std::ffi::c_char) -> *mut c_void;
        fn sel_registerName(name: *const std::ffi::c_char) -> *mut c_void;
        fn objc_msgSend();
    }

    type MsgSendId = unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void;
    type MsgSendPoint = unsafe extern "C" fn(*mut c_void, *mut c_void) -> CGPoint;
    type MsgSendSize = unsafe extern "C" fn(*mut c_void, *mut c_void) -> CGSize;
    type MsgSendCgImage = unsafe extern "C" fn(
        *mut c_void,
        *mut c_void,
        *mut CGRect,
        *mut c_void,
        *mut c_void,
    ) -> CGImageRef;

    let position = cursor_position()?;
    let scale = f64::from(scale_factor.max(1.0));

    unsafe {
        let send_id: MsgSendId = std::mem::transmute(objc_msgSend as *const ());
        let send_pt: MsgSendPoint = std::mem::transmute(objc_msgSend as *const ());
        let send_sz: MsgSendSize = std::mem::transmute(objc_msgSend as *const ());
        let send_cg: MsgSendCgImage = std::mem::transmute(objc_msgSend as *const ());

        let cls = objc_getClass(c"NSCursor".as_ptr());
        if cls.is_null() {
            return None;
        }
        let mut cursor = send_id(cls, sel_registerName(c"currentSystemCursor".as_ptr()));
        if cursor.is_null() {
            cursor = send_id(cls, sel_registerName(c"currentCursor".as_ptr()));
        }
        // The pointer may sit over another app's window (a delayed capture,
        // or right after the viewer tore down): both "current" cursors are
        // then NULL — fall back to the standard arrow so the stamp always
        // has an image when the user asked for the pointer in captures.
        if cursor.is_null() {
            cursor = send_id(cls, sel_registerName(c"arrow".as_ptr()));
        }
        if cursor.is_null() {
            return None;
        }

        let hotspot = send_pt(cursor, sel_registerName(c"hotSpot".as_ptr()));
        let ns_image = send_id(cursor, sel_registerName(c"image".as_ptr()));
        if ns_image.is_null() {
            return None;
        }
        let size = send_sz(ns_image, sel_registerName(c"size".as_ptr()));
        if size.width <= 0.0 || size.height <= 0.0 {
            return None;
        }

        let w = (size.width * scale).round().max(1.0) as usize;
        let h = (size.height * scale).round().max(1.0) as usize;
        let mut proposed = CGRect::new(&CGPoint::new(0.0, 0.0), &CGSize::new(w as f64, h as f64));
        let cg_img = send_cg(
            ns_image,
            sel_registerName(c"CGImageForProposedRect:context:hints:".as_ptr()),
            &mut proposed,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );
        if cg_img.is_null() {
            return None;
        }

        let cs = CGColorSpace::create_device_rgb();
        let mut ctx = CGContext::create_bitmap_context(
            None,
            w,
            h,
            8,
            w * 4,
            &cs,
            kCGImageAlphaPremultipliedLast,
        );
        let draw_rect = CGRect::new(&CGPoint::new(0.0, 0.0), &CGSize::new(w as f64, h as f64));
        // `CGImageForProposedRect` returns an autoreleased `CGImageRef`
        // (get rule) — wrap in `ManuallyDrop<CGImage>` so `draw_image` can
        // borrow it without releasing on drop.
        let img = std::mem::ManuallyDrop::new(std::mem::transmute::<
            CGImageRef,
            core_graphics::image::CGImage,
        >(cg_img));
        ctx.draw_image(draw_rect, &img);

        let mut buf = ctx.data().to_vec();
        // Un-premultiply RGBA for `image::RgbaImage`.
        for px in buf.as_chunks_mut::<4>().0 {
            let a = px[3] as u32;
            if a > 0 && a < 255 {
                px[0] = ((px[0] as u32 * 255 + a / 2) / a).min(255) as u8;
                px[1] = ((px[1] as u32 * 255 + a / 2) / a).min(255) as u8;
                px[2] = ((px[2] as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
        let image = image::RgbaImage::from_raw(w as u32, h as u32, buf)?;
        Some(super::CursorSnapshot {
            image,
            position,
            hotspot_px: (
                (hotspot.x * scale).round() as i32,
                (hotspot.y * scale).round() as i32,
            ),
        })
    }
}

/// Hides the OS cursor on every connected display (per-display counter):
/// `CGWindowListCreateImage` composites the cursor into the frame, and this
/// is the only way to keep it out of a capture. Works regardless of which
/// application has focus. Always balance with [`unhide_cursor`].
pub fn hide_cursor() {
    unsafe {
        for m in xcap::Monitor::all().unwrap_or_default() {
            if let Ok(id) = m.id() {
                core_graphics::display::CGDisplayHideCursor(id);
            }
        }
    }
}

/// Balances [`hide_cursor`] (see above).
pub fn unhide_cursor() {
    unsafe {
        for m in xcap::Monitor::all().unwrap_or_default() {
            if let Ok(id) = m.id() {
                core_graphics::display::CGDisplayShowCursor(id);
            }
        }
    }
}

/// Closes any third-party popup menu (`NSMenu`, layer ≥ 100) still tracking
/// the mouse after the screen frames have been frozen, and activates PixMe.
///
/// Status-bar extras (`LSUIElement` daemons like SwitchResX) run a modal menu
/// tracking loop at `kCGPopUpMenuWindowLevel` (101) that ignores normal app
/// activation and keeps eating mouse events. Sending `Escape` (key code 53)
/// once the pixels are already captured in `frozen_all` ends that tracking
/// loop cleanly so the selection overlay owns the pointer.
///
/// Returns `true` when a synthetic Escape was actually posted — the overlay
/// uses it to swallow that Esc when it lands on the freshly-keyed window.
pub fn dismiss_active_menus() -> bool {
    use core_graphics::sys::CGEventRef;
    use std::ffi::c_void;

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGEventCreateKeyboardEvent(
            source: *const c_void,
            virtual_key: u16,
            key_down: bool,
        ) -> CGEventRef;
        fn CGEventPost(tap: u32, event: CGEventRef);
        fn CGEventPostToPid(pid: i32, event: CGEventRef);
        fn CFRelease(cf: *const c_void);
    }
    #[link(name = "objc", kind = "dylib")]
    unsafe extern "C" {
        fn objc_getClass(name: *const std::ffi::c_char) -> *mut c_void;
        fn sel_registerName(name: *const std::ffi::c_char) -> *mut c_void;
        fn objc_msgSend();
    }

    const ESCAPE_KEYCODE: u16 = 53;
    const POPUP_MENU_LAYER: i32 = 100;
    const HID_EVENT_TAP: u32 = 0;

    let own_pid = std::process::id();
    // WindowServer keeps permanent layer ≥ 100 windows (cursor, menubar
    // backstop) that never run a menu tracking loop — without the exclusion
    // the Esc below fires on every single capture.
    let mut menu_pids: Vec<u32> = window_list()
        .into_iter()
        .filter(|w| w.pid != own_pid && w.layer >= POPUP_MENU_LAYER && w.app_name != "WindowServer")
        .map(|w| w.pid)
        .collect();
    menu_pids.sort_unstable();
    menu_pids.dedup();

    let mut posted = false;
    if !menu_pids.is_empty() {
        unsafe {
            let down = CGEventCreateKeyboardEvent(std::ptr::null(), ESCAPE_KEYCODE, true);
            let up = CGEventCreateKeyboardEvent(std::ptr::null(), ESCAPE_KEYCODE, false);
            if !down.is_null() && !up.is_null() {
                for pid in &menu_pids {
                    CGEventPostToPid(*pid as i32, down);
                    CGEventPostToPid(*pid as i32, up);
                }
                CGEventPost(HID_EVENT_TAP, down);
                CGEventPost(HID_EVENT_TAP, up);
                posted = true;
                tracing::info!(?menu_pids, "posted Esc to dismiss foreign popup menus");
            }
            if !down.is_null() {
                CFRelease(down.cast());
            }
            if !up.is_null() {
                CFRelease(up.cast());
            }
        }
    }

    activate_app();
    posted
}

/// Brings PixMe frontmost so `presentationOptions` and `makeKey` apply —
/// `-[NSApplication activate]` on macOS 14+ (`activateIgnoringOtherApps:`
/// is deprecated there and silently ignored on 15.x, which left the real
/// menu bar interactive during captures); the legacy selector is the
/// fallback on older systems. Main thread only.
pub fn activate_app() {
    use std::ffi::c_void;

    #[link(name = "objc", kind = "dylib")]
    unsafe extern "C" {
        fn objc_getClass(name: *const std::ffi::c_char) -> *mut c_void;
        fn sel_registerName(name: *const std::ffi::c_char) -> *mut c_void;
        fn objc_msgSend();
    }

    type MsgSendId = unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void;
    type MsgSendUsize = unsafe extern "C" fn(*mut c_void, *mut c_void) -> usize;
    type MsgSendIdUsize = unsafe extern "C" fn(*mut c_void, *mut c_void, usize) -> *mut c_void;
    type MsgSendIsize = unsafe extern "C" fn(*mut c_void, *mut c_void) -> isize;
    type MsgSendVoid = unsafe extern "C" fn(*mut c_void, *mut c_void);
    type MsgSendVoidId = unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void);
    type MsgSendVoidBool = unsafe extern "C" fn(*mut c_void, *mut c_void, bool);
    type MsgSendBool = unsafe extern "C" fn(*mut c_void, *mut c_void) -> bool;
    type MsgSendBoolSel = unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> bool;

    unsafe {
        let send_id: MsgSendId = std::mem::transmute(objc_msgSend as *const ());
        let send_usize: MsgSendUsize = std::mem::transmute(objc_msgSend as *const ());
        let send_id_usize: MsgSendIdUsize = std::mem::transmute(objc_msgSend as *const ());
        let send_isize: MsgSendIsize = std::mem::transmute(objc_msgSend as *const ());
        let send_void: MsgSendVoid = std::mem::transmute(objc_msgSend as *const ());
        let send_void_id: MsgSendVoidId = std::mem::transmute(objc_msgSend as *const ());
        let send_void_bool: MsgSendVoidBool = std::mem::transmute(objc_msgSend as *const ());
        let send_bool: MsgSendBool = std::mem::transmute(objc_msgSend as *const ());
        let send_bool_sel: MsgSendBoolSel = std::mem::transmute(objc_msgSend as *const ());
        let cls = objc_getClass(c"NSApplication".as_ptr());
        if cls.is_null() {
            return;
        }
        let app = send_id(cls, sel_registerName(c"sharedApplication".as_ptr()));
        if app.is_null() {
            return;
        }
        let responds = sel_registerName(c"respondsToSelector:".as_ptr());
        let activate = sel_registerName(c"activate".as_ptr());
        if send_bool_sel(app, responds, activate) {
            send_void(app, activate);
        } else {
            send_void_bool(
                app,
                sel_registerName(c"activateIgnoringOtherApps:".as_ptr()),
                true,
            );
        }
        // Any normal-level window (the viewer — the 1×1 anchor is
        // floating-level) that is miniaturized in the Dock is pulled
        // straight out and ordered front: `ViewportCommand::Minimized(false)`
        // waits on the child viewport's pass, which AppKit pauses while
        // the window is minimized.
        let windows = send_id(app, sel_registerName(c"windows".as_ptr()));
        if !windows.is_null() {
            let count = send_usize(windows, sel_registerName(c"count".as_ptr()));
            let sel_obj = sel_registerName(c"objectAtIndex:".as_ptr());
            let sel_level = sel_registerName(c"level".as_ptr());
            let sel_min = sel_registerName(c"isMiniaturized".as_ptr());
            let sel_demin = sel_registerName(c"deminiaturize:".as_ptr());
            let sel_vis = sel_registerName(c"isVisible".as_ptr());
            let sel_key_front = sel_registerName(c"makeKeyAndOrderFront:".as_ptr());
            for i in 0..count {
                let win = send_id_usize(windows, sel_obj, i);
                if win.is_null() || send_isize(win, sel_level) != 0 {
                    continue;
                }
                let was_min = send_bool(win, sel_min);
                if was_min {
                    send_void_id(win, sel_demin, std::ptr::null_mut());
                }
                if was_min || send_bool(win, sel_vis) {
                    send_void_id(win, sel_key_front, std::ptr::null_mut());
                }
            }
        }
    }
}

/// Whether the viewer's native `NSWindow` is currently miniaturized in
/// the Dock (yellow `-` button): `ToggleViewer` must then restore it
/// instead of closing it to `Idle`.
pub fn viewer_is_minimized() -> bool {
    use std::ffi::c_void;

    #[link(name = "objc", kind = "dylib")]
    unsafe extern "C" {
        fn objc_getClass(name: *const std::ffi::c_char) -> *mut c_void;
        fn sel_registerName(name: *const std::ffi::c_char) -> *mut c_void;
        fn objc_msgSend();
    }

    type MsgSendId = unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void;
    type MsgSendUsize = unsafe extern "C" fn(*mut c_void, *mut c_void) -> usize;
    type MsgSendIdUsize = unsafe extern "C" fn(*mut c_void, *mut c_void, usize) -> *mut c_void;
    type MsgSendIsize = unsafe extern "C" fn(*mut c_void, *mut c_void) -> isize;
    type MsgSendBool = unsafe extern "C" fn(*mut c_void, *mut c_void) -> bool;

    unsafe {
        let send_id: MsgSendId = std::mem::transmute(objc_msgSend as *const ());
        let send_usize: MsgSendUsize = std::mem::transmute(objc_msgSend as *const ());
        let send_id_usize: MsgSendIdUsize = std::mem::transmute(objc_msgSend as *const ());
        let send_isize: MsgSendIsize = std::mem::transmute(objc_msgSend as *const ());
        let send_bool: MsgSendBool = std::mem::transmute(objc_msgSend as *const ());
        let cls = objc_getClass(c"NSApplication".as_ptr());
        if cls.is_null() {
            return false;
        }
        let app = send_id(cls, sel_registerName(c"sharedApplication".as_ptr()));
        if app.is_null() {
            return false;
        }
        let windows = send_id(app, sel_registerName(c"windows".as_ptr()));
        if windows.is_null() {
            return false;
        }
        let count = send_usize(windows, sel_registerName(c"count".as_ptr()));
        let sel_obj = sel_registerName(c"objectAtIndex:".as_ptr());
        let sel_level = sel_registerName(c"level".as_ptr());
        let sel_min = sel_registerName(c"isMiniaturized".as_ptr());
        for i in 0..count {
            let win = send_id_usize(windows, sel_obj, i);
            if !win.is_null() && send_isize(win, sel_level) == 0 && send_bool(win, sel_min) {
                return true;
            }
        }
        false
    }
}

/// Enters the capture kiosk: hides the REAL menu bar and Dock and locks
/// process switching (Cmd+Tab) and Hide-application for as long as PixMe
/// is frontmost — the trick iScreenShot-style tools use so no system
/// chrome can be interacted with mid-capture. The frozen frame taken
/// before this call already carries the menu bar's and Dock's pixels, so
/// their screen areas stay selectable under the overlay while nothing
/// real can be clicked through. Reversed by [`capture_chrome_exit`]
/// (and by process exit — the options never outlive the app).
///
/// Only meaningful once the screen is frozen: called from the overlay
/// session after `frozen_all` lands, never before (hiding chrome first
/// would photograph the empty strips).
pub fn capture_chrome_enter() {
    /// `NSApplicationPresentationHideDock` — the Dock stays gone while
    /// PixMe is frontmost.
    const HIDE_DOCK: usize = 1 << 1;
    /// `NSApplicationPresentationHideMenuBar` — the menu bar cannot be
    /// clicked or revealed; the overlay's frozen image of it takes its
    /// place.
    const HIDE_MENU_BAR: usize = 1 << 3;
    /// `NSApplicationPresentationDisableProcessSwitching` — no Cmd+Tab
    /// away mid-capture.
    const DISABLE_PROCESS_SWITCHING: usize = 1 << 5;
    /// `NSApplicationPresentationDisableHideApplication` — Cmd+H cannot
    /// stash the overlay away either.
    const DISABLE_HIDE_APPLICATION: usize = 1 << 8;

    set_presentation_options(
        HIDE_DOCK | HIDE_MENU_BAR | DISABLE_PROCESS_SWITCHING | DISABLE_HIDE_APPLICATION,
    );
}

/// Leaves the capture kiosk: `presentationOptions` back to default — the
/// real menu bar and Dock slide back in. Called on every transition out
/// of `Capturing`; a no-op when the kiosk was never armed (instant
/// fullscreen captures, a failed freeze).
pub fn capture_chrome_exit() {
    set_presentation_options(0);
}

/// `NSApp.presentationOptions = options` — `setPresentationOptions:` on
/// the shared application, main thread only.
fn set_presentation_options(options: usize) {
    use std::ffi::c_void;

    #[link(name = "objc", kind = "dylib")]
    unsafe extern "C" {
        fn objc_getClass(name: *const std::ffi::c_char) -> *mut c_void;
        fn sel_registerName(name: *const std::ffi::c_char) -> *mut c_void;
        fn objc_msgSend();
    }

    type MsgSendId = unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void;
    type MsgSendVoidUsize = unsafe extern "C" fn(*mut c_void, *mut c_void, usize);

    unsafe {
        let send_id: MsgSendId = std::mem::transmute(objc_msgSend as *const ());
        let send_void_usize: MsgSendVoidUsize = std::mem::transmute(objc_msgSend as *const ());
        let cls = objc_getClass(c"NSApplication".as_ptr());
        if cls.is_null() {
            return;
        }
        let app = send_id(cls, sel_registerName(c"sharedApplication".as_ptr()));
        if app.is_null() {
            return;
        }
        send_void_usize(
            app,
            sel_registerName(c"setPresentationOptions:".as_ptr()),
            options,
        );
    }
}

/// Raises the capture overlay window to `kCGScreenSaverWindowLevel` (1000) —
/// above the system menu bar (25) and popup menus (101) — and makes it key.
/// `winit`'s `with_always_on_top()` stops at `kCGFloatingWindowLevel` (3),
/// leaving status-item menus visually and interactively on top of the veil.
pub fn elevate_overlay() {
    use std::ffi::c_void;

    #[link(name = "objc", kind = "dylib")]
    unsafe extern "C" {
        fn objc_getClass(name: *const std::ffi::c_char) -> *mut c_void;
        fn sel_registerName(name: *const std::ffi::c_char) -> *mut c_void;
        fn objc_msgSend();
    }

    type MsgSendId = unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void;
    type MsgSendUsize = unsafe extern "C" fn(*mut c_void, *mut c_void) -> usize;
    type MsgSendIdUsize = unsafe extern "C" fn(*mut c_void, *mut c_void, usize) -> *mut c_void;
    type MsgSendBool = unsafe extern "C" fn(*mut c_void, *mut c_void) -> bool;
    type MsgSendIsize = unsafe extern "C" fn(*mut c_void, *mut c_void) -> isize;
    type MsgSendVoidIsize = unsafe extern "C" fn(*mut c_void, *mut c_void, isize);
    type MsgSendVoidId = unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void);

    const FLOATING_LEVEL: isize = 3;
    const SCREEN_SAVER_LEVEL: isize = 1000;

    unsafe {
        let send_id: MsgSendId = std::mem::transmute(objc_msgSend as *const ());
        let send_usize: MsgSendUsize = std::mem::transmute(objc_msgSend as *const ());
        let send_id_usize: MsgSendIdUsize = std::mem::transmute(objc_msgSend as *const ());
        let send_bool: MsgSendBool = std::mem::transmute(objc_msgSend as *const ());
        let send_isize: MsgSendIsize = std::mem::transmute(objc_msgSend as *const ());
        let send_void_isize: MsgSendVoidIsize = std::mem::transmute(objc_msgSend as *const ());
        let send_void_id: MsgSendVoidId = std::mem::transmute(objc_msgSend as *const ());

        let cls = objc_getClass(c"NSApplication".as_ptr());
        if cls.is_null() {
            return;
        }
        let app = send_id(cls, sel_registerName(c"sharedApplication".as_ptr()));
        if app.is_null() {
            return;
        }
        activate_app();
        let windows = send_id(app, sel_registerName(c"windows".as_ptr()));
        if windows.is_null() {
            return;
        }
        let count = send_usize(windows, sel_registerName(c"count".as_ptr()));
        let sel_obj = sel_registerName(c"objectAtIndex:".as_ptr());
        let sel_vis = sel_registerName(c"isVisible".as_ptr());
        let sel_opaque = sel_registerName(c"isOpaque".as_ptr());
        let sel_level = sel_registerName(c"level".as_ptr());
        let sel_set_level = sel_registerName(c"setLevel:".as_ptr());
        let sel_key_front = sel_registerName(c"makeKeyAndOrderFront:".as_ptr());
        for i in 0..count {
            let win = send_id_usize(windows, sel_obj, i);
            if win.is_null() {
                continue;
            }
            // The capture overlay is the only visible, opaque always-on-top
            // window (the anchor, bar, countdown and pins are transparent).
            if send_bool(win, sel_vis)
                && send_bool(win, sel_opaque)
                && send_isize(win, sel_level) == FLOATING_LEVEL
            {
                send_void_isize(win, sel_set_level, SCREEN_SAVER_LEVEL);
                send_void_id(win, sel_key_front, std::ptr::null_mut());
            }
        }
    }
}

/// The display's stable UUID (`CGDisplayCreateUUIDFromDisplayID`) — derived
/// from the monitor's vendor/model/serial, it survives reconnects, port
/// changes and reboots, unlike `CGDirectDisplayID` which is per-session.
/// `None` when the OS reports none: the caller falls back to a geometry key.
pub(crate) fn stable_monitor_id(monitor: &crate::capture::MonitorInfo) -> Option<String> {
    use core_foundation::base::TCFType;
    use core_foundation::string::{CFString, CFStringRef};
    use std::ffi::c_void;

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGDisplayCreateUUIDFromDisplayID(display: u32) -> *const c_void;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        // `NULL` allocator = kCFAllocatorDefault.
        fn CFUUIDCreateString(alloc: *const c_void, uuid: *const c_void) -> CFStringRef;
        fn CFRelease(cf: *const c_void);
    }

    unsafe {
        let uuid = CGDisplayCreateUUIDFromDisplayID(monitor.id);
        if uuid.is_null() {
            return None;
        }
        let raw = CFUUIDCreateString(std::ptr::null(), uuid);
        CFRelease(uuid);
        if raw.is_null() {
            return None;
        }
        Some(CFString::wrap_under_create_rule(raw).to_string())
    }
}
/// No-op on macOS: the capture kiosk (`presentationOptions` +
/// screensaver-level overlay) already keeps the session's focus —
/// there is nothing to hand back.
pub fn refocus_capture_overlay(_ctx: &egui::Context, _id: egui::ViewportId) {}

/// Classifies `info` for the status-strip icon: `CGDisplayIsBuiltin`
/// covers the laptop panel. The `IODisplayCreateInfoDictionary`
/// transport splits the rest — `"Virtual"` marks virtual/indirect
/// displays; an iPad is only claimed `Tablet` when the product name
/// mentions it (Sidecar), never on the transport alone. Uncertain
/// lookups fall back to `Unknown`; a plain external monitor reports
/// no virtual transport → `External`.
pub(crate) fn monitor_kind(info: &crate::capture::MonitorInfo) -> crate::capture::MonitorKind {
    use crate::capture::MonitorKind;
    use std::ffi::{c_char, c_void};

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGDisplayIsBuiltin(display: u32) -> bool;
        fn CGDisplayIOServicePort(display: u32) -> u32;
    }
    unsafe extern "C" {
        fn dlsym(handle: *const c_void, symbol: *const c_char) -> *mut c_void;
    }
    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IORegistryEntryCreateCFProperties(
            entry: u32,
            properties: *mut core_foundation::dictionary::CFDictionaryRef,
            allocator: *const c_void,
            options: u32,
        ) -> i32;
        fn IOObjectRelease(object: u32) -> i32;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFDictionaryGetValue(dict: *const c_void, key: *const c_void) -> *const c_void;
        fn CFStringGetCString(
            s: *const c_void,
            buf: *mut c_char,
            size: isize,
            encoding: u32,
        ) -> bool;
        fn CFRelease(cf: *const c_void);
        fn CFNumberGetValue(n: *const c_void, ty: i64, out: *mut i32) -> bool;
    }

    let id = info.id;
    if unsafe { CGDisplayIsBuiltin(id) } {
        return MonitorKind::BuiltIn;
    }
    // `NSScreen.localizedName` is the one reliable Sidecar signal on
    // Apple Silicon — an iPad reports "Sidecar Display (AirPlay)" or
    // the device name there even though `CGDisplayIOServicePort` is
    // dead (returns 0). Checked first: it must not be gated behind
    // the IOKit probe below. `Tablet` only on an explicit iPad/
    // Sidecar marker; a virtual/AirPlay name without it → `Virtual`.
    let localized = ns_localized_name(id).to_lowercase();
    if localized.contains("ipad") || localized.contains("sidecar") {
        return MonitorKind::Tablet;
    }
    if localized.contains("virtual") || localized.contains("airplay") {
        return MonitorKind::Virtual;
    }
    // `CGDisplayIsVirtual` marks virtual displays — Sidecar iPads
    // included; they report no IODisplay transport, so without this
    // they fell to `Unknown`. It isn't exported by older SDKs — weak
    // `dlsym` lookup so the check simply stays false where missing.
    let is_virtual = unsafe {
        let f = dlsym(-2isize as *const c_void, c"CGDisplayIsVirtual".as_ptr());
        if f.is_null() {
            false
        } else {
            let f: unsafe extern "C" fn(u32) -> bool = std::mem::transmute(f);
            f(id)
        }
    };
    if is_virtual {
        return MonitorKind::Virtual;
    }

    let port = unsafe { CGDisplayIOServicePort(id) };
    if port == 0 {
        // No IOKit entry (Apple Silicon) — a localized name was still
        // readable → a real non-builtin display IS external; nothing
        // readable at all → Unknown (never guessed).
        return if localized.is_empty() {
            MonitorKind::Unknown
        } else {
            MonitorKind::External
        };
    }
    let mut dict: core_foundation::dictionary::CFDictionaryRef = std::ptr::null();
    let ok = unsafe { IORegistryEntryCreateCFProperties(port, &mut dict, std::ptr::null(), 0) };
    unsafe { IOObjectRelease(port) };
    if ok != 0 || dict.is_null() {
        return MonitorKind::Unknown;
    }
    let read_str = |key: &core::ffi::CStr| -> String {
        unsafe {
            let cf_key = {
                #[link(name = "CoreFoundation", kind = "framework")]
                unsafe extern "C" {
                    fn CFStringCreateWithCString(
                        alloc: *const c_void,
                        s: *const c_char,
                        enc: u32,
                    ) -> *const c_void;
                }
                CFStringCreateWithCString(std::ptr::null(), key.as_ptr(), 0x08000100)
            };
            if cf_key.is_null() {
                return String::new();
            }
            let v = CFDictionaryGetValue(dict as *const c_void, cf_key);
            CFRelease(cf_key);
            let mut buf = [0i8; 128];
            if !v.is_null() && CFStringGetCString(v, buf.as_mut_ptr(), 128, 0x08000100) {
                std::ffi::CStr::from_ptr(buf.as_ptr())
                    .to_string_lossy()
                    .to_lowercase()
            } else {
                String::new()
            }
        }
    };
    let name = read_str(c"IOServicePropertyTransportType");
    let product = read_str(c"kDisplayProductName");
    unsafe { CFRelease(dict as *const c_void) };

    // Sidecar surfaces as a "Virtual" transport whose product name is
    // the iPad — only then do we claim `Tablet`; a bare virtual
    // transport stays `Virtual`, never a guessed tablet.
    if product.contains("ipad") || product.contains("sidecar") {
        return MonitorKind::Tablet;
    }
    if name.contains("virtual") || product.contains("virtual") {
        return MonitorKind::Virtual;
    }
    if name.is_empty() && product.is_empty() {
        return MonitorKind::Unknown;
    }
    MonitorKind::External
}

/// `NSScreen.localizedName` for `id` — Sidecar iPads report their
/// device name ("iPad") there even when no IOKit transport exists.
pub(crate) fn ns_localized_name(id: u32) -> String {
    use std::ffi::{c_char, c_void};
    #[link(name = "objc", kind = "dylib")]
    unsafe extern "C" {
        fn objc_getClass(name: *const c_char) -> *mut c_void;
        fn sel_registerName(name: *const c_char) -> *mut c_void;
        fn objc_msgSend();
    }
    type MId = unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void;
    type MIdId = unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> *mut c_void;
    type MIdChar = unsafe extern "C" fn(*mut c_void, *mut c_void, *const c_char) -> *mut c_void;
    type MIdUsz = unsafe extern "C" fn(*mut c_void, *mut c_void, usize) -> *mut c_void;
    type MUsz = unsafe extern "C" fn(*mut c_void, *mut c_void) -> usize;
    type MU32 = unsafe extern "C" fn(*mut c_void, *mut c_void) -> u32;
    type MCChar = unsafe extern "C" fn(*mut c_void, *mut c_void) -> *const c_char;
    unsafe {
        let send_id: MId = std::mem::transmute(objc_msgSend as *const ());
        let send_id_id: MIdId = std::mem::transmute(objc_msgSend as *const ());
        let send_id_char: MIdChar = std::mem::transmute(objc_msgSend as *const ());
        let send_id_usz: MIdUsz = std::mem::transmute(objc_msgSend as *const ());
        let send_usz: MUsz = std::mem::transmute(objc_msgSend as *const ());
        let send_u32: MU32 = std::mem::transmute(objc_msgSend as *const ());
        let send_cchar: MCChar = std::mem::transmute(objc_msgSend as *const ());
        let screens = send_id(
            objc_getClass(c"NSScreen".as_ptr()),
            sel_registerName(c"screens".as_ptr()),
        );
        if screens.is_null() {
            return String::new();
        }
        let key = send_id_char(
            objc_getClass(c"NSString".as_ptr()),
            sel_registerName(c"stringWithUTF8String:".as_ptr()),
            c"NSScreenNumber".as_ptr(),
        );
        let count = send_usz(screens, sel_registerName(c"count".as_ptr()));
        for i in 0..count {
            let s = send_id_usz(screens, sel_registerName(c"objectAtIndex:".as_ptr()), i);
            if s.is_null() {
                continue;
            }
            let dd = send_id(s, sel_registerName(c"deviceDescription".as_ptr()));
            if dd.is_null() {
                continue;
            }
            let num = send_id_id(dd, sel_registerName(c"objectForKey:".as_ptr()), key);
            if num.is_null() || send_u32(num, sel_registerName(c"unsignedIntValue".as_ptr())) != id
            {
                continue;
            }
            let n = send_id(s, sel_registerName(c"localizedName".as_ptr()));
            if !n.is_null() {
                let p = send_cchar(n, sel_registerName(c"UTF8String".as_ptr()));
                if !p.is_null() {
                    return std::ffi::CStr::from_ptr(p).to_string_lossy().to_string();
                }
            }
        }
        String::new()
    }
}

/// A Dock-icon click queues `OpenViewer` — the arm un-minimizes and
/// focuses the viewer without moving it. `applicationShouldHandleReopen:
/// hasVisibleWindows:` is added on the app delegate when nothing answers
/// it, or swizzled onto winit's implementation (which still runs first).
fn install_dock_reopen_hook(ctx: egui::Context) {
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static ORIG: AtomicUsize = AtomicUsize::new(0);
    // The C callback can't capture: the egui context is parked in a
    // static so the queued OpenViewer also wakes the pump.
    static DOCK_CTX: std::sync::Mutex<Option<egui::Context>> = std::sync::Mutex::new(None);
    *DOCK_CTX.lock().unwrap() = Some(ctx);

    unsafe extern "C" fn on_reopen(
        this: *mut c_void,
        sel: *mut c_void,
        sender: *mut c_void,
        has_visible: bool,
    ) -> bool {
        let orig = ORIG.load(Ordering::Relaxed) as *const c_void;
        let result = if orig.is_null() {
            true
        } else {
            type Orig = unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void, bool) -> bool;
            unsafe {
                let f: Orig = std::mem::transmute(orig);
                f(this, sel, sender, has_visible)
            }
        };
        // Queue, don't act — the delegate callback runs mid-event and
        // the command is pumped on the next pass, which we wake now.
        super::queue_menu_command(crate::app::Command::OpenViewer);
        if let Some(ctx) = DOCK_CTX.lock().unwrap().as_ref() {
            crate::app::wake_viewports(ctx);
        }
        result
    }

    #[link(name = "objc", kind = "dylib")]
    unsafe extern "C" {
        fn objc_getClass(name: *const std::ffi::c_char) -> *mut c_void;
        fn sel_registerName(name: *const std::ffi::c_char) -> *mut c_void;
        fn objc_msgSend();
        fn object_getClass(obj: *mut c_void) -> *mut c_void;
        fn class_addMethod(
            cls: *mut c_void,
            sel: *mut c_void,
            imp: *const c_void,
            types: *const std::ffi::c_char,
        ) -> bool;
        fn class_getInstanceMethod(cls: *mut c_void, sel: *mut c_void) -> *mut c_void;
        fn method_getImplementation(m: *mut c_void) -> *const c_void;
        fn method_setImplementation(m: *mut c_void, imp: *const c_void) -> *const c_void;
    }
    type MsgSendId = unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void;
    type MsgSendBoolSel = unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> bool;

    unsafe {
        let send_id: MsgSendId = std::mem::transmute(objc_msgSend as *const ());
        let send_bool_sel: MsgSendBoolSel = std::mem::transmute(objc_msgSend as *const ());
        let app = send_id(
            objc_getClass(c"NSApplication".as_ptr()),
            sel_registerName(c"sharedApplication".as_ptr()),
        );
        if app.is_null() {
            return;
        }
        let delegate = send_id(app, sel_registerName(c"delegate".as_ptr()));
        if delegate.is_null() {
            return;
        }
        let sel = sel_registerName(c"applicationShouldHandleReopen:hasVisibleWindows:".as_ptr());
        if send_bool_sel(
            delegate,
            sel_registerName(c"respondsToSelector:".as_ptr()),
            sel,
        ) {
            // winit implements it (it turns the call into an event eframe
            // ignores): wrap — the original still runs first.
            let m = class_getInstanceMethod(object_getClass(delegate), sel);
            ORIG.store(method_getImplementation(m) as usize, Ordering::Relaxed);
            method_setImplementation(m, on_reopen as *const c_void);
        } else {
            // No existing handler: ours is the whole answer (YES = the
            // app may reopen — the queued OpenViewer does the real work).
            class_addMethod(
                object_getClass(delegate),
                sel,
                on_reopen as *const c_void,
                c"B@:@B".as_ptr(),
            );
        }
    }
}
