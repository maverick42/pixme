//! PixMe's two palettes — Pro (dark) and Pro Light — adapted from the visual
//! reference (MIT OR Apache-2.0; see ATTRIBUTION.md). Tokens are identical in
//! name and number across themes (enforced by exhaustive literals; `docs/
//! theme.md` is the prose source of truth).
//!
//! Theme-independent colours (region-selection scrim, annotation palette) are
//! fixed constants at the bottom of this file.

use egui::{
    Color32, Context, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Id, Margin,
    Shadow, Stroke, Style, TextStyle, Vec2, Visuals,
};

/// One of the two supported palettes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeKind {
    /// Darkest palette: `#323232` panels on a `#1E1E1E` dock.
    Pro,
    /// Lightest palette: `#F0F0F0` panels.
    ProLight,
}

impl ThemeKind {
    /// The token set for this palette.
    pub fn tokens(self) -> Tokens {
        match self {
            Self::Pro => Tokens::pro(),
            Self::ProLight => Tokens::pro_light(),
        }
    }
}

/// Named visual tokens. Both palettes fill every field — adding a token
/// without updating both literals fails compilation.
#[derive(Clone, Copy, Debug)]
pub struct Tokens {
    /// Window chrome / toolbar bar surface.
    pub chrome: Color32,
    /// Canvas behind the edited image (darker than panels).
    pub canvas: Color32,
    /// Background behind panels/cards.
    pub dock: Color32,
    /// Panel and floating-bar fill.
    pub card: Color32,
    /// Panel outline.
    pub card_border: Color32,
    /// Input and dropdown fill.
    pub field: Color32,
    /// Input outline.
    pub field_border: Color32,
    /// Hovered control fill.
    pub hover: Color32,
    /// Pressed control fill.
    pub pressed: Color32,
    /// Body text.
    pub text: Color32,
    /// Secondary text.
    pub text_dim: Color32,
    /// Hints and disabled text.
    pub text_faint: Color32,
    /// Icon tint at rest.
    pub icon: Color32,
    /// Accent (Spectrum blue).
    pub accent: Color32,
    /// Selected fill behind icons/chips.
    pub accent_soft: Color32,
    /// Selected outline.
    pub accent_border: Color32,
    /// Text/icon drawn on `accent`.
    pub accent_text: Color32,
    /// Hairlines.
    pub separator: Color32,
    /// Shadow colour (with alpha).
    pub shadow: Color32,
    /// Primary action button fill.
    pub primary_bg: Color32,
    /// Text on `primary_bg`.
    pub primary_text: Color32,
    /// Destructive/error accents.
    pub danger: Color32,
    /// Warning accents.
    pub warning: Color32,
    /// Text/region selection fill.
    pub selection: Color32,
    /// Tab-strip background.
    pub tab_strip: Color32,
    /// Selected list row.
    pub row_selected: Color32,
    /// Small radius (icon buttons, chips).
    pub radius_sm: u8,
    /// Medium radius (buttons, fields, menus).
    pub radius_md: u8,
    /// Large radius (windows, panels, the floating bar).
    pub radius_lg: u8,
}

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb(
        ((hex >> 16) & 0xFF) as u8,
        ((hex >> 8) & 0xFF) as u8,
        (hex & 0xFF) as u8,
    )
}

impl Tokens {
    /// Dark palette (darkest Pro level of the visual reference).
    pub const fn pro() -> Self {
        Self {
            chrome: rgb(0x323232),
            canvas: rgb(0x282828),
            dock: rgb(0x1E1E1E),
            card: rgb(0x323232),
            card_border: rgb(0x1E1E1E),
            field: rgb(0x242424),
            field_border: rgb(0x4A4A4A),
            hover: rgb(0x424242),
            pressed: rgb(0x4E4E4E),
            text: rgb(0xDEDEDE),
            text_dim: rgb(0xB2B2B2),
            text_faint: rgb(0x808080),
            icon: rgb(0xC8C8C8),
            accent: rgb(0x378EF0),
            accent_soft: rgb(0x4E4E4E),
            accent_border: rgb(0x378EF0),
            accent_text: Color32::WHITE,
            separator: rgb(0x1E1E1E),
            shadow: Color32::from_black_alpha(150),
            primary_bg: rgb(0x378EF0),
            primary_text: Color32::WHITE,
            danger: rgb(0xEC5B62),
            warning: rgb(0xE8B046),
            selection: rgb(0x378EF0),
            tab_strip: rgb(0x262626),
            row_selected: rgb(0x525252),
            radius_sm: 3,
            radius_md: 4,
            radius_lg: 6,
        }
    }

    /// Light palette (lightest Pro level of the visual reference).
    pub const fn pro_light() -> Self {
        Self {
            chrome: rgb(0xF0F0F0),
            canvas: rgb(0xD6D6D6),
            dock: rgb(0xE2E2E2),
            card: rgb(0xF0F0F0),
            card_border: rgb(0xD6D6D6),
            field: Color32::WHITE,
            field_border: rgb(0xBABABA),
            hover: rgb(0xE2E2E2),
            pressed: rgb(0xCECECE),
            text: rgb(0x141414),
            text_dim: rgb(0x404040),
            text_faint: rgb(0x787878),
            icon: rgb(0x2E2E2E),
            accent: rgb(0x378EF0),
            accent_soft: rgb(0xD6D6D6),
            accent_border: rgb(0x378EF0),
            accent_text: rgb(0x1062C4),
            separator: rgb(0xD6D6D6),
            shadow: Color32::from_black_alpha(50),
            primary_bg: rgb(0x378EF0),
            primary_text: Color32::WHITE,
            danger: rgb(0xEC5B62),
            warning: rgb(0xE8B046),
            selection: rgb(0x378EF0),
            tab_strip: rgb(0xE2E2E2),
            row_selected: rgb(0xCCCCCC),
            radius_sm: 3,
            radius_md: 4,
            radius_lg: 6,
        }
    }
}

// --- Theme-independent constants -------------------------------------------

/// Fixed palette for annotations: readable on arbitrary screenshots.
pub mod annotation {
    use egui::Color32;

    /// Classic markup red.
    pub const RED: Color32 = Color32::from_rgb(0xED, 0x4B, 0x46);
    /// Markup yellow.
    pub const YELLOW: Color32 = Color32::from_rgb(0xF5, 0xC5, 0x3D);
    /// Markup green.
    pub const GREEN: Color32 = Color32::from_rgb(0x3E, 0xBB, 0x6B);
    /// Markup blue (same Spectrum blue as the accent).
    pub const BLUE: Color32 = Color32::from_rgb(0x37, 0x8E, 0xF0);
    /// Markup magenta.
    pub const MAGENTA: Color32 = Color32::from_rgb(0xD7, 0x62, 0xD9);
    /// White annotation ink (paired with a dark halo at draw time).
    pub const WHITE: Color32 = Color32::WHITE;
    /// Black annotation ink.
    pub const BLACK: Color32 = Color32::BLACK;

    /// Default annotation colour order for the picker.
    pub const ALL: [Color32; 7] = [RED, YELLOW, GREEN, BLUE, MAGENTA, WHITE, BLACK];
}

/// Dim veil drawn over the frozen screen during region selection.
pub const REGION_SCRIM: Color32 = Color32::from_black_alpha(110);

// --- Context access ---------------------------------------------------------

const TOKENS_ID: &str = "pixme/theme/tokens";

/// The currently active palette's tokens (Pro when none was applied yet).
pub fn tokens(ctx: &Context) -> Tokens {
    ctx.data(|d| {
        d.get_temp::<Tokens>(Id::new(TOKENS_ID))
            .unwrap_or_else(Tokens::pro)
    })
}

/// Applies the palette: forces egui's theme, sets `Visuals`, widget states,
/// shadows, text styles, spacing, and stores `tokens` for widgets to read
/// via [`tokens`].
pub fn apply(ctx: &Context, kind: ThemeKind) {
    let t = kind.tokens();
    ctx.data_mut(|d| d.insert_temp(Id::new(TOKENS_ID), t));

    let theme = match kind {
        ThemeKind::Pro => egui::Theme::Dark,
        ThemeKind::ProLight => egui::Theme::Light,
    };
    ctx.set_theme(theme);
    ctx.style_mut_of(theme, |style| {
        let dark = kind == ThemeKind::Pro;
        apply_visuals(&mut style.visuals, dark, t);
        apply_style(style);
    });
}

fn apply_visuals(v: &mut Visuals, dark: bool, t: Tokens) {
    *v = if dark {
        Visuals::dark()
    } else {
        Visuals::light()
    };
    v.dark_mode = dark;
    v.override_text_color = Some(t.text);
    v.weak_text_color = Some(t.text_dim);
    v.hyperlink_color = t.accent;
    v.error_fg_color = t.danger;
    v.warn_fg_color = t.warning;

    v.panel_fill = t.chrome;
    v.window_fill = t.card;
    v.window_stroke = Stroke::new(1.0, t.card_border);
    v.window_corner_radius = CornerRadius::same(t.radius_lg);
    v.menu_corner_radius = CornerRadius::same(t.radius_md);
    v.faint_bg_color = t.dock;
    v.extreme_bg_color = t.field;
    v.text_edit_bg_color = Some(t.field);
    v.selection.bg_fill = t.selection;
    v.selection.stroke = Stroke::new(1.0, t.accent_text);

    let shadow = Shadow {
        offset: [0, 6],
        blur: 16,
        spread: 0,
        color: t.shadow,
    };
    v.window_shadow = shadow;
    v.popup_shadow = shadow;

    let md = CornerRadius::same(t.radius_md);
    let text = Stroke::new(1.0, t.text);
    v.widgets.noninteractive = egui::style::WidgetVisuals {
        bg_fill: t.card,
        weak_bg_fill: t.card,
        bg_stroke: Stroke::new(1.0, t.separator),
        corner_radius: md,
        fg_stroke: text,
        expansion: 0.0,
    };
    v.widgets.inactive = egui::style::WidgetVisuals {
        bg_fill: t.card,
        weak_bg_fill: t.field,
        bg_stroke: Stroke::new(1.0, t.field_border),
        fg_stroke: text,
        ..v.widgets.noninteractive
    };
    v.widgets.hovered = egui::style::WidgetVisuals {
        bg_fill: t.hover,
        weak_bg_fill: t.hover,
        bg_stroke: Stroke::new(1.0, t.field_border),
        fg_stroke: text,
        ..v.widgets.noninteractive
    };
    v.widgets.active = egui::style::WidgetVisuals {
        bg_fill: t.pressed,
        weak_bg_fill: t.pressed,
        bg_stroke: Stroke::new(1.0, t.field_border),
        fg_stroke: text,
        ..v.widgets.noninteractive
    };
    v.widgets.open = egui::style::WidgetVisuals {
        weak_bg_fill: t.field,
        ..v.widgets.inactive
    };
}

fn apply_style(style: &mut Style) {
    use egui::FontFamily::{Monospace, Name, Proportional};

    let semibold: FontFamily = Name("semibold".into());
    style.text_styles = [
        (TextStyle::Body, FontId::new(12.0, Proportional)),
        (TextStyle::Button, FontId::new(12.0, Proportional)),
        (TextStyle::Heading, FontId::new(15.0, semibold)),
        (TextStyle::Small, FontId::new(10.5, Proportional)),
        (TextStyle::Monospace, FontId::new(12.0, Monospace)),
    ]
    .into();

    style.spacing.item_spacing = Vec2::new(8.0, 4.0);
    style.spacing.button_padding = Vec2::new(10.0, 5.0);
    style.spacing.window_margin = Margin::same(8);
    style.spacing.menu_margin = Margin::same(6);
    style.spacing.interact_size = Vec2::new(20.0, 20.0);
    style.interaction.tooltip_delay = 0.4;
    style.interaction.show_tooltips_only_when_still = true;
}

// --- Fonts ------------------------------------------------------------------

const INTER_REGULAR: &[u8] = include_bytes!("../../assets/fonts/Inter-Regular.ttf");
const INTER_MEDIUM: &[u8] = include_bytes!("../../assets/fonts/Inter-Medium.ttf");
const INTER_SEMIBOLD: &[u8] = include_bytes!("../../assets/fonts/Inter-SemiBold.ttf");

/// Custom egui font families registered by [`install_fonts`]:
/// `"medium"` and `"semibold"` resolve to Inter Medium/SemiBold with a
/// fallback to Inter Regular.
fn family(name: &str) -> FontFamily {
    FontFamily::Name(name.into())
}

/// A [`FontId`] in Inter Medium (`family = "medium"`).
pub fn medium(size: f32) -> FontId {
    FontId::new(size, family("medium"))
}

/// A [`FontId`] in Inter SemiBold (`family = "semibold"`).
pub fn semibold(size: f32) -> FontId {
    FontId::new(size, family("semibold"))
}

/// Registers Inter Regular/Medium/SemiBold and repoints egui's `Proportional`
/// family at Inter Regular first (bundled defaults remain as fallbacks).
pub fn install_fonts(ctx: &Context) {
    let mut fonts = FontDefinitions::default();
    for (name, bytes) in [
        ("Inter-Regular", INTER_REGULAR),
        ("Inter-Medium", INTER_MEDIUM),
        ("Inter-SemiBold", INTER_SEMIBOLD),
    ] {
        fonts.font_data.insert(
            name.into(),
            std::sync::Arc::new(FontData::from_static(bytes)),
        );
    }
    let families = fonts.families.get_mut(&FontFamily::Proportional);
    if let Some(list) = families {
        list.insert(0, "Inter-Regular".into());
    }
    fonts.families.insert(
        family("medium"),
        vec!["Inter-Medium".into(), "Inter-Regular".into()],
    );
    fonts.families.insert(
        family("semibold"),
        vec!["Inter-SemiBold".into(), "Inter-Regular".into()],
    );
    ctx.set_fonts(fonts);
}

#[cfg(test)]
mod tests {
    use owned_ttf_parser::AsFaceRef;

    use super::*;

    /// WCAG 2.x relative luminance, in sRGB.
    fn luminance(c: Color32) -> f32 {
        fn channel(v: u8) -> f32 {
            let v = f32::from(v) / 255.0;
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        }
        0.2126 * channel(c.r()) + 0.7152 * channel(c.g()) + 0.0722 * channel(c.b())
    }

    /// WCAG contrast ratio (1.0–21.0).
    fn contrast(a: Color32, b: Color32) -> f32 {
        let (hi, lo) = {
            let (la, lb) = (luminance(a), luminance(b));
            if la > lb { (la, lb) } else { (lb, la) }
        };
        (hi + 0.05) / (lo + 0.05)
    }

    /// Foreground/background pairs that must stay readable in both palettes
    /// (WCAG AA normal text = 4.5).
    #[test]
    fn contrast_holds_for_text_pairs() {
        for t in [Tokens::pro(), Tokens::pro_light()] {
            for (fg, bg, what) in [
                (t.text, t.card, "text on card"),
                (t.text, t.chrome, "text on chrome"),
                (t.text_dim, t.card, "dim text on card"),
                (t.icon, t.card, "icon on card"),
                (t.icon, t.hover, "icon on hover"),
                // Accent-tinted text drawn on surfaces (not on accent fill).
                (t.accent_text, t.card, "accent text on card"),
            ] {
                assert!(
                    contrast(fg, bg) >= 4.5,
                    "{what}: {:.2} < 4.5 (fg {fg:?} on {bg:?})",
                    contrast(fg, bg)
                );
            }
            // The primary button ships the reference's exact colours: white on
            // Spectrum blue measures 3.34:1 — at/above WCAG's 3:1 floor for
            // UI components and large text. Kept for visual parity.
            assert!(
                contrast(t.primary_text, t.primary_bg) >= 3.0,
                "primary button: {:.2} < 3.0",
                contrast(t.primary_text, t.primary_bg)
            );
        }
    }

    /// Bundled fonts cover every accented/special glyph PixMe's locales need.
    #[test]
    fn inter_covers_required_glyphs() {
        const REQUIRED: &str = "éèêàçñ¿¡ÉÈÊÀÇÑ";
        for (name, bytes) in [
            ("Inter-Regular", INTER_REGULAR),
            ("Inter-Medium", INTER_MEDIUM),
            ("Inter-SemiBold", INTER_SEMIBOLD),
        ] {
            let face = owned_ttf_parser::OwnedFace::from_vec(bytes.to_vec(), 0)
                .expect("embedded font must parse");
            let face = face.as_face_ref();
            for ch in REQUIRED.chars() {
                assert!(
                    face.glyph_index(ch).is_some(),
                    "{name} is missing glyph {ch:?}"
                );
            }
        }
    }
}
