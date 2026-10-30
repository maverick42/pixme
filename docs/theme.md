# PixMe theme

PixMe's interface is inspired by **PhotoCraft**. This document is the source
of truth for PixMe's visual tokens (rule 10): it summarises the visual
reference's design system and states exactly what PixMe imports. PixMe keeps
**two themes only**: Pro (dark) and Pro Light.

## Source and licence

- Source: the visual reference's egui UI crate (an external local checkout —
  ask the user for its location; file list at the end of this document).
- Licence: **MIT OR Apache-2.0** — permissive. Reuse is allowed; copyright
  notices stay with the original authors and are kept in ported source files
  and in `ATTRIBUTION.md` (see `docs/decisions.md`).

## Dependency versions (from the reference's `Cargo.lock`)

| Crate | Version | PixMe use |
| --- | --- | --- |
| egui | 0.36.2 | UI |
| eframe | 0.36.2 | App shell — **`glow` backend** (the reference uses `wgpu`; both are supported by eframe, PixMe pins `glow` per project rule) |
| epaint / ecolor | 0.36.2 | via egui |
| egui-winit | 0.36.2 | via eframe |
| egui_glow | 0.36.2 | via eframe `glow` feature |
| egui_extras | 0.36.2 (`default-features = false`, features `["svg"]`) | SVG icon rasterization (`egui_extras::image::load_svg_bytes_with_size`) |

## Theme model

`ThemeKind { Pro, ProLight }` only (the reference app ships 14 themes;
PixMe imports exactly these two palettes). A `Tokens` struct holds every colour
and radius used by the UI; widgets read `Tokens::get(ctx)` (stored in egui
context data by `apply`), never a literal colour. Both palettes fill the **same
exhaustive token set** — the struct literal is written out in full for each
theme (no `..base` spread), so the compiler fails the build if a token is
missing on either side. A unit test additionally asserts the surface ordering
and WCAG contrast floors (below).

`Appearance` setting: `Auto | Dark | Light`, persisted in the config, applied
at runtime. `Auto` follows `ctx.input(|i| i.system_theme)` (reported by winit
on macOS and Windows) and falls back to **Dark** when the OS reports nothing.

### Token table

| Token | Pro (dark) | Pro Light | Use |
| --- | --- | --- | --- |
| `chrome` | `#323232` | `#F0F0F0` | Window chrome, toolbar bar surface |
| `dock` | `#1E1E1E` | `#E2E2E2` | Background behind cards |
| `card` | `#323232` | `#F0F0F0` | Panels / floating bar fill |
| `card_border` | `#1E1E1E` | `#D6D6D6` | Panel outline |
| `field` | `#242424` | `#FFFFFF` | Inputs, dropdowns |
| `field_border` | `#4A4A4A` | `#BABABA` | Input outline |
| `hover` | `#424242` | `#E2E2E2` | Hovered control fill |
| `pressed` | `#4E4E4E` | `#CECECE` | Pressed control fill |
| `text` | `#DEDEDE` | `#141414` | Body text |
| `text_dim` | `#B2B2B2` | `#404040` | Secondary text |
| `text_faint` | `#808080` | `#787878` | Hints, disabled |
| `icon` | `#C8C8C8` | `#2E2E2E` | Icon tint at rest |
| `accent` | `#378EF0` | `#378EF0` | Accent (selection, links, focus ring) |
| `accent_soft` | `#4E4E4E` | `#D6D6D6` | Selected fill behind icons |
| `accent_border` | `#378EF0` | `#378EF0` | Selected outline |
| `accent_text` | `#FFFFFF` | `#1062C4` | Text/glyph on accent surfaces |
| `separator` | `#1E1E1E` | `#D6D6D6` | Hairlines |
| `shadow` | `rgba(0,0,0,0.59)` | `rgba(0,0,0,0.20)` | Floating window shadow |
| `primary_bg` | `#378EF0` | `#378EF0` | Primary button fill |
| `primary_text` | `#FFFFFF` | `#FFFFFF` | Primary button label |
| `danger` | `#EC5B62` | `#EC5B62` | Errors / destructive |
| `warning` | `#E8B046` | `#E8B046` | Warnings |
| `tab_strip` | `#262626` | `#E2E2E2` | Tab-strip background |
| `row_selected` | `#525252` | `#CCCCCC` | Selected list row |
| `scrim` | `rgba(0,0,0,0.43)` | `rgba(0,0,0,0.43)` | Dim behind modal |
| `caption_close` | `#C42B1C` | `#C42B1C` | Close hover (custom caption, Windows) |
| `caption_close_text` | `#FFFFFF` | `#FFFFFF` | Close glyph on hover |
| `radius_sm` | `3.0` | `3.0` | Controls |
| `radius` | `4.0` | `4.0` | Menus, popups |
| `radius_lg` | `6.0` | `6.0` | Windows, floating bar |
| `pro` | `true` | `true` | Layout flag: flat panels, pill buttons |

Layout-flag note: the reference's `bevel`/`round_chips` flags and editor-only
tokens (`canvas`, `canvas_dot`, `histogram_*`, layer-label colours) are not
imported — PixMe has no canvas docks or histograms.

### Theme-independent colours (never change with theme)

Defined as constants in `ui/theme.rs`, outside `Tokens`:

- **Region-selection dim veil**: dark translucent overlay over the frozen
  capture — fixed `rgba(0,0,0,~0.45)` regardless of UI theme.
- **Annotation palette** (arrows, shapes, text, step markers, blur handles):
  fixed set (red default, plus yellow/green/blue/white/black), chosen to read
  on arbitrary screenshots.

## egui `Visuals` mapping (`apply`)

From the reference's `theme::apply` — build `Visuals::dark()/light()`, then:

- `panel_fill = chrome`, `window_fill = card`, `window_stroke = 1px card_border`
- `extreme_bg_color = field`, `faint_bg_color = card`, `code_bg_color = field`
- `override_text_color = text`, `hyperlink_color = accent`
- `warn_fg_color = warning`, `error_fg_color = danger`
- `window_corner_radius = radius_lg`, `menu_corner_radius = radius`
- `window_shadow = offset(0,10) blur 32`, `popup_shadow = offset(0,6) blur 20`
- `selection.bg_fill = accent` (Pro keeps solid accent), `selection.stroke = 1px accent_text`
- `slider_trailing_fill = true`, `handle_shape = Circle`, `striped = false`
- Widgets: `noninteractive = card / 1px separator / fg text_dim`;
  `inactive = field`, `hovered = hover`, `active = pressed + accent_border`,
  `open = hover`; all `corner_radius = radius_sm`, `expansion = 0`

Both egui style branches are set (`ctx.set_style_of` for `Light` and `Dark`)
so egui's own OS-appearance switch never sees an unstyled branch; when PixMe
is in `Auto` the palette itself is re-applied on system theme change.
`ctx.set_theme(ThemePreference::System)` keeps native windows reporting OS
appearance changes.

## Typography and spacing

Fonts (see below) with these `TextStyle`s: `Small` 10.5, `Body`/`Button` 12.0
(Pro size), `Heading` = semibold 15, `Monospace` 12.

`Spacing`: `item_spacing (8,6)`, `button_padding (10,4)`,
`interact_size 24×24`, `slider_width 150`, `combo_width 120`,
`menu_margin 6`, `window_margin 16`, `icon_width 14`, `tooltip_width 280`,
tooltip delay 0.35 s; scroll style = thin floating bars, width 4.

Helpers: `medium(size)`, `semibold(size)`, `mono(size)` `FontId`s.

## Fonts

Bundled (from the reference's `assets/fonts/`, OFL 1.1 — licence files copied
next to the fonts):

- Inter Regular / Medium / SemiBold — UI text.
- JetBrains Mono Regular — numeric fields (defer until a numeric input ships).

Stack: `Proportional = [Inter, …egui defaults]`, `Monospace = [JetBrainsMono, …]`;
named families `medium` → `Inter-Medium` + proportional fallback,
`semibold` → `Inter-SemiBold` + fallback. egui's own `default_fonts` stay last
in the stack for symbols/emoji Inter lacks (the reference deflates them for a
wasm size cap that does not apply to PixMe — keep the feature on).

**Accent coverage — verified**: `é è ê à ç ñ ¿ ¡` (and `É È Ê À Ç Ñ`) all
have glyphs in Inter Regular/Medium/SemiBold and JetBrains Mono (cmap check).
A regression test re-checks coverage in CI. No CJK/lazy system-font machinery
is imported — en/fr/es are Latin-only.

## Icons

The reference embeds **Lucide** SVGs (`assets/icons/`, ISC licence, licence file
copied with the assets): `icon_data.rs` is a generated `ICONS: &[(&str, &[u8])]`
table of `include_bytes!`. At runtime each SVG is recoloured
`currentColor → #ffffff` with `stroke-width = 1.75`, rasterized by
`egui_extras`' resvg loader at the exact on-screen pixel size (crisp at any
DPI), and tinted per use. The reference's `craft-ui` `SvgIconSet`/`IconButton` are
**not** taken as a dependency — the same ~80-line pattern is reimplemented on
`egui_extras` (see `docs/decisions.md`).

Icon-button chrome: transparent at rest → `hover` fill → `accent_soft` fill +
`1px accent_border` + `accent_text` glyph when selected; corner radius
`radius_sm`; glyph ≈ 52 % of the box; hit area `interact_size`.

Icons already in the set that PixMe needs: `monitor`/`scan` (full screen),
`square-dashed` (region), `app-window` (window), `settings`, `x` (quit),
`crop`, `droplet` (blur), `type`, `circle-dot`/`plus`/`check`/chevrons.
Known gap: no straight-arrow glyph for the arrow tool — add Lucide
`move-up-right`/`arrow-up-right` (same ISC source) when the editor lands.

## Reusable components (ported to `src/ui/widgets/`)

From the reference's `widgets.rs`/`icons.rs`: `icon_button`, `primary_button` /
`secondary_button` (`primary_bg` + `primary_text`), `dropdown` (chevron),
`toggle`, `checkbox`, `card` frame, `hairline`/`vline`, `elided_label`
(truncate with `…`, full text on hover), `menu_scroll` for long popups.

The floating mini-toolbar is a borderless, transparent, always-on-top
viewport; its visible frame is drawn as `card` fill + `card_border` stroke +
`radius_lg` + `window_shadow`, dragged via `ViewportCommand::start_drag`.

## Contrast guarantees (tests ported)

WCAG 2 contrast helper from `theme.rs` tests; floors: body `text` ≥ 4.5:1,
`icon`/`text_dim` ≥ 3:1 on `chrome`, `dock`, `card`, `field`, `hover`,
`tab_strip`, `row_selected`; `accent_text` ≥ 3:1 on `card`; close glyph ≥ 4.5:1.
The floating bar is additionally checked visually over light and dark desktop
backgrounds (it floats above arbitrary content; `card` + `shadow` isolate it).

## Reference source files analysed (visual reference checkout)

- `crates/ui-egui/src/theme.rs` — `ThemeKind`, `Tokens`, `apply`, fonts install
- `crates/ui-egui/src/widgets.rs` + `widgets/` — shared widgets
- `crates/ui-egui/src/icons.rs`, `icon_data.rs` — icon pipeline
- `crates/ui-egui/src/egui_fonts.rs`, `cjk_fonts.rs` — font stacks (CJK not imported)
- `docs/ui-design.md` — design-system rules
- `assets/icons/` (164 SVGs, `LICENSE-lucide.txt`), `assets/fonts/` + `OFL-*.txt`
- `ATTRIBUTION.md` — attribution convention PixMe follows
