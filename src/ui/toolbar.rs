//! The floating mini toolbar — PixMe's main window.
//!
//! A borderless, transparent, always-on-top viewport sized to its content,
//! draggable anywhere, drawn as themed [`widgets::card_frame`] surfaces:
//! capture buttons on one row, and a settings card revealed underneath.

use egui::{CentralPanel, Frame, Margin, Rect, Vec2, ViewportCommand};
use rust_i18n::t;

use crate::config::{Appearance, Settings};
use crate::i18n::{self, Language};
use crate::ui::icons::Icon;
use crate::ui::{theme, widgets};

/// Transparent margin kept around the bar so its painted shadow is not
/// clipped by the viewport bounds.
const SHADOW_MARGIN: i8 = 14;

/// Gap between the bar and the settings card.
const CARDS_GAP: f32 = 4.0;

/// Initial vertical position of the bar, as a fraction of the monitor height
/// (sits near the top, below a menu bar, without clinging to the edge).
const TOP_MARGIN: f32 = 0.12;

/// State of the floating toolbar window.
#[derive(Debug, Default)]
pub struct Toolbar {
    /// Whether the settings card is expanded below the bar.
    settings_open: bool,
    /// Whether the window has already been moved to its initial position.
    positioned: bool,
}

impl Toolbar {
    /// Draws the toolbar (and the settings card when open) for this frame.
    pub fn show(&mut self, ui: &mut egui::Ui, settings: &mut Settings) {
        let ctx = ui.ctx().clone();
        CentralPanel::default()
            .frame(Frame::NONE.inner_margin(Margin::same(SHADOW_MARGIN)))
            .show(ui, |ui| {
                let content = ui.vertical(|ui| {
                    self.capture_bar(ui);
                    if self.settings_open {
                        ui.add_space(CARDS_GAP);
                        self.settings_card(ui, settings);
                    }
                });
                self.sync_viewport(&ctx, content.response.rect);
            });
    }

    /// One row of capture icon buttons, settings and quit.
    fn capture_bar(&mut self, ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        let open = self.settings_open;
        let frame = widgets::card_frame(t).shadow(widgets::card_shadow(t));
        let inner = frame.show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(2.0, 0.0);
                if cap_button(ui, Icon::Monitor, "toolbar.fullscreen.tooltip", false).clicked() {
                    tracing::info!(kind = "fullscreen", "capture requested");
                }
                if cap_button(ui, Icon::SquareDashed, "toolbar.region.tooltip", false).clicked() {
                    tracing::info!(kind = "region", "capture requested");
                }
                if cap_button(ui, Icon::AppWindow, "toolbar.window.tooltip", false).clicked() {
                    tracing::info!(kind = "window", "capture requested");
                }
                widgets::vline(ui, 18.0);
                self.settings_open ^=
                    cap_button(ui, Icon::Settings, "toolbar.settings.tooltip", open).clicked();
                widgets::vline(ui, 18.0);
                if cap_button(ui, Icon::X, "toolbar.quit.tooltip", false).clicked() {
                    ui.ctx().send_viewport_cmd(ViewportCommand::Close);
                }
            });
        });
        if inner.response.drag_started() {
            ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
        }
    }

    /// Language and appearance pickers, applied hot and persisted.
    fn settings_card(&mut self, ui: &mut egui::Ui, settings: &mut Settings) {
        let prev = (settings.language, settings.appearance);
        let t = theme::tokens(ui.ctx());
        widgets::card_frame(t)
            .inner_margin(Margin::same(8))
            .show(ui, |ui| {
                egui::Grid::new(ui.id().with("settings-grid"))
                    .num_columns(2)
                    .spacing([10.0, 6.0])
                    .show(ui, |ui| {
                        ui.label(t!("settings.language.label"));
                        let btn = widgets::dropdown_button(ui, &language_label(settings.language));
                        egui::Popup::menu(&btn).show(|ui| {
                            ui.selectable_value(
                                &mut settings.language,
                                Language::Auto,
                                t!("settings.language.auto"),
                            );
                            for lang in Language::ALL {
                                ui.selectable_value(
                                    &mut settings.language,
                                    lang,
                                    lang.native_name(),
                                );
                            }
                        });
                        ui.end_row();

                        ui.label(t!("settings.appearance.label"));
                        let btn =
                            widgets::dropdown_button(ui, &appearance_label(settings.appearance));
                        egui::Popup::menu(&btn).show(|ui| {
                            for (value, key) in [
                                (Appearance::Auto, "settings.appearance.auto"),
                                (Appearance::Dark, "settings.appearance.dark"),
                                (Appearance::Light, "settings.appearance.light"),
                            ] {
                                ui.selectable_value(&mut settings.appearance, value, t!(key));
                            }
                        });
                        ui.end_row();
                    });
            });

        if (settings.language, settings.appearance) != prev {
            i18n::apply(settings.language);
            settings.save();
            ui.ctx()
                .send_viewport_cmd(ViewportCommand::Title(t!("app.title").into_owned()));
        }
    }

    /// Keeps the viewport hugging its content, and on the first frame with
    /// known monitor geometry moves it to the top-center of the screen —
    /// exactly once, so a user drag is never overridden afterwards.
    fn sync_viewport(&mut self, ctx: &egui::Context, content: Rect) {
        let target = content.size() + Vec2::splat(2.0 * f32::from(SHADOW_MARGIN));
        let current = ctx.input(|i| i.viewport().inner_rect.map_or(Vec2::ZERO, |r| r.size()));
        if (target - current).abs().max_elem() > 1.0 {
            ctx.send_viewport_cmd(ViewportCommand::InnerSize(target));
        }
        if !self.positioned
            && let Some(monitor) = ctx.input(|i| i.viewport().monitor_size)
        {
            self.positioned = true;
            let pos = egui::pos2((monitor.x - target.x) / 2.0, monitor.y * TOP_MARGIN);
            ctx.send_viewport_cmd(ViewportCommand::OuterPosition(pos));
        }
    }
}

/// One capture icon button with its translated tooltip; starting a drag on it
/// moves the whole window.
fn cap_button(ui: &mut egui::Ui, icon: Icon, tip: &'static str, selected: bool) -> egui::Response {
    let resp = widgets::icon_button(ui, icon, selected).on_hover_text(t!(tip));
    if resp.drag_started() {
        ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
    }
    resp
}

/// Current language as displayed in the dropdown: "Auto" is translated,
/// explicit languages name themselves.
fn language_label(language: Language) -> String {
    match language {
        Language::Auto => t!("settings.language.auto").into_owned(),
        lang => lang.native_name().to_owned(),
    }
}

fn appearance_label(appearance: Appearance) -> String {
    let key = match appearance {
        Appearance::Auto => "settings.appearance.auto",
        Appearance::Dark => "settings.appearance.dark",
        Appearance::Light => "settings.appearance.light",
    };
    t!(key).into_owned()
}
