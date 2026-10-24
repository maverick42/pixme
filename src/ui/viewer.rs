//! The session viewer — PixMe's main window.
//!
//! One tab per capture of the session, a "+" action that hands back to the
//! floating bar for a new capture, and the translated Save/Discard/Cancel
//! confirmation when a dirty session would be lost. On Windows the viewer
//! also draws the application's menu strip (no global menu bar exists);
//! on macOS the same actions live in the native PixMe/Capture/File menus.
//! The document canvas and its View/Edit modes land in the next phase.

use egui::{Align2, CentralPanel, Margin, Vec2, ViewportCommand};
use rust_i18n::t;

use crate::app::{CaptureMode, Command, Shared};
use crate::ui::icons::Icon;
use crate::ui::{theme, widgets};

/// Session state rendered inside the viewer viewport.
#[derive(Debug, Default)]
pub struct Viewer {
    /// One title per captured document (filled by the capture pipeline).
    pub tabs: Vec<String>,
    /// Index of the visible tab.
    pub active: usize,
    /// Whether the session holds unsaved captures or annotations.
    pub dirty: bool,
    /// Show the Save/Discard/Cancel confirmation.
    pub confirm_close: bool,
    /// After the confirmation resolves, quit instead of going Idle.
    pub confirm_then_quit: bool,
}

impl Viewer {
    /// Draws the viewer for this frame. `shared` routes actions back to the
    /// state machine (the deferred-viewport callback cannot borrow the app).
    pub fn show(&mut self, ui: &mut egui::Ui, shared: &Shared) {
        let ctx = ui.ctx().clone();
        if ctx.input(|i| i.viewport().close_requested()) {
            // The state machine owns this window's lifetime: always cancel
            // and let dispatch() decide whether closing is allowed.
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            shared.push(Command::ViewerClosed);
        }
        CentralPanel::default()
            .frame(egui::Frame::default().inner_margin(Margin::ZERO))
            .show(ui, |ui| {
                if !shared.has_native_app_menu() {
                    self.menu_strip(ui, shared);
                    widgets::hairline(ui);
                }
                self.tab_strip(ui, shared);
                widgets::hairline(ui);
                self.content(ui);
            });
        self.confirm_dialog(&ctx, shared);
    }

    /// egui menu strip for OSes without a native menu bar (Windows). Same
    /// commands as the native menus — the only difference is where they
    /// render.
    fn menu_strip(&mut self, ui: &mut egui::Ui, shared: &Shared) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(2.0, 0.0);
            menu_button(ui, t!("menu.file").as_ref(), |ui| {
                menu_item(ui, t!("menu.file.open").as_ref(), || {
                    shared.push(Command::OpenFile)
                });
                menu_item(ui, t!("menu.app.import_clipboard").as_ref(), || {
                    shared.push(Command::ImportClipboard)
                });
                ui.separator();
                menu_item(ui, t!("menu.file.save").as_ref(), || {
                    shared.push(Command::SaveActive)
                });
                menu_item(ui, t!("menu.file.save_all").as_ref(), || {
                    shared.push(Command::SaveAll)
                });
                menu_item(ui, t!("menu.file.rename").as_ref(), || {
                    shared.push(Command::RenameActive)
                });
                ui.separator();
                menu_item(ui, t!("menu.app.quit").as_ref(), || {
                    shared.push(Command::Quit)
                });
            });
            menu_button(ui, t!("menu.capture").as_ref(), |ui| {
                for (tip, mode) in [
                    ("toolbar.active_window.tooltip", CaptureMode::ActiveWindow),
                    ("toolbar.window.tooltip", CaptureMode::WindowOrObject),
                    ("toolbar.region.tooltip", CaptureMode::Region),
                    ("toolbar.fullscreen.tooltip", CaptureMode::FullScreen),
                    ("toolbar.fixed_region.tooltip", CaptureMode::FixedRegion),
                    ("toolbar.pin.tooltip", CaptureMode::Pin),
                ] {
                    menu_item(ui, t!(tip).as_ref(), || shared.push(Command::Capture(mode)));
                }
            });
            menu_button(ui, t!("settings.title").as_ref(), |ui| {
                menu_item(ui, t!("menu.app.preferences").as_ref(), || {
                    shared.push(Command::OpenSettings)
                });
            });
        });
    }

    /// The capture tab strip: one selectable label per capture, then "+".
    fn tab_strip(&mut self, ui: &mut egui::Ui, shared: &Shared) {
        egui::Frame::default()
            .inner_margin(Margin::symmetric(8, 6))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(4.0, 0.0);
                    for index in 0..self.tabs.len() {
                        if ui
                            .selectable_label(index == self.active, &self.tabs[index])
                            .clicked()
                        {
                            self.active = index;
                        }
                    }
                    if widgets::icon_button(ui, Icon::Plus, false)
                        .on_hover_text(t!("viewer.new_capture.tooltip"))
                        .clicked()
                    {
                        shared.push(Command::ShowBar);
                    }
                });
            });
    }

    /// The document canvas — an empty-state hint until captures land.
    fn content(&mut self, ui: &mut egui::Ui) {
        if self.tabs.is_empty() {
            let t = theme::tokens(ui.ctx());
            ui.centered_and_justified(|ui| {
                ui.colored_label(t.text_dim, t!("viewer.empty").to_string());
            });
        }
    }

    /// The translated Save/Discard/Cancel confirmation for a dirty session.
    fn confirm_dialog(&mut self, ctx: &egui::Context, shared: &Shared) {
        if !self.confirm_close {
            return;
        }
        egui::Window::new(t!("dialog.unsaved.title").to_string())
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label(t!("dialog.unsaved.body").to_string());
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if widgets::primary_button(ui, &t!("dialog.unsaved.save")).clicked() {
                        shared.push(Command::ConfirmSave);
                    }
                    if widgets::secondary_button(ui, &t!("dialog.unsaved.discard")).clicked() {
                        shared.push(Command::ConfirmDiscard);
                    }
                    if widgets::secondary_button(ui, &t!("dialog.unsaved.cancel")).clicked() {
                        shared.push(Command::ConfirmCancel);
                    }
                });
            });
    }
}

/// One top-level menu label opening a popup (egui menu strip style).
fn menu_button(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui)) {
    let resp = ui.button(label);
    egui::Popup::menu(&resp).show(|ui| {
        ui.set_min_width(180.0);
        add(ui);
    });
}

/// One menu entry dispatching a [`Command`] when clicked.
fn menu_item(ui: &mut egui::Ui, label: &str, action: impl FnOnce()) {
    if ui.button(label).clicked() {
        action();
        ui.close();
    }
}
