//! Transient confirmation toasts — a themed pill floating bottom-centre
//! of the window for ~2 s, with a fade/slide in-and-out. Arm with
//! [`toast`] when the event happens; the window calls [`paint_toasts`]
//! (or [`paint_toasts_above`] to clear a bottom bar) every frame so
//! pending toasts appear, stack and expire.

use std::time::{Duration, Instant};

use egui::{Align2, Area, Color32, Context, CornerRadius, Id, Order, Sense, Vec2, vec2};

use crate::ui::{
    icons::{self, Icon},
    theme,
};

/// How long a toast stays on screen.
const TTL: Duration = Duration::from_secs(2);
/// Fade + slide duration, in and out.
const FADE: f32 = 0.15;
/// `ctx.data` key for the pending toast list.
const KEY: &str = "pixme.toasts";

/// Severity of a toast — drives its pill colour and leading icon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastKind {
    /// Green pill + check — confirmations.
    Success,
    /// Accent-blue pill + info icon — neutral information.
    Info,
    /// Amber pill + warning triangle — recoverable issues.
    Warning,
    /// Red pill + X — failures.
    Error,
}

/// Queues `msg` for display. The window must call [`paint_toasts`]
/// every frame — nothing is painted here, so it is safe to call from
/// menus, commands or background-landed state.
pub fn toast(ctx: &Context, kind: ToastKind, msg: impl Into<String>) {
    let key = Id::new(KEY);
    ctx.data_mut(|d| {
        let mut v: Vec<(ToastKind, String, Instant)> = d.get_temp(key).unwrap_or_default();
        v.push((kind, msg.into(), Instant::now()));
        d.insert_temp(key, v);
    });
}

/// Paints the pending toasts above `bottom_margin` points of window
/// chrome (the default hugs the bottom edge; Draw's option bar passes
/// its own height + a gap). Toasts stack newest-last, each with its own
/// fade/slide lifecycle. Call once per frame, after the window content.
pub fn paint_toasts_above(ctx: &Context, bottom_margin: f32) {
    let key = Id::new(KEY);
    let now = Instant::now();
    let mut v: Vec<(ToastKind, String, Instant)> =
        ctx.data_mut(|d| d.get_temp(key).unwrap_or_default());
    v.retain(|(_, _, t)| now.duration_since(*t) < TTL);
    ctx.data_mut(|d| d.insert_temp(key, v.clone()));
    if v.is_empty() {
        return;
    }
    let oldest_left = TTL
        .checked_sub(now.duration_since(v[0].2))
        .unwrap_or_default();
    // Repaint continuously while any toast animates, else wake at expiry.
    if v.iter().any(|(_, _, t)| {
        let age = now.duration_since(*t).as_secs_f32();
        age < FADE || age > TTL.as_secs_f32() - FADE
    }) {
        ctx.request_repaint();
    } else {
        ctx.request_repaint_after(oldest_left.max(Duration::from_millis(16)));
    }
    let t = theme::tokens(ctx);
    Area::new(Id::new("pixme.toast"))
        .order(Order::Foreground)
        .anchor(Align2::CENTER_BOTTOM, vec2(0.0, -bottom_margin))
        .show(ctx, |ui| {
            for (kind, msg, at) in &v {
                let age = now.duration_since(*at).as_secs_f32();
                let (mut alpha, mut dy) = (1.0f32, 0.0f32);
                if age < FADE {
                    alpha = age / FADE;
                    dy = (1.0 - alpha) * 8.0;
                }
                let left = TTL.as_secs_f32() - age;
                if left < FADE {
                    alpha = left / FADE;
                    dy = (1.0 - alpha) * 6.0;
                }
                ui.add_space(dy);
                let (fill, icon) = match kind {
                    ToastKind::Success => (theme::annotation::GREEN, Icon::Check),
                    ToastKind::Info => (t.accent, Icon::Info),
                    ToastKind::Warning => (t.warning, Icon::TriangleAlert),
                    ToastKind::Error => (t.danger, Icon::X),
                };
                egui::Frame::new()
                    .fill(fill)
                    .corner_radius(CornerRadius::same(t.radius_md))
                    .inner_margin(egui::Margin {
                        left: 8,
                        right: 14,
                        top: 6,
                        bottom: 6,
                    })
                    .show(ui, |ui| {
                        ui.multiply_opacity(alpha);
                        ui.horizontal(|ui| {
                            let (badge, _) =
                                ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
                            ui.painter().circle_filled(
                                badge.center(),
                                9.0,
                                Color32::from_rgba_unmultiplied(255, 255, 255, 60),
                            );
                            icons::paint(ui, badge, icon, 10.0, Color32::WHITE);
                            ui.add_space(6.0);
                            ui.label(
                                egui::RichText::new(msg.clone())
                                    .color(Color32::WHITE)
                                    .size(12.0),
                            );
                        });
                    });
                ui.add_space(6.0 - dy.min(6.0));
            }
        });
}

/// Paints the pending toasts hugging the bottom edge — see
/// [`paint_toasts_above`] for the margin-aware variant.
pub fn paint_toasts(ctx: &Context) {
    paint_toasts_above(ctx, 14.0);
}
