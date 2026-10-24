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

## 2026-10-24 — Theme scope: Pro Medium (dark) + Pro Light only

- **Context**: the visual reference ships 14 themes; the user does not want a
  multi-theme PixMe. PixMe must feel like the same family.
- **Decision**: PixMe imports exactly **Pro Medium** (dark; the reference's
  default: #535353 panels, #378EF0 Spectrum-blue accent, 3/4/6 px radii) and
  **Pro Light** (#F0F0F0 panels). Setting `appearance = auto | dark | light`,
  persisted and applied at runtime; `auto` follows the OS appearance via
  winit's `system_theme` and falls back to dark.
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

## 2026-10-24 — i18n stack as per project rule

- **Context**: en/fr/es support, runtime switching, completeness test.
- **Decision**: `rust-i18n` + `sys-locale` for `auto` detection, `serde`/`toml`/
  `directories` for persisted settings, `tracing` for logs — all already
  listed in rule 00; recorded here for completeness.
- **Consequences**: `locales/{en,fr,es}.yml`, English as source and fallback;
  a test fails on missing/extra keys.
