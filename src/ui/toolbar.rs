//! The floating mini toolbar — PixMe's capture window.
//!
//! A borderless, transparent, always-on-top viewport sized to its content,
//! draggable anywhere, drawn as a themed [`widgets::card_frame`] surface:
//! one row of capture-mode buttons plus delay, capture options and quit.
//! It only exposes capture-related controls — application settings such as
//! language and appearance belong to the viewer window's settings UI.

use egui::{CentralPanel, Frame, Margin, Rect, Vec2, ViewportCommand};
use rust_i18n::t;

use crate::app::{CaptureMode, Command, Shared};
use crate::config::Settings;
use crate::ui::icons::{self, Icon};
use crate::ui::{theme, widgets};

/// Transparent margin kept around the bar so its painted shadow is not
/// clipped by the viewport bounds.
const SHADOW_MARGIN: i8 = 14;

/// Initial vertical position of the bar, as a fraction of the monitor height
/// (sits near the top, below a menu bar, without clinging to the edge).
const TOP_MARGIN: f32 = 0.12;

/// State of the floating toolbar window.
#[derive(Debug, Default)]
pub struct Toolbar {
    /// Whether the window has already been moved to its initial position.
    positioned: bool,
}

impl Toolbar {
    /// Draws the toolbar for this frame. Actions are pushed as [`Command`]s
    /// on `shared` — the state machine dispatches them.
    pub fn show(&mut self, ui: &mut egui::Ui, settings: &mut Settings, shared: &Shared) {
        let ctx = ui.ctx().clone();
        CentralPanel::default()
            .frame(Frame::NONE.inner_margin(Margin::same(SHADOW_MARGIN)))
            .show(ui, |ui| {
                let content = self.capture_bar(ui, settings, shared);
                self.sync_viewport(&ctx, content.response.rect);
            });
    }

    /// One row of capture-mode icon buttons, delay, options and quit.
    fn capture_bar(
        &mut self,
        ui: &mut egui::Ui,
        settings: &mut Settings,
        shared: &Shared,
    ) -> egui::InnerResponse<()> {
        let t = theme::tokens(ui.ctx());
        let frame = widgets::card_frame(t).shadow(widgets::card_shadow(t));
        let inner = frame.show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(2.0, 0.0);
                // Drag handle (functional reference's dotted grip): an
                // explicit grab affordance so the bar reads as movable.
                let (grip_rect, grip_resp) =
                    ui.allocate_exact_size(Vec2::new(10.0, 26.0), egui::Sense::drag());
                icons::paint(ui, grip_rect, Icon::GripVertical, 12.0, t.text_faint);
                if grip_resp.drag_started() {
                    ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
                }
                grip_resp.on_hover_cursor(egui::CursorIcon::Grab);
                for (icon, tip, mode) in [
                    (
                        Icon::AppWindow,
                        "toolbar.active_window.tooltip",
                        CaptureMode::ActiveWindow,
                    ),
                    (
                        Icon::SquareDashedMousePointer,
                        "toolbar.window.tooltip",
                        CaptureMode::WindowOrObject,
                    ),
                    (
                        Icon::SquareDashed,
                        "toolbar.region.tooltip",
                        CaptureMode::Region,
                    ),
                    (
                        Icon::Monitor,
                        "toolbar.fullscreen.tooltip",
                        CaptureMode::FullScreen,
                    ),
                    (
                        Icon::Crop,
                        "toolbar.fixed_region.tooltip",
                        CaptureMode::FixedRegion,
                    ),
                    (Icon::Pin, "toolbar.pin.tooltip", CaptureMode::Pin),
                ] {
                    if cap_button(ui, icon, tip, false).clicked() {
                        shared.push(Command::Capture(mode));
                    }
                }
                widgets::vline(ui, 18.0);
                let delay_btn = cap_button(ui, Icon::Clock, "toolbar.delay.tooltip", false);
                Self::delay_menu(&delay_btn, settings);
                widgets::vline(ui, 18.0);
                let opts_btn = cap_button(ui, Icon::Settings, "toolbar.options.tooltip", false);
                Self::options_menu(&opts_btn, settings);
                widgets::vline(ui, 18.0);
                if cap_button(ui, Icon::X, "toolbar.close.tooltip", false).clicked() {
                    shared.push(Command::CloseBar);
                }
            });
        });
        if inner.response.drag_started() {
            ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
        }
        inner
    }

    /// Dropdown on the clock button: delay in seconds before a capture, plus
    /// the auto-reset-to-zero toggle. Changes persist immediately.
    fn delay_menu(btn: &egui::Response, settings: &mut Settings) {
        let mut changed = false;
        egui::Popup::menu(btn).show(|ui| {
            changed |= ui
                .selectable_value(&mut settings.capture_delay_secs, 0, t!("delay.none"))
                .changed();
            ui.separator();
            for secs in 1..=9u32 {
                let key = if secs == 1 {
                    "delay.second"
                } else {
                    "delay.seconds"
                };
                changed |= ui
                    .selectable_value(
                        &mut settings.capture_delay_secs,
                        secs,
                        t!(key, count = secs),
                    )
                    .changed();
            }
            ui.separator();
            changed |= ui
                .checkbox(&mut settings.delay_auto_reset, t!("delay.auto_reset"))
                .changed();
        });
        if changed {
            settings.save();
        }
    }

    /// Menu on the gear button: capture-related toggles only — application
    /// settings live in the viewer menus / app menu, never in the bar.
    fn options_menu(btn: &egui::Response, settings: &mut Settings) {
        let mut changed = false;
        egui::Popup::menu(btn).show(|ui| {
            changed |= ui
                .checkbox(&mut settings.include_pointer, t!("options.include_pointer"))
                .changed();
            changed |= ui
                .checkbox(
                    &mut settings.copy_to_clipboard,
                    t!("options.copy_to_clipboard"),
                )
                .changed();
        });
        if changed {
            settings.save();
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
