//! The capture-delay countdown: a floating always-on-top transparent
//! digit parked at the bottom-right of the monitor under the cursor,
//! painted in the PixMe accent blue with
//! a soft outline so it stays readable over any background.

use std::time::{Duration, Instant};

use egui::{CentralPanel, Frame, Margin, Vec2};

use crate::app::CaptureMode;
use crate::capture::{MonitorInfo, screen};
use crate::platform;
use crate::ui::theme;
use crate::ui::viewport_sync::ViewportSync;

/// Transparent margin kept around the digit so its painted outline is
/// not clipped by the viewport bounds (same trick as the floating bar).
const SHADOW_MARGIN: i8 = 14;

/// Side of the square area holding the digit.
const CARD_SIDE: f32 = 104.0;

/// 3×5 pixel-font rows for digits 0–9 (top row first, bit 2 = leftmost
/// column) — the same blocky style as the PixMe icon's "P".
const PIXEL_DIGITS: [[u8; 5]; 10] = [
    [0b111, 0b101, 0b101, 0b101, 0b111], // 0
    [0b010, 0b110, 0b010, 0b010, 0b111], // 1
    [0b111, 0b001, 0b111, 0b100, 0b111], // 2
    [0b111, 0b001, 0b111, 0b001, 0b111], // 3
    [0b101, 0b101, 0b111, 0b001, 0b001], // 4
    [0b111, 0b100, 0b111, 0b001, 0b111], // 5
    [0b111, 0b100, 0b111, 0b101, 0b111], // 6
    [0b111, 0b001, 0b001, 0b010, 0b010], // 7
    [0b111, 0b101, 0b111, 0b101, 0b111], // 8
    [0b111, 0b101, 0b111, 0b001, 0b111], // 9
];

/// Distance between the digit area's right edge and the monitor's right
/// edge. Note the
/// transparent SHADOW_MARGIN sits inside this distance, so the painted
/// digit lands `EDGE_MARGIN + SHADOW_MARGIN` from the screen edge.
const EDGE_MARGIN: f32 = 10.0;

/// Larger clearance at the bottom: the glyph must breathe above the
/// taskbar/dock, not sit flush on it.
const BOTTOM_MARGIN: f32 = 40.0;

/// Repaint cadence: refreshes the digit without spinning.
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
    /// Last geometry this window asked for — commands are deduped
    /// against it, never against the reported (transient) rect.
    sync: ViewportSync,
}

impl Countdown {
    /// Arms a countdown of `secs` seconds for `mode`.
    pub fn new(mode: CaptureMode, secs: u32) -> Self {
        Self {
            mode,
            deadline: Instant::now() + Duration::from_secs(u64::from(secs)),
            shown: None,
            sync: ViewportSync::new("PixMe Countdown"),
        }
    }

    /// Whole seconds still to go (`0` at the deadline).
    fn remaining(&self) -> u32 {
        self.deadline
            .saturating_duration_since(Instant::now())
            .as_secs_f32()
            .ceil() as u32
    }

    /// Paints the countdown digit for this frame and keeps the viewport
    /// hugging it, parked at the bottom-right of `anchor`'s monitor (the
    /// bar's or the viewer's — resolved in native units by the owner, so
    /// no window's points are mixed here). Without an anchor it falls
    /// back to the monitor under the cursor, then the primary. Returns
    /// `false` once the deadline passed — the caller then fires the
    /// capture.
    pub fn show(&mut self, ui: &mut egui::Ui, anchor: Option<MonitorInfo>) -> bool {
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
                let (rect, _) =
                    ui.allocate_exact_size(Vec2::splat(CARD_SIDE), egui::Sense::hover());
                // Pixel-art digits, matching the "P" of the PixMe icon:
                // a dark drop-shadow pass then the accent-blue fill,
                // right-aligned inside the card.
                paint_pixel_number(ui.painter(), rect, remaining, t.accent);
                rect
            })
            .inner;
        self.sync_viewport(&ctx, card, anchor);
        true
    }

    /// Keeps the viewport hugging the digit area, parked at the
    /// bottom-right of `anchor`'s monitor — with cursor-monitor and
    /// primary fallbacks. The monitor rect is expressed in THIS window's
    /// points (its own ppp, the factor egui-winit multiplies commands by)
    /// and the position is only resent when it really changed — dragging
    /// the window across a mixed-DPI boundary sends nothing.
    fn sync_viewport(&mut self, ctx: &egui::Context, card: egui::Rect, anchor: Option<MonitorInfo>) {
        let target = card.size() + Vec2::splat(2.0 * f32::from(SHADOW_MARGIN));
        self.sync.request_inner_size(ctx, target);
        let Some(m) = anchor
            .or_else(|| {
                platform::cursor_position().and_then(|(x, y)| screen::monitor_at(x, y))
            })
            .or_else(|| screen::primary_monitor().ok())
        else {
            return;
        };
        let ppp = platform::window_ppp(ctx, m.scale_factor.max(1.0));
        let mon = platform::monitor_ui_rect(&m, ppp);
        self.sync.request_outer_position(
            ctx,
            egui::pos2(
                mon.max.x - target.x - EDGE_MARGIN,
                mon.max.y - target.y - BOTTOM_MARGIN,
            ),
        );
    }
}

/// Paints `n` as right-aligned pixel-art digits inside `area`: a dark
/// drop-shadow pass offset down-right, then the accent fill — two flat
/// passes so the glyph stays readable over any wallpaper.
fn paint_pixel_number(painter: &egui::Painter, area: egui::Rect, n: u32, color: egui::Color32) {
    const CELL: f32 = 13.0; // one pixel's edge — a digit lands ~36×65 pt
    const STEP: f32 = CELL * 4.0; // 3-column glyph + 1 column of air
    const SEAM: f32 = 1.5; // dark seam between pixels (the pixelated look)
    let label = n.to_string();
    let width = label.len() as f32 * STEP - CELL;
    let right = area.right() - 10.0;
    let top = area.center().y - CELL * 2.5;
    let paint = |offset: egui::Vec2, c: egui::Color32| {
        for (i, ch) in label.chars().enumerate() {
            let Some(d) = ch.to_digit(10) else { continue };
            let x0 = right - width + i as f32 * STEP + offset.x;
            let y0 = top + offset.y;
            for (row, bits) in PIXEL_DIGITS[d as usize].iter().enumerate() {
                for col in 0..3u8 {
                    if bits & (0b100 >> col) != 0 {
                        painter.rect_filled(
                            egui::Rect::from_min_size(
                                egui::pos2(x0 + col as f32 * CELL, y0 + row as f32 * CELL),
                                Vec2::splat(CELL - SEAM),
                            ),
                            0.0,
                            c,
                        );
                    }
                }
            }
        }
    };
    paint(egui::vec2(4.0, 4.0), egui::Color32::from_black_alpha(140));
    paint(egui::vec2(0.0, 0.0), color);
}
