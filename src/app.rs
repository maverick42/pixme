//! Application root: global state and the eframe event loop.
//!
//! `PixMeApp` owns the persisted [`config::Settings`] and applies them live:
//! the UI language (`i18n`) and the active palette (`ui::theme`). Phase 2
//! replaces the placeholder central panel with the floating capture toolbar.

use eframe::egui;
use rust_i18n::t;

use crate::config::Settings;
use crate::{i18n, ui};

/// Root application state, created once in [`eframe::App::update`]'s setup.
pub struct PixMeApp {
    /// Persisted user settings (language, appearance).
    settings: Settings,
}

impl PixMeApp {
    /// Loads settings, installs fonts and image loaders, applies language and theme.
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let settings = Settings::load();
        i18n::apply(settings.language);
        ui::install(&cc.egui_ctx);
        ui::theme::apply(
            &cc.egui_ctx,
            settings.appearance.kind(cc.egui_ctx.system_theme()),
        );
        Self { settings }
    }
}

impl eframe::App for PixMeApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // Theme is reapplied every frame so a hot `appearance` change or an OS
        // theme switch under `Auto` takes effect immediately.
        ui::theme::apply(&ctx, self.settings.appearance.kind(ctx.system_theme()));

        egui::CentralPanel::default().show(ui, |ui| {
            ui.centered_and_justified(|ui| {
                ui.vertical_centered(|ui| {
                    ui.heading(t!("app.title"));
                    ui.label(t!("app.placeholder"));
                });
            });
        });
    }
}

/// Runs the app window. Phase 2 turns this into a borderless always-on-top
/// floating toolbar; for now it is a regular decorated window.
pub fn run() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(t!("app.title"))
            .with_inner_size([420.0, 160.0]),
        ..Default::default()
    };
    eframe::run_native(
        "pixme",
        options,
        Box::new(|cc| Ok(Box::new(PixMeApp::new(cc)))),
    )
}
