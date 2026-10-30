//! macOS implementation: menu-bar status item, native application menus and
//! the accessory (no-Dock-icon) activation policy.
//!
//! The status item exposes the same two actions as the Windows tray icon —
//! "Take a capture" and "Open PixMe". The real application menus (PixMe /
//! Capture / File / Window) live on the menu bar, and quitting is only
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
use tray_icon::TrayIcon;
use tray_icon::menu::accelerator::{Accelerator, Code, Modifiers};
use tray_icon::menu::{IsMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};

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
            // The egui windows may be hidden: wake the loop so pump() runs.
            repaint_ctx.request_repaint();
        }
    }));

    let mut labels = Labels::default();

    // Status item: the same two actions the Windows tray menu carries.
    // Quitting lives exclusively in the PixMe application menu.
    let status_menu = Menu::new();
    item(
        &mut labels,
        &status_menu,
        "take_capture",
        "menubar.take_capture",
        None,
    );
    item(
        &mut labels,
        &status_menu,
        "open_viewer",
        "menubar.open_viewer",
        None,
    );
    let tray = install_status_item(status_menu);

    // Application menu bar (system menu bar, always present).
    let app_menu = Menu::new();
    let app = Submenu::new("PixMe", true);
    let about = PredefinedMenuItem::about(Some(t!("menu.app.about").as_ref()), None);
    append(&app, &about);
    labels.predef.push((about, "menu.app.about"));
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
        "import_clipboard",
        "menu.app.import_clipboard",
        Some(Accelerator::new(
            Modifiers::META | Modifiers::ALT,
            Code::KeyV,
        )),
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

    let capture = Submenu::new(t!("menu.capture").as_ref(), true);
    labels.subs.push((capture.clone(), "menu.capture"));
    for (id, key) in [
        ("capture.window", "toolbar.window.tooltip"),
        ("capture.region", "toolbar.region.tooltip"),
        ("capture.fullscreen", "toolbar.fullscreen.tooltip"),
        ("capture.fixed_region", "toolbar.fixed_region.tooltip"),
        ("capture.pin", "toolbar.pin.tooltip"),
    ] {
        item(&mut labels, &capture, id, key, None);
    }
    append(&app_menu, &capture);

    let file = Submenu::new(t!("menu.file").as_ref(), true);
    labels.subs.push((file.clone(), "menu.file"));
    item(
        &mut labels,
        &file,
        "file.open",
        "menu.file.open",
        Some(Accelerator::new(Modifiers::META, Code::KeyO)),
    );
    separator(&file);
    item(
        &mut labels,
        &file,
        "file.save",
        "menu.file.save",
        Some(Accelerator::new(Modifiers::META, Code::KeyS)),
    );
    item(
        &mut labels,
        &file,
        "file.save_all",
        "menu.file.save_all",
        Some(Accelerator::new(
            Modifiers::META | Modifiers::SHIFT,
            Code::KeyS,
        )),
    );
    separator(&file);
    item(&mut labels, &file, "file.rename", "menu.file.rename", None);
    separator(&file);
    item(
        &mut labels,
        &file,
        "file.print",
        "menu.file.print",
        Some(Accelerator::new(Modifiers::META, Code::KeyP)),
    );
    append(&app_menu, &file);

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
pub fn sync_menu_enabled(modal: crate::app::ModalState) {
    INSTALLED.with(|cell| {
        if let Some(installed) = &*cell.borrow() {
            for (item, _key, id) in &installed.labels.items {
                if let Some(command) = super::command_from_menu_id(id) {
                    item.set_enabled(crate::app::is_command_enabled(&command, modal));
                }
            }
        }
    });
}

/// The menu-bar status item with its template (monochrome) icon.
fn install_status_item(menu: Menu) -> Option<TrayIcon> {
    let mut builder = TrayIcon::builder()
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(true)
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

/// Prints `path` on the default printer via `lp` (CUPS scales the image
/// to the page). Returns `Err` when the job could not be queued.
pub fn print_file(path: &std::path::Path) -> Result<(), String> {
    std::process::Command::new("lp")
        .arg(path)
        .status()
        .map_err(|e| e.to_string())
        .and_then(|s| {
            if s.success() {
                Ok(())
            } else {
                Err(format!("lp exited with {s}"))
            }
        })
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
        // or right after the bar tore down): both "current" cursors are
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
pub fn dismiss_active_menus() {
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

    type MsgSendId = unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void;
    type MsgSendVoidBool = unsafe extern "C" fn(*mut c_void, *mut c_void, bool);

    const ESCAPE_KEYCODE: u16 = 53;
    const POPUP_MENU_LAYER: i32 = 100;
    const HID_EVENT_TAP: u32 = 0;

    let own_pid = std::process::id();
    let mut menu_pids: Vec<u32> = window_list()
        .into_iter()
        .filter(|w| w.pid != own_pid && w.layer >= POPUP_MENU_LAYER)
        .map(|w| w.pid)
        .collect();
    menu_pids.sort_unstable();
    menu_pids.dedup();

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
            }
            if !down.is_null() {
                CFRelease(down.cast());
            }
            if !up.is_null() {
                CFRelease(up.cast());
            }
        }
    }

    unsafe {
        let send_id: MsgSendId = std::mem::transmute(objc_msgSend as *const ());
        let send_void_bool: MsgSendVoidBool = std::mem::transmute(objc_msgSend as *const ());
        let cls = objc_getClass(c"NSApplication".as_ptr());
        if !cls.is_null() {
            let app = send_id(cls, sel_registerName(c"sharedApplication".as_ptr()));
            if !app.is_null() {
                send_void_bool(
                    app,
                    sel_registerName(c"activateIgnoringOtherApps:".as_ptr()),
                    true,
                );
            }
        }
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
    type MsgSendVoidBool = unsafe extern "C" fn(*mut c_void, *mut c_void, bool);

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
        let send_void_bool: MsgSendVoidBool = std::mem::transmute(objc_msgSend as *const ());

        let cls = objc_getClass(c"NSApplication".as_ptr());
        if cls.is_null() {
            return;
        }
        let app = send_id(cls, sel_registerName(c"sharedApplication".as_ptr()));
        if app.is_null() {
            return;
        }
        send_void_bool(
            app,
            sel_registerName(c"activateIgnoringOtherApps:".as_ptr()),
            true,
        );
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
