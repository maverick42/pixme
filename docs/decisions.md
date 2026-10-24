# Decisions log

One entry per architectural, dependency, scope or visual decision, newest last.
Format: date — context — decision — consequences.

## 2026-10-24 — egui/eframe pinned to 0.36.2, `glow` backend

- **Context**: the project rule requires the same egui/eframe versions as the
  visual reference; its `Cargo.lock` resolves them to 0.36.2 (egui, eframe,
  epaint, ecolor, egui-winit, egui_glow, egui-wgpu, egui_extras).
- **Decision**: PixMe uses eframe/egui **0.36.2** with the **`glow`** renderer,
  not `wgpu` (project rule: lightweight standalone binary). `egui_extras` is
  added with `default-features = false, features = ["svg"]` for icon rendering.
- **Consequences**: version-locked to the reference app, so ported widgets and
  the SVG loader behave identically; `glow` keeps the binary small and
  dependency-light.

## 2026-10-24 — Visual-reference licence: permissive, reuse with attribution

- **Context**: the visual reference (PhotoCraft) is dual-licensed **MIT OR
  Apache-2.0** — permissive, not copyleft.
- **Decision**: PixMe reuses/adapts its theme code, widget code and icon
  pipeline. The licences require retaining the copyright notice in copied
  portions, so `ATTRIBUTION.md` and ported-file headers name the authors
  (ArtCraft Team / PhotoCraft contributors); that legal attribution is the one
  place the name must appear. Reused assets listed there: Inter + JetBrains
  Mono (SIL OFL 1.1), Lucide icons (ISC).
- **Consequences**: no clean-room rewrite needed for visuals; every copied
  asset keeps its licence file next to it and a row in `ATTRIBUTION.md`.

## 2026-10-24 — Theme scope: Pro (dark) + Pro Light only

- **Context**: the visual reference ships 14 themes; the user does not want a
  multi-theme PixMe. PixMe must feel like the same family. The Pro family has
  four brightness levels; the user picked the darkest one for dark mode.
- **Decision**: PixMe imports exactly **Pro** (dark; darkest Pro level:
  #323232 panels on #1E1E1E dock, #378EF0 Spectrum-blue accent, 3/4/6 px
  radii) and **Pro Light** (#F0F0F0 panels, the lightest level). Reference
  screenshots of both live outside the repo with the rest of the reference
  material. Setting `appearance = auto | dark | light`, persisted and applied
  at runtime; `auto` follows the OS appearance via winit's `system_theme` and
  falls back to dark.
- **Consequences**: `docs/theme.md` documents both palettes; a single `Tokens`
  struct with exhaustive per-theme literals makes token parity
  compiler-enforced. Region-selection dim and annotation colours are
  theme-independent constants.

## 2026-10-24 — Icon pipeline reimplemented on `egui_extras`, no `craft-ui`

- **Context**: the visual reference renders icons through
  `craft_ui::SvgIconSet`, a shared crate pulled from a git revision
  (`storytold/craft-libs`).
- **Decision**: PixMe does **not** take the `craft-libs` git dependency. The
  same mechanism is reimplemented locally (~80 lines): embedded Lucide SVGs,
  `currentColor → #ffffff` + `stroke-width 1.75` rewrite, rasterized at exact
  pixel size by `egui_extras::image::load_svg_bytes_with_size`, tinted per use.
- **Consequences**: one fewer external git dependency; the pattern is small and
  fully covered by the reference's own tests, which we adapt.

## 2026-10-24 — Fonts: Inter + JetBrains Mono embedded as TTF

- **Context**: the visual reference deflates fonts via `build.rs` for a wasm
  size budget PixMe does not have; it also has lazy CJK/system-font fallback.
- **Decision**: embed `Inter-Regular` + `Inter-SemiBold` (Medium added when a
  widget needs it) and `JetBrainsMono-Regular` with `include_bytes!`; keep
  egui's `default_fonts` as the tail of each family stack. No CJK machinery.
  Glyph coverage for `é è ê à ç ñ ¿ ¡` (+ uppercase) verified in all bundled
  faces; a unit test re-asserts coverage in CI (dev-dependency `ttf-parser` —
  tiny cmap reader, justified here).
- **Consequences**: simpler build (no `build.rs` deflate step), guaranteed
  French/Spanish rendering.

## 2026-10-24 — CI: GitHub Actions

- **Context**: the repo lives on GitHub (`github.com/maverick42/pixme`) and is
  public; the goal is a fast pipeline (feedback in minutes).
- **Decision**: `.github/workflows/ci.yml` on `pull_request` and pushes to
  `main` only, ignoring docs/markdown-only changes (but not `locales/**`).
  Parallel jobs `lint` (fmt + clippy `-D warnings`), `test` (debug, incl. the
  i18n completeness test) on a `windows-latest`/`macos-latest` matrix, and
  `build` (`cargo build --release`) only on pushes to `main`.
  `concurrency.cancel-in-progress`, `permissions: contents: read`,
  `Swatinem/rust-cache`. Toolchain pinned by a committed `rust-toolchain.toml`
  (channel `stable`, components `rustfmt`, `clippy`) so local and CI agree;
  action versions verified against their repos and pinned.
- **Consequences**: free unlimited minutes on public-repo runners; no CI test
  may need a display or screen-recording permission — those are `#[ignore]`d
  and listed as manual tests in the README.

## 2026-10-24 — Functional-reference material stays outside the repository

- **Context**: the functional reference document and its PDF are reference
  material, not project artifacts.
- **Decision**: no committed file names or describes the reference
  application; the rules point at "the functional reference (external
  document)" and the material lives outside the repo.
- **Consequences**: repo docs and rules stay self-contained; behaviour
  requirements that need the reference are copied into them in neutral words
  when they become actionable.

## 2026-10-24 — Drag-out of captures; View vs Edit modes

- **Context**: a viewed capture must be droppable into other apps. Canvas
  drag is already reserved for tools and region selection, and the viewer
  keeps the functional flow: re-selecting a region or importing the
  clipboard produces another capture rather than replacing the document.
  Actual image modification only happens in an explicit Edit mode — View
  mode is limited to navigation, export and capture-generation actions.
- **Decision**: captures open as named tabs; **dragging a tab** initiates an
  OS-level drag-out of that capture as a PNG payload (canvas is never the
  drag-out surface). The capture window has two modes: **View** (limited:
  re-select a region, clipboard import, tab drag-out, export, navigation)
  and **Edit** (annotation tools enabled). New region selection or
  clipboard import while viewing creates a new document tab.
- **Consequences**: drag-out needs OS-level drag support (winit drag-drop or
  a dedicated crate — chosen when implemented); the `tab_strip` token is
  already in the palette for the tab bar; the editor UI must gate tool
  availability on the current mode.

## 2026-10-24 — i18n stack as per project rule

- **Context**: en/fr/es support, runtime switching, completeness test.
- **Decision**: `rust-i18n` + `sys-locale` for `auto` detection, `serde`/`toml`/
  `directories` for persisted settings, `tracing` for logs — all already
  listed in rule 00; recorded here for completeness.
- **Consequences**: `locales/{en,fr,es}.yml`, English as source and fallback;
  a test fails on missing/extra keys.

## 2026-10-24 — Build profiles and test-only dependencies

- **Context**: rule 00 demands a size-tuned release binary and a fast dev
  loop, and the i18n completeness test needs to parse YAML while the font
  coverage test needs a cmap reader.
- **Decision**: `[profile.dev]` uses `debug = "line-tables-only"` and
  dependencies compile at `opt-level = 2` without debug info (they are not
  debugged). Release is `opt-level = "z"` for PixMe's own code with `s` for
  dependencies (at "z" the image codecs and rasterizer lose measurable speed
  for little size gain), `lto = true`, `codegen-units = 1`, `strip = true`,
  `panic = "abort"`. Dev-dependencies added: `serde_yml` (locale test) and
  `ttf-parser` (font glyph test) — neither ships in the binary.
- **Consequences**: `cargo test --release` cannot run (abort panics); tests
  run in debug only, which CI already does. Release binary stays small while
  keeping reasonable performance in pixel-heavy paths.

## 2026-10-24 — Floating toolbar: single viewport, settings as a second card

- **Context**: phase 2 — the floating mini toolbar plus a language and
  appearance panel.
- **Decision**: one borderless, transparent, always-on-top, non-resizable
  viewport holds the capture bar drawn as a themed `card_frame`. Opening
  Settings reveals a second card below inside the same window; the viewport
  resizes itself each frame to the painted content plus a fixed transparent
  margin (so the shadow is never clipped) via `ViewportCommand::InnerSize`.
  The window is moved to the top-center of the screen exactly once, on the
  first frame with known monitor geometry, so user drags
  (`ViewportCommand::StartDrag`, bound to the bar background and every
  button) are never overridden. Capture buttons only emit `tracing` logs
  for now; Quit sends `ViewportCommand::Close`.
- **Consequences**: a single window keeps layering and dragging trivial;
  sizing stays exact across themes and languages; the settings card reuses
  the same tokens and matches the visual reference in both themes.

## 2026-10-24 — Toolbar button set, two-window model, settings relocation

- **Context**: the functional reference's capture panel holds more buttons
  than the first cut (active window, fixed region, pin-to-screen, delay,
  capture options). The user also clarified the window model: the floating
  bar and the tabbed capture viewer never show at once — a capture or
  closing the bar hands over to the viewer, and the viewer's "+" tab brings
  the bar back.
- **Decision**: the bar exposes, left to right: Active window, Window or
  object, Rectangular region, Full screen, Fixed-size region (resizable),
  Pin to screen | Delay (0–9 s, auto-reset) | Capture options (include
  pointer, copy to clipboard) | Quit. Excluded per user: freehand,
  scrolling window, screen recorder, keep-on-top pin. Application settings
  (language, appearance) move out of the bar — they belong to the future
  viewer's settings UI; until then they are edited via `settings.toml`.
  The File menu button is deferred to the viewer phase.
- **Consequences**: `Settings` gains persisted capture options
  (`include_pointer`, `copy_to_clipboard`, `capture_delay_secs`,
  `delay_auto_reset`); the settings card is removed from the bar; rules 00
  and 10 record the two-window handoff.
