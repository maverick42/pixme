//! The capture-delay countdown: a floating always-on-top card anchored
//! just below the floating capture bar, centred on it — it follows the
//! bar when the user drags it. Without a visible bar (capture launched
//! from a menu while the viewer is closed) it falls back to the centre
//! of the monitor under the cursor.

use std::time::{Duration, Instant};

use egui::{Align2, CentralPanel, Frame, Margin, Rect, Vec2, ViewportCommand};

use crate::app::CaptureMode;
use crate::platform;
use crate::ui::{theme, widgets};

/// Transparent margin kept around the card so its painted shadow is not
/// clipped by the viewport bounds (same trick as the floating bar).
const SHADOW_MARGIN: i8 = 14;

/// Gap between the floating bar's bottom edge and the card (its top
/// already carries `SHADOW_MARGIN` of transparent padding, so `0` puts
/// the painted card `SHADOW_MARGIN` pt below the bar).
const BAR_GAP: f32 = 0.0;

/// Side of the square card holding the digit.
const CARD_SIDE: f32 = 104.0;

/// Countdown digit size — readable across the screen.
const DIGIT_SIZE: f32 = 64.0;

/// Repaint cadence: refreshes the digit and keeps the card following the
/// bar without spinning.
const REPAINT: Duration = Duration::from_millis(120);

/// A live delayed-capture countdown, armed by `Command::Capture` when the
/// configured delay is non-zero; at zero the owner fires
/// `Command::CaptureNow`.
pub struct Countdown {
    /// The capture to fire at zero.
    pub mode: CaptureMode,
    /// When the delay ends.
    deadline: Instant,
    /// The second currently displayed — a tick sounds each time it
    /// changes, including the first shown digit.
    shown: Option<u32>,
}

impl Countdown {
    /// Arms a countdown of `secs` seconds for `mode`.
    pub fn new(mode: CaptureMode, secs: u32) -> Self {
        Self {
            mode,
            deadline: Instant::now() + Duration::from_secs(u64::from(secs)),
            shown: None,
        }
    }

    /// Whole seconds still to go (`0` at the deadline).
    fn remaining(&self) -> u32 {
        self.deadline
            .saturating_duration_since(Instant::now())
            .as_secs_f32()
            .ceil() as u32
    }

    /// Paints the countdown card for this frame and keeps the viewport
    /// hugging it, anchored under `bar` (the floating bar's outer rect —
    /// `None` when the bar is closed). Returns `false` once the deadline
    /// passed — the caller then fires the capture.
    pub fn show(&mut self, ui: &mut egui::Ui, bar: Option<Rect>) -> bool {
        let remaining = self.remaining();
        if remaining == 0 {
            return false;
        }
        if self.shown != Some(remaining) {
            self.shown = Some(remaining);
            platform::tick();
        }
        ui.ctx().request_repaint_after(REPAINT);

        let ctx = ui.ctx().clone();
        let card = CentralPanel::default()
            .frame(Frame::NONE.inner_margin(Margin::same(SHADOW_MARGIN)))
            .show(ui, |ui| {
                let t = theme::tokens(ui.ctx());
                widgets::card_frame(t)
                    .shadow(widgets::card_shadow(t))
                    .show(ui, |ui| {
                        let (rect, _) =
                            ui.allocate_exact_size(Vec2::splat(CARD_SIDE), egui::Sense::hover());
                        ui.painter().text(
                            rect.center(),
                            Align2::CENTER_CENTER,
                            remaining.to_string(),
                            egui::FontId::new(DIGIT_SIZE, egui::FontFamily::Monospace),
                            t.text,
                        );
                    })
                    .response
                    .rect
            })
            .inner;
        self.sync_viewport(&ctx, card, bar);
        true
    }

    /// Keeps the viewport hugging the card, parked centred under the
    /// floating bar (following its drags); fallback: centre of the
    /// monitor under the cursor.
    fn sync_viewport(&self, ctx: &egui::Context, card: egui::Rect, bar: Option<Rect>) {
        let target = card.size() + Vec2::splat(2.0 * f32::from(SHADOW_MARGIN));
        let current = ctx.input(|i| i.viewport().inner_rect.map_or(Vec2::ZERO, |r| r.size()));
        if (target - current).abs().max_elem() > 1.0 {
            ctx.send_viewport_cmd(ViewportCommand::InnerSize(target));
        }
        let pos = match bar {
            Some(bar) => egui::pos2(bar.center().x - target.x / 2.0, bar.bottom() + BAR_GAP),
            None => {
                // No visible bar (menu-launched capture): centre the card
                // at 60 % of the monitor under the cursor.
                let Some(m) = platform::cursor_position()
                    .and_then(|(x, y)| crate::capture::screen::monitor_at(x, y))
                    .or_else(|| crate::capture::screen::primary_monitor().ok())
                else {
                    return;
                };
                let scale = m.scale_factor.max(1.0);
                let origin =
                    platform::native_to_ui(egui::pos2(m.rect.x as f32, m.rect.y as f32), scale);
                let size = platform::native_to_ui(
                    egui::pos2(m.rect.width as f32, m.rect.height as f32),
                    scale,
                )
                .to_vec2();
                egui::pos2(
                    origin.x + size.x * 0.5 - target.x / 2.0,
                    origin.y + size.y * 0.6 - target.y / 2.0,
                )
            }
        };
        ctx.send_viewport_cmd(ViewportCommand::OuterPosition(pos));
    }
}
