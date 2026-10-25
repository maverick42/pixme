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
use crate::ui::icons::{self, Icon};

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
/// its windows is focused — accessory apps can never own it. Nothing to
/// tweak at event-loop creation.
pub fn apply_native_options(_options: &mut eframe::NativeOptions) {}

/// Status-item icon edge in pixels (22 pt menu-bar slot × 2 for Retina).
const STATUS_ICON_PX: u32 = 44;

/// Menu handles kept alive for the app lifetime, plus the i18n keys needed
/// to relabel them when the language changes.
#[derive(Default)]
struct Labels {
    items: Vec<(MenuItem, &'static str)>,
    predef: Vec<(PredefinedMenuItem, &'static str)>,
    subs: Vec<(Submenu, &'static str)>,
}

impl Labels {
    fn relabel(&self) {
        for (item, key) in &self.items {
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
    labels.items.push((item, key));
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

/// The menu-bar status item with its template (monochrome) icon.
fn install_status_item(menu: Menu) -> Option<TrayIcon> {
    let mut builder = TrayIcon::builder()
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(true)
        .with_tooltip("PixMe");
    match tray_icon::Icon::from_rgba(
        icons::rgba(Icon::Scan, STATUS_ICON_PX),
        STATUS_ICON_PX,
        STATUS_ICON_PX,
    ) {
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
