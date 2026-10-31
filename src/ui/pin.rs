//! Pinned captures: autonomous, always-on-top floating windows that keep
//! a capture on screen — the functional reference's pin, drawn with the
//! app's chrome. A pin owns its image, opens where its source region was
//! captured, and dies with the app (closing the root closes every child
//! viewport).

use std::sync::Arc;

use egui::{Align2, Color32, CornerRadius, Pos2, Rect, Sense, Stroke, Vec2};

use crate::app::{Command, Shared};
use crate::capture::Capture;

use super::{
    icons::{self, Icon},
    theme::{self, Tokens},
};

/// macOS traffic-light red for the hover close dot.
const CLOSE_RED: Color32 = Color32::from_rgb(0xFF, 0x5F, 0x57);
/// Bottom control strip metrics.
const STRIP_H: f32 = 26.0;
const STRIP_PAD: f32 = 10.0;
/// Zoom presets of the percent menu.
const ZOOMS: [f32; 3] = [0.5, 1.0, 2.0];
/// Corner rounding of the pinned image.
const ROUNDING: f32 = 10.0;
/// Dark scoop hugging the top-left corner behind the close dot.
const HALO: f32 = 34.0;
/// Below this width/height only the close dot is shown — there is no
/// room left for the strip icons.
const MIN_STRIP_W: f32 = 210.0;
const MIN_STRIP_H: f32 = 90.0;
/// Below this width the zoom percent is hidden first (the icons matter
/// more).
const MIN_ZOOM_W: f32 = 280.0;
/// The centre grip pill needs more room than the round buttons.
const MIN_PILL_W: f32 = 300.0;
/// Smallest on-screen side a pin can be zoomed to (logical px).
pub const MIN_SIDE: f32 = 64.0;
/// Largest zoom (logical units per image point).
pub const MAX_ZOOM: f32 = 6.0;

/// One pinned capture — an autonomous floating window.
pub struct PinWindow {
    /// Identity; the viewport id is `("pixme.pin", id)` hashed.
    pub id: u32,
    /// The pinned image plus its monitor info (shared, cheap to clone).
    pub capture: Arc<Capture>,
    /// Global logical position the pin opened at — where its source
    /// region sat on screen.
    pub origin: Pos2,
    /// Display zoom (logical units per image point at scale 1). Updated
    /// by the % menu.
    pub zoom: f32,
    /// GPU texture, built once.
    texture: Option<egui::TextureHandle>,
}

impl PinWindow {
    pub fn new(id: u32, capture: Capture, origin: Pos2) -> Self {
        Self {
            id,
            capture: Arc::new(capture),
            origin,
            zoom: 1.0,
            texture: None,
        }
    }

    /// Resize the window to `zoom`, keeping the top-left corner fixed —
    /// sent to THIS viewport directly (the pin is autonomous; no command
    /// round-trip through the shared queue).
    fn apply_zoom(&mut self, ctx: &egui::Context, z: f32) {
        let nat = self.logical_size() / self.zoom;
        let z_min = (MIN_SIDE / nat.x).max(MIN_SIDE / nat.y);
        self.zoom = z.clamp(z_min, MAX_ZOOM);
        let size = self.logical_size();
        if let Some(outer) = ctx.input(|i| i.viewport().outer_rect) {
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(outer.min));
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
    }

    /// On-screen logical size at the current zoom (physical px ÷ scale).
    pub fn logical_size(&self) -> Vec2 {
        let s = self.capture.scale_factor.max(f32::EPSILON);
        Vec2::new(
            self.capture.image.width() as f32 / s,
            self.capture.image.height() as f32 / s,
        ) * self.zoom
    }

    /// The pinned image plus, on hover, its control chrome.
    pub fn show(&mut self, ui: &mut egui::Ui, shared: &Shared) {
        let ctx = ui.ctx().clone();
        let rect = ui.max_rect();
        let t = theme::tokens(&ctx);

        let img = &self.capture.image;
        let tex = self.texture.get_or_insert_with(|| {
            ctx.load_texture(
                format!("pin-{}", self.id),
                egui::ColorImage::from_rgba_unmultiplied(
                    [img.width() as usize, img.height() as usize],
                    img.as_raw(),
                ),
                egui::TextureOptions::LINEAR,
            )
        });
        egui::Image::new(egui::load::SizedTexture::from_handle(tex))
            .corner_radius(ROUNDING)
            .paint_at(ui, rect);
        ui.painter().rect_stroke(
            rect,
            ROUNDING as u8,
            Stroke::new(1.0, t.card_border),
            egui::StrokeKind::Inside,
        );

        // The image body moves the window. Controls registered above it
        // use `Sense::click_and_drag()` so egui's hit-tester claims both
        // the click and the drag hit — a pure `Sense::click()` on top of
        // a `Sense::drag()` background lets the drag hit fall through to
        // the background and starts a window drag on press.
        let body = ui.interact(rect, ui.id().with("pin-body"), Sense::drag());

        let hovered = ctx
            .input(|i| i.pointer.latest_pos())
            .is_some_and(|p| rect.contains(p));
        if hovered {
            self.chrome(ui, &ctx, rect, &t, shared);
        }

        if body.drag_started() {
            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
    }

    /// Hover controls: halo + red close dot top-left; bottom strip with
    /// the zoom percent, a drag grip, and copy / save buttons — hidden
    /// progressively when the window shrinks.
    fn chrome(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        rect: Rect,
        t: &Tokens,
        shared: &Shared,
    ) {
        // -- dark scoop hugging the corner behind the close dot -----------
        let halo = Rect::from_min_size(rect.min, Vec2::splat(HALO));
        ui.painter().rect_filled(
            halo,
            CornerRadius {
                nw: ROUNDING as u8,
                ne: 0,
                sw: 0,
                se: 16,
            },
            t.dock,
        );

        // -- red close dot -------------------------------------------------
        let c = rect.min + Vec2::splat(16.0);
        let dot = Rect::from_center_size(c, Vec2::splat(18.0));
        let dot_resp = ui.interact(dot, ui.id().with("pin-close"), Sense::click_and_drag());
        let fill = if dot_resp.hovered() {
            t.danger
        } else {
            CLOSE_RED
        };
        ui.painter().circle_filled(c, 7.0, fill);
        if dot_resp.hovered() {
            icons::paint(
                ui,
                dot.shrink(5.0),
                Icon::X,
                9.0,
                Color32::from_black_alpha(150),
            );
            ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if dot_resp.clicked() || (dot_resp.drag_stopped() && dot_resp.contains_pointer()) {
            shared.push(Command::ClosePin(self.id));
            return;
        }

        // Progressive chrome: on a tiny pin the close dot is all that fits.
        if rect.width() < MIN_STRIP_W || rect.height() < MIN_STRIP_H {
            return;
        }
        let strip_y = rect.max.y - STRIP_PAD - STRIP_H;

        // -- zoom percent pill (bottom-left) ------------------------------
        if rect.width() >= MIN_ZOOM_W {
            let zoom_rect = Rect::from_min_size(
                Pos2::new(rect.min.x + STRIP_PAD, strip_y),
                Vec2::new(56.0, STRIP_H),
            );
            let zoom_resp =
                ui.interact(zoom_rect, ui.id().with("pin-zoom"), Sense::click_and_drag());
            ui.painter().rect_filled(
                zoom_rect,
                STRIP_H / 2.0,
                if zoom_resp.hovered() { t.hover } else { t.card },
            );
            ui.painter().rect_stroke(
                zoom_rect,
                STRIP_H / 2.0,
                Stroke::new(1.0, t.card_border),
                egui::StrokeKind::Outside,
            );
            ui.painter().text(
                zoom_rect.center(),
                Align2::CENTER_CENTER,
                format!("{}%", (self.zoom * 100.0).round() as u32),
                theme::semibold(11.0),
                t.text,
            );
            egui::Popup::menu(&zoom_resp).show(|ui| {
                for z in ZOOMS {
                    let label = format!("{}%", (z * 100.0).round() as u32);
                    if ui
                        .selectable_label((self.zoom - z).abs() < f32::EPSILON, label)
                        .clicked()
                    {
                        self.apply_zoom(ctx, z);
                        ui.close();
                    }
                }
            });
        }

        // -- centre pill: drag grip (also moves the window) ---------------
        if rect.width() >= MIN_PILL_W {
            let pill = Rect::from_center_size(
                Pos2::new(rect.center().x, strip_y + STRIP_H / 2.0),
                Vec2::new(72.0, STRIP_H),
            );
            let grip = ui.interact(pill, ui.id().with("pin-grip"), Sense::drag());
            ui.painter().rect_filled(
                pill,
                STRIP_H / 2.0,
                if grip.hovered() {
                    theme::pin_chrome::BG_HOVER
                } else {
                    theme::pin_chrome::BG
                },
            );
            ui.painter().rect_stroke(
                pill,
                STRIP_H / 2.0,
                Stroke::new(1.0, theme::pin_chrome::BORDER),
                egui::StrokeKind::Outside,
            );
            icons::paint(
                ui,
                Rect::from_center_size(
                    Pos2::new(pill.center().x - 12.0, pill.center().y),
                    Vec2::splat(16.0),
                ),
                Icon::GripVertical,
                14.0,
                theme::pin_chrome::ICON,
            );
            icons::paint(
                ui,
                Rect::from_center_size(
                    Pos2::new(pill.center().x + 12.0, pill.center().y),
                    Vec2::splat(16.0),
                ),
                Icon::Pin,
                13.0,
                theme::pin_chrome::ICON,
            );
            if grip.drag_started() {
                ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            grip.on_hover_cursor(egui::CursorIcon::Grab);
        }

        // -- right: copy and save round buttons ---------------------------
        for (n, icon, cmd) in [
            (0, Icon::Copy, Command::CopyPin(self.id)),
            (1, Icon::Download, Command::SavePin(self.id)),
        ] {
            let c = Pos2::new(
                rect.max.x - STRIP_PAD - STRIP_H / 2.0 - n as f32 * (STRIP_H + 8.0),
                strip_y + STRIP_H / 2.0,
            );
            let cr = Rect::from_center_size(c, Vec2::splat(STRIP_H));
            let r = ui.interact(cr, ui.id().with(("pin-act", n)), Sense::click_and_drag());
            ui.painter().circle_filled(
                c,
                STRIP_H / 2.0,
                if r.hovered() {
                    theme::pin_chrome::BG_HOVER
                } else {
                    theme::pin_chrome::BG
                },
            );
            ui.painter().circle_stroke(
                c,
                STRIP_H / 2.0,
                Stroke::new(1.0, theme::pin_chrome::BORDER),
            );
            icons::paint(ui, cr.shrink(7.0), icon, 13.0, theme::pin_chrome::ICON);
            let tip = match n {
                0 => t!("pin.copy"),
                _ => t!("pin.save"),
            };
            let clicked = r.clicked() || (r.drag_stopped() && r.contains_pointer());
            r.on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text(tip);
            if clicked {
                shared.push(cmd);
            }
        }
    }
}
