//! Application root: global state and the eframe event loop.
//!
//! `PixMeApp` owns the persisted [`config::Settings`] and applies them live:
//! the UI language (`i18n`) and the active palette (`ui::theme`). Its single
//! window is the floating capture [`ui::toolbar::Toolbar`].

use eframe::egui;

use crate::config::Settings;
use crate::ui::toolbar::Toolbar;
use crate::{i18n, ui};

/// Root application state, created once in [`eframe::App::ui`]'s setup.
pub struct PixMeApp {
    /// Persisted user settings (language, appearance).
    settings: Settings,
    /// The floating toolbar's transient state.
    toolbar: Toolbar,
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
        Self {
            settings,
            toolbar: Toolbar::default(),
        }
    }
}

impl eframe::App for PixMeApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // Theme is reapplied every frame so a hot `appearance` change or an OS
        // theme switch under `Auto` takes effect immediately.
        ui::theme::apply(&ctx, self.settings.appearance.kind(ctx.system_theme()));
        self.toolbar.show(ui, &mut self.settings);
    }
}

/// Runs the app as a borderless, transparent, always-on-top floating bar.
pub fn run() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("PixMe")
            .with_decorations(false)
            .with_transparent(true)
            .with_resizable(false)
            .with_always_on_top()
            .with_has_shadow(false)
            .with_inner_size([240.0, 48.0]),
        ..Default::default()
    };
    eframe::run_native(
        "pixme",
        options,
        Box::new(|cc| Ok(Box::new(PixMeApp::new(cc)))),
    )
}
