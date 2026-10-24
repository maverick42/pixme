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
use crate::ui::icons::{self, Icon};

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
    options.viewport = options.viewport.clone().with_taskbar(true);
}

/// Tray icon edge in pixels (16 px slot × ~2 for high-DPI trays).
const TRAY_ICON_PX: u32 = 32;

/// Menu handles kept alive for the app lifetime, plus the i18n keys needed
/// to relabel them when the language changes.
struct Installed {
    _tray: Option<TrayIcon>,
    _menu: Menu,
    items: Vec<(MenuItem, &'static str)>,
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
        items.push((item, key));
    }

    let mut builder = TrayIcon::builder()
        .with_menu(Box::new(menu.clone()))
        .with_menu_on_left_click(true)
        .with_tooltip("PixMe");
    // The Windows tray has no template mode: draw the glyph in the accent
    // colour so it stays visible on light and dark taskbars.
    match tray_icon::Icon::from_rgba(
        icons::rgba_with(Icon::Scan, TRAY_ICON_PX, "#378EF0"),
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

/// Rewrites the tray menu labels in the current language.
pub fn relabel_menus() {
    INSTALLED.with(|cell| {
        if let Some(installed) = &*cell.borrow() {
            for (item, key) in &installed.items {
                item.set_text(t!(*key).as_ref());
            }
        }
    });
}
