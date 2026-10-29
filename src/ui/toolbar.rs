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

/// Height reserved below the bar while one of its menus is open: egui
/// constrains popup `Area`s to this viewport's rect, so a menu taller than
/// the bar would be clipped without the extra room (the window is
/// transparent there — nothing shows).
const MENU_RESERVE: f32 = 360.0;

/// Initial vertical position of the bar, as a fraction of the monitor height
/// (sits near the top, below a menu bar, without clinging to the edge).
const TOP_MARGIN: f32 = 0.12;

/// State of the floating toolbar window.
#[derive(Debug, Default)]
pub struct Toolbar {
    /// Monitor the bar should be top-centred on once its content size is
    /// known (`ShowBar` sets this to the monitor under the cursor).
    target_monitor: Option<crate::capture::MonitorInfo>,
}

impl Toolbar {
    /// Schedules the bar to move to the top-centre of `monitor` on its
    /// next frame.
    pub fn place_on_monitor(&mut self, monitor: crate::capture::MonitorInfo) {
        self.target_monitor = Some(monitor);
    }

    /// Draws the toolbar for this frame. Actions are pushed as [`Command`]s
    /// on `shared` — the state machine dispatches them.
    pub fn show(&mut self, ui: &mut egui::Ui, settings: &mut Settings, shared: &Shared) {
        let ctx = ui.ctx().clone();
        CentralPanel::default()
            .frame(Frame::NONE.inner_margin(Margin::same(SHADOW_MARGIN)))
            .show(ui, |ui| {
                let content = self.capture_bar(ui, settings, shared);
                // The painted card's on-screen bounds (not the viewport's
                // `outer_rect`, which is 360 pt taller while a menu is
                // open): the countdown card anchors just below this.
                if let Some(win) = ctx.input(|i| i.viewport().outer_rect.map(|r| r.min)) {
                    let card = content.response.rect;
                    *crate::app::lock(&shared.bar_rect) =
                        Some(Rect::from_min_size(win + card.min.to_vec2(), card.size()));
                }
                let mut rect = content.response.rect;
                if content.inner {
                    rect.max.y += MENU_RESERVE;
                }
                self.sync_viewport(&ctx, rect);
            });
    }

    /// One row of capture-mode icon buttons, delay, options and quit.
    /// Returns `true` in `inner` while one of the row's menus is open —
    /// the caller then reserves room for it under the bar.
    fn capture_bar(
        &mut self,
        ui: &mut egui::Ui,
        settings: &mut Settings,
        shared: &Shared,
    ) -> egui::InnerResponse<bool> {
        let t = theme::tokens(ui.ctx());
        let frame = widgets::card_frame(t).shadow(widgets::card_shadow(t));
        let inner = frame.show(ui, |ui| {
            ui.horizontal(|ui| -> bool {
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
                // A modal window (save-as opened from a pin, etc.) can be
                // up while the bar is visible: grey out every command it
                // forbids, keep the close X live (`CloseBar` always runs).
                let menu_open = ui
                    .add_enabled_ui(!shared.modal_child_open(), |ui| {
                        for (icon, tip, mode) in [
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
                                shared.push_capture(mode, ui.ctx());
                            }
                        }
                        widgets::vline(ui, 18.0);
                        // Accent border + soft fill while a delay is
                        // armed — the "small blue frame" marker.
                        let delay_btn = cap_button(
                            ui,
                            Icon::Clock,
                            "toolbar.delay.tooltip",
                            settings.capture_delay_secs > 0,
                        );
                        Self::delay_menu(&delay_btn, settings);
                        widgets::vline(ui, 18.0);
                        let opts_btn =
                            cap_button(ui, Icon::Settings, "toolbar.options.tooltip", false);
                        Self::options_menu(&opts_btn, settings);
                        widgets::vline(ui, 18.0);
                        popup_open(ui, &delay_btn) || popup_open(ui, &opts_btn)
                    })
                    .inner;
                if cap_button(ui, Icon::X, "toolbar.close.tooltip", false).clicked() {
                    shared.push(Command::CloseBar);
                }
                menu_open
            })
            .inner
        });
        if inner.response.drag_started() {
            ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
        }
        inner
    }

    /// Dropdown on the clock button: delay in seconds before a capture.
    /// The chosen delay is session state — it survives between captures
    /// but is never saved to disk.
    fn delay_menu(btn: &egui::Response, settings: &mut Settings) {
        egui::Popup::menu(btn).show(|ui| {
            ui.selectable_value(&mut settings.capture_delay_secs, 0, t!("delay.none"));
            // A quick 5-second pick, then the full 1–9 s range below a
            // separator.
            ui.selectable_value(
                &mut settings.capture_delay_secs,
                5,
                t!("delay.seconds", count = 5),
            );
            ui.separator();
            for secs in 1..=9u32 {
                let key = if secs == 1 {
                    "delay.second"
                } else {
                    "delay.seconds"
                };
                ui.selectable_value(
                    &mut settings.capture_delay_secs,
                    secs,
                    t!(key, count = secs),
                );
            }
        });
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

    /// Keeps the viewport hugging its content, and when `ShowBar` set a
    /// `target_monitor`, moves it to the top-centre of that monitor once
    /// the content size is known — consumed immediately so a user drag is
    /// never overridden afterwards.
    fn sync_viewport(&mut self, ctx: &egui::Context, content: Rect) {
        let target = content.size() + Vec2::splat(2.0 * f32::from(SHADOW_MARGIN));
        let current = ctx.input(|i| i.viewport().inner_rect.map_or(Vec2::ZERO, |r| r.size()));
        if (target - current).abs().max_elem() > 1.0 {
            ctx.send_viewport_cmd(ViewportCommand::InnerSize(target));
        }
        if let Some(m) = self.target_monitor.take() {
            let scale = m.scale_factor.max(1.0);
            let origin =
                crate::platform::native_to_ui(egui::pos2(m.rect.x as f32, m.rect.y as f32), scale);
            let size = crate::platform::native_to_ui(
                egui::pos2(m.rect.width as f32, m.rect.height as f32),
                scale,
            )
            .to_vec2();
            let pos = egui::pos2(
                origin.x + (size.x - target.x) / 2.0,
                origin.y + size.y * TOP_MARGIN,
            );
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

/// Whether the [`egui::Popup::menu`] anchored on `btn` is currently open —
/// egui derives the popup id from the anchor response id.
fn popup_open(ui: &egui::Ui, btn: &egui::Response) -> bool {
    egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(btn))
}
