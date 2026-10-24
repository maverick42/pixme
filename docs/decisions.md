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

## 2026-10-24 — App lifecycle, OS entry points and command dispatch

- **Context**: PixMe needs the reference's two-window flow before capture
  lands, plus OS-native residency (menu-bar icon on macOS, tray on
  Windows).
- **Decision**: one shared state machine — `Idle`, `Bar`, `Capturing`,
  `Viewer` — identical on both OSes, with `Bar`/`Viewer` strictly
  exclusive. Every user action is a `Command` dispatched through one path
  (bar buttons, icon menu, app menus, future global hotkeys).
  - **Both OSes**: launch shows no window — the app is resident in the
    macOS menu bar (template status item) or the Windows tray (accent-
    coloured icon); the icon menu carries exactly "Take a capture" →
    `Bar` and "Open PixMe" → `Viewer`. Closing the bar goes to `Viewer`
    when the session has captures, else `Idle`. Closing the viewer goes
    to `Idle` (session kept). Quitting exists only in the app menus —
    PixMe menu on macOS, the viewer's own menu strip on Windows.
  - **macOS**: default `regular` activation policy kept — a permanent
    Dock icon, launcher-style (clicking it activates the app, shows
    nothing; the status item is the only entry point). The `accessory`
    policy was tried first and rejected: an accessory app can never own
    the menu bar, so the PixMe/File/… menus could not exist. Native app
    menus (PixMe / Capture / File / Window) via `muda` (`init_for_nsapp`).
  - **Windows**: tray icon via the same `tray-icon` crate, plus a
    permanent taskbar button on the invisible anchor window — the
    functional twin of the always-present Dock icon (clicking it focuses
    the app without showing anything). The viewer draws an egui menu
    strip inside the window (no global menu bar exists), dispatching
    the same commands.
  - Foregrounding: child viewports re-send `ViewportCommand::Focus`
    until their window reports focused (a `Focus` sent before the window
    is visible is dropped by winit), capped ~1.5 s so a window the user
    deliberately backgrounds is not re-focused forever. The floating bar
    is a palette: it never requests focus and rides `always_on_top`
    above the current app.
  - Menus are built with `muda` — no new dependency: it is re-exported
    as `tray_icon::menu` and powers the status item too. Menu labels
    come from i18n and are re-set on language change; `Cmd+Shift+3/4/5`
    are never used (reserved by macOS).
  - A dirty session (unsaved captures/annotations) blocks closing behind
    a translated Save/Discard/Cancel confirmation.
  - Known caveat: when the macOS menu bar is crowded, the system hides
    overflow status items — ours may be invisible until space frees up
    (the item is still installed and functional).
- **Consequences**: no extra platform dependency — the menu-bar work
  rides `muda` (re-exported by `tray-icon`) and the regular policy is
  winit's default. Viewports: the
  root is a permanent 1×1 transparent "anchor" window (`taskbar(false)`,
  no decorations) — egui/winit never repaints a window that was never
  shown, so an always-visible anchor keeps the command pump alive; bar,
  viewer and settings are deferred child viewports registered while
  their state requires them (the bar's position is persisted in
  `Shared::bar_pos` across hide/show). muda/tray handles are
  `Rc`-backed (not `Send`) so they live in a `thread_local` inside
  `platform/`, never in shared app state.
  Capture-testing on macOS will require the app packaged as a signed
  `.app` bundle (ad-hoc signature suffices) because Screen Recording
  permission binds to the application identity.

## 2026-10-24 — Dependency hygiene: `cargo audit` baseline

- **Context**: `cargo audit` on the 565-package lockfile found one real
  vulnerability (`noyalib` RUSTSEC-2026-0333, via the `serde_yml` dev-dep,
  itself flagged unsound/unmaintained) plus unmaintained warnings.
- **Decision**: dev-deps moved to maintained crates — `serde_yml` →
  `serde_yaml_ng`, `ttf-parser` → `owned_ttf_parser` (the maintained
  successor by the same author). The vulnerability is gone.
- **Consequences**: 7 warnings remain, all transitive and unactionable:
  `ttf-parser` and `unsafe-libyaml` are pulled *by* the maintained crates
  themselves; `zbus*`/`zvariant*`/`glib` belong to the Linux D-Bus stack
  of `xcap`/`arboard` (never compiled on macOS/Windows);
  `proc-macro-error` is a transitive proc-macro helper. Re-run
  `cargo audit` before releases; the advisory set lives at
  `~/.cargo/advisory-db`.

## 2026-10-24 — Region-capture overlay and xcap coordinate semantics

- **Context**: phase 3 — the functional reference's region selector
  (frozen screen, crosshair, magnifier, eight-handle selection) on top of
  the lifecycle built in phase 2.
- **Decision**:
  - `Command::Capture(mode)` → `begin_capture` parks an `OverlaySession`
    in `Shared.capture` and moves to `Capturing`. `Region`/`FixedRegion`
    spawn a borderless, always-on-top deferred viewport covering the
    monitor; `FullScreen` skips the UI entirely (the anchor pass takes
    the frame after a warm-up). Two warm-up frames run before the
    screenshot so the floating bar's window is torn down and cannot
    photograph itself.
  - The overlay paints the frozen monitor frame (a `ColorImage`
    texture), then a theme-independent dim veil — four bands leaving
    the selection untouched. Selection is drawn with a dashed accent
    border and eight handles (corner discs, edge pills); dragging
    inside moves, on a handle resizes (handles mirror across), outside
    creates. Double-click or Enter confirms; Esc cancels and returns
    to the state the capture was launched from.
  - The magnifier is a 176-pt card: a 21-px `NEAREST` zoom centred on
    the cursor with a reticle, the physical-pixel selection size (or
    cursor position), the `#RRGGBB` colour under the cursor and the
    "Press C to copy" hint (`C` or `Cmd+C` → arboard, with a "Copied!"
    flash). The card flips below-left/above near screen edges.
  - **xcap semantics discovered**: `Monitor::x/y/width/height` report
    *logical points* (`CGDisplay` bounds), while `capture_image()`
    returns *physical pixels* (= rect × `scale_factor`).
    `MonitorInfo.rect` is therefore documented and used as logical;
    conversion happens only at the physical-pixel edge (`take_capture`,
    colour sampling, loupe).
  - **macOS `visibleFrame` clamping**: `initWithContentRect` clamps a
    new window to the screen minus menu bar and Dock (observed:
    2560×1593 at (0,53) for a 2560×1707 request). The overlay sends a
    one-shot `OuterPosition`+`InnerSize` nudge after creation — macOS
    does not re-clamp programmatic frames, so the window reaches its
    true monitor size — *and* records `screen_offset = window_origin −
    monitor_origin` every frame, painting the frozen image shifted by
    `−offset` so screen point == image pixel whatever geometry the OS
    grants. Selection, loupe and hex sampling all add the offset.
  - Confirm → `take_capture` crops the frozen frame (physical px) →
    `Document::new` → `Viewer::push_document` → new tab (auto-numbered
    "Capture n", selected, marks the session dirty) → `Viewer` state.
    The viewer displays the capture at its logical size (image px ÷
    scale factor), fitted to the canvas, never upscaled.
  - `PIXME_CAPTURE=region|fullscreen|fixed` env var fires that capture
    at startup — a permanent dev/test hook that exercises the pipeline
    without the resident menu.
- **Consequences**: the capture path is exercised end-to-end; Windows
  will need the same offset logic verified against per-monitor DPI
  (DPI awareness may or may not produce the same clamping). Remaining
  modes (`ActiveWindow`, `WindowOrObject`, `Pin`) still log only.

## 2026-10-24 — Window pick via single-pass CGWindow enumeration

- **Context**: `xcap::Window::all()` enumerates every on-screen
  `CGWindow` — including the Dock, menu bar, ~20 status items,
  notification banners and Stage-Manager icons. Worse, every `xcap`
  accessor (`pid()`, `x()`, `title()`, `is_focused()`…) re-runs the
  whole `CGWindowListCopyWindowInfo` enumeration internally: ~50
  windows × ~10 accessors = O(n²) system calls, enough to freeze the
  UI for seconds when the pick overlay first hit-tests.
- **Decision**: `platform::window_list()` does ONE enumeration and
  returns `WindowRecord`s with id, pid, app name, title, bounds and
  `kCGWindowLayer` (macOS: `core-graphics`/`core-foundation` FFI — same
  crates winit already links; Windows: `xcap`, whose EnumWindows path
  is already cheap). `capture::window::windows()` keeps only layers
  0–8 (normal, floating, modal): Dock=20, menubar=24, status
  items=25, notifications (negative) are dropped; PixMe's own pid is
  excluded so the bar/overlay are never pickable.
- **Scope cut (user decision)**: the separate `ActiveWindow` mode is
  removed — the hover pick covers its use with explicit intent, and
  the toolbar keeps one window-related button. `CaptureMode` is now:
  `WindowOrObject`, `Region`, `FixedRegion`, `FullScreen`, `Pin`.
- **`WindowOrObject` mode**: frozen-screen overlay; each frame
  hit-tests the cursor against the cached window list (front-to-back,
  first hit wins); the hit rect becomes the proposed selection —
  undimmed, dashed border, no handles, pointing-hand cursor.
  Double-click or Enter captures via `xcap::Window::capture_image`
  (the window's own pixels — complete even if partially occluded),
  falling back to the frozen-frame crop; Esc cancels. Sub-object
  picking (buttons/menus inside a window) would need Accessibility
  APIs — out of scope for now.
- **i18n fix**: placeholders use `%{name}` syntax (rust-i18n 4.x), not
  `{name}` — `delay.*` and `capture.default_name` were rendering
  literally. All three locales corrected.
- **Consequences**: the pick overlay stays responsive; the freeze is
  gone. Windows needs testing on-device.
