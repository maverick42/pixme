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

## 2026-10-24 — Fullscreen capture deadlock: mutex guard held across a match

- **Bug**: a fullscreen ("capture écran") capture froze the app — the
  macOS hang report showed the main thread deadlocked on itself in
  `__psynch_mutexwait`. Interactive captures (region, window pick)
  worked, which made the cause non-obvious.
- **Root cause**: the anchor pass ran `match *lock(&self.shared.state)`.
  A scrutinee temporary lives for the whole `match`, so the `state`
  guard stayed locked while arms dispatched. `Step::Confirm` calls
  `confirm_capture` → `enter(Viewer)` → `lock(&self.state)` — a
  same-thread re-lock of a `std::sync::Mutex`: permanent self-deadlock.
  Region/pick confirms escaped it because they arrive through the
  command queue (`push` → `pump`), where `state` is not held; only
  instant captures call `confirm_capture` inside the match.
  `Step::Cancel`/`Step::Gone` had the same latent deadlock via
  `cancel_capture`/`enter`.
- **Fixes**:
  - The anchor snapshots the state (`let state = *lock(..)`) before
    matching — the guard drops at the end of the statement.
  - Same pattern fixed in `begin_capture`'s `return_to` match.
  - `enter` always wakes the anchor (`request_repaint_of(ROOT)`) even
    on same-state transitions.
  - `begin_capture` inherits `return_to` from the session it replaces
    when already `Capturing` — `Capturing` is never a return target.
  - `confirm_capture` takes the session out of the mutex before window
    capture (no window-server call under an app lock).
- **Rule of thumb**: never lock a `Mutex` in a `match`/`if let`
  scrutinee when an arm may re-lock it — bind the value first.

## 2026-10-24 — Fixed-region capture remembers its rect (session memory)

- **User requirement**: the first `FixedRegion` call shows the placement
  frame (rides the cursor until clicked, then resizable). After any
  confirmed fixed capture, the next `FixedRegion` must open the zone at
  the **same size and position** as the previous one (reference parity).
- **Implementation**: `Shared.last_fixed_region` stores the confirmed
  selection in monitor-logical space (`selection + screen_offset`).
  `begin_capture` passes it to `OverlaySession` as `restore_fixed`; the
  overlay materializes it (`translate(-screen_offset)`) once the real
  window↔monitor offset is known, clamped to the visible bounds via
  `slide_inside`. `placed` starts `true` so the frame never rides the
  cursor — it stays movable/resizable; whatever is confirmed becomes the
  new memory.
- **Scope**: session memory only (survives bar/viewer cycles, cleared on
  quit). Persisting to `settings.toml` is a possible follow-up.

## 2026-10-24 — Pin capture: autonomous floating windows, image-level export

- **User requirement**: `Pin` selects a region (prefilled with the last
  confirmed capture rect of any mode), then the cropped image opens as a
  borderless, always-on-top **floating window fully independent from the
  viewer** — "Rien n'est ajouté au viewer". Hover shows a macOS-red close
  dot, a zoom menu (50 % / 100 % / 200 %), drag grip, Copy and Save-As
  actions. All pins die with the app.
- **Implementation**: `Shared.pins: Vec<PinWindow>` lives outside the
  state machine; each pin is a deferred viewport (`pixme.pin.<id>`,
  borderless + transparent + always-on-top, no taskbar entry) registered
  by the anchor pass every frame. `Shared.last_capture_rect` (monitor-
  logical space) records every confirmed capture's rect — fullscreen
  counts as the whole monitor — and seeds the next Pin's overlay via the
  session's `restore_sel` (renamed from `restore_fixed`). `push_pin`
  positions the window at the region's on-screen spot (selection +
  monitor origin). Closing the root viewport closes every child viewport
  → pins die on Quit with no extra code; a pin's own close routes through
  `ClosePin`, which deregisters it next pass.
- **Export**: `copy_capture`/`save_capture_with_dialog` operate on a
  `Capture` (RGBA pixels via `arboard`, PNG/JPEG via `rfd`+`image`),
  separate from the `Document`-level stubs that land with the editor
  pipeline — a pin is not a document.
- **Consequences**: pins survive every bar/viewer/idle transition; multi-
  monitor correctness comes for free (all space math is monitor-logical);
  zoom changes the viewport's `InnerSize` rather than the texture, so
  scaling stays pixel-accurate.

## 2026-10-24 — Pin resize, white chrome, and a real save-as window

- **User requirements**: pins must be resizable (corner drags, aspect
  locked); the hover chrome's drag pill and copy/save buttons must be
  white with dark icons; the pin save action must open a proper dialog —
  folder + browse, format radio, file name, "show in folder" — offering
  **WEBP** where the reference offers PDF.
- **Resize**: `winit`'s `drag_resize_window` is `NotSupported` on macOS,
  so `ViewportCommand::BeginResize` is out. Instead, 20 px corner zones
  latch a resize geometrically (`pointer.press_origin` inside the zone +
  `primary_down` — no egui `dragged()` dependency, which never fired
  reliably here), then `apply_zoom` sends `InnerSize`/`OuterPosition`
  straight to the pin's own viewport — the dragged corner follows the
  pointer, the OPPOSITE corner stays bolted. Zoom clamps so no edge goes
  below 64 logical px.
- **Chrome**: `theme::pin_chrome` constants (white fill, near-black icon,
  faint border) — theme-independent like the selection veil; the zoom
  pill stays theme-tinted.
- **Save dialog**: `ui::save_dialog` is a small decorated viewport
  (`pixme.save_dialog`) holding an `Arc<Capture>` — the pin may close
  while the dialog is open. It writes via `export::file::save_capture_to`
  (new non-dialog variant), dedups file names (`capture 2.png`), strips
  path-hostile characters, and reveals the file through
  `platform::reveal_in_file_manager` (`open -R` / `explorer /select,`).
- **Formats**: `ImageFormat` gains `Webp` (lossless encoder via the
  `image` crate's `webp` feature — a feature flag on an existing dep,
  no new top-level dependency).
  Save-dialog follow-ups (same date): default file name is
  `capture`+YYMMDDHHMMSS via `chrono` (new dep: date/time formatting
  must not be hand-rolled); an already-existing destination disables
  the Save button with an inline warning instead of silently renaming;
  JPEG export flattens RGBA→RGB first (the encoder rejects Rgba8);
  the dialog floats `always_on_top` above the pins; closing it (Cancel,
  Esc, window X, successful save) wakes the ROOT anchor — forgetting to
  left the viewport registered and produced a ghost window (the same
  latent trap existed on the settings window, fixed too).

## 2026-10-24 — Viewer toolbar: file/edit actions, zoom, marquee, resize

- **The toolbar** sits between the (Windows-only) menu strip and the tab
  strip: Open, Save | Rectangle + zoom in/out + "1:1" | Draw (disabled,
  next phase), Resize | Copy, Paste | Print | Close. Exactly the user's
  list — no extras. Zoom/selection are viewer-local state (no Command
  round-trip); clipboard, file and print actions stay Commands so OS
  menus and the strip share one path.
- **Rectangle** is a marquee tool, not just a crop button: drag on the
  canvas selects (image-physical-px coords, `px_per_pt` mapping inside
  the ScrollArea), the dashed accent border + white halo mirrors the
  capture overlay, double-click inside or Enter crops. Esc / re-click
  clears. Copier prefers the live selection, else the whole document.
- **Zoom**: `Viewer.zoom: Option<f32>` — `None` = fit-to-window (never
  upscales, previous behaviour); ± buttons step ×1.25, "1:1" = actual
  size; explicit zoom shows scrollbars via `ScrollArea::both`.
- **Tab.saved** replaces the session-only dirty flag for real: captures
  and pastes open dirty, opened files clean, crop/resize mark dirty,
  Save Active (`SaveDialog.viewer_tab`) and Save All mark clean.
  `Viewer::recompute_dirty` recomputes after every transition.
- **SaveAll / ConfirmSave** write every unsaved tab to the last-used
  folder (or ~/Pictures) as `capture_YYMMDD_HHMMSS_<i>.png` — no dialog
  storm; uniqueness via index suffix.
- **Crop/resize are destructive on the base image** (Lanczos3 resample);
  annotations are dropped — the editor model has no coordinate
  translation yet (draw phase will need it or a history entry).
- **Texture cache lives on `Tab`** (not an index-keyed map): a closed
  tab can never leave a stale texture aliasing a later document.
- **Print** is v1-simple: temp PNG → `platform::print_file` (`lp` on
  macOS — CUPS fits to page; `mspaint /p` on Windows). No print dialog
  yet; margins/printer picker deferred.
- **Open** loads only compiled codecs (PNG/JPEG/WebP) via `image::open`;
  multi-select opens one tab each, title = file stem, `saved = true`.
- **File menu** gains Print (⌘P macOS, same item in the Windows strip).
- New Lucide icons: `folder-open`, `pencil`, `scaling`, `zoom-in` copied
  from the visual reference's set (ISC); `save`, `zoom-out`,
  `clipboard-paste`, `printer` added from upstream Lucide (same licence,
  added to ATTRIBUTION.md list).

## 2026-10-25 — prefs.json persistence, band toolbar with dropdown menus

- **Settings move to `prefs.json`** (was `settings.toml` in the OS config
  dir): the file lives **next to the executable** on both OSes — the
  user's portable-mode requirement (`C:\portable\pixme\prefs.json` on
  Windows; on macOS the same rule applies, except a binary inside a
  `.app` bundle cannot write beside itself and falls back to
  `~/Library/Application Support/pixme`). A legacy `settings.toml` is
  migrated once, silently. `serde_json` added (the format change needs
  a JSON codec — `toml` stays for the one-shot migration read).
- **New persisted fields**: `last_save_dir` (moved off `Shared`, where
  it was session-only) and `last_open_dir` — both feed the save dialog,
  File → Open seeding, and the new "open last used folder" item.
- **Band toolbar**: `widgets::tool_button` (18px icon over a 10pt
  caption, 56×44) replaces the compact icon row — the functional
  reference's band layout in our widget style. `has_menu` buttons
  (Ouvrir, Rectangle, zoom) get a right-edge caret zone:
  `caret_hit(&response)` distinguishes menu-open clicks from the main
  action. No keyboard shortcuts are exposed anywhere in the band.
- **Ouvrir ▾**: New (blank 800×600 canvas via `push_blank_document`),
  Open, Import from clipboard, "open last used folder" — the last item
  is disabled until a folder has actually been used once.
- **Rectangle ▾**: Rectangle / Oval / Rounded rectangle — only the
  marquee outline changes (`EllipseShape` / rounded `rect_stroke`);
  crop and copy still use the bounding rect, matching the reference.
- **Zoom**: `Viewer.zoom: ZoomMode` — Fit, FitWidth (new: fills the
  window width, scrolls vertically), or Factor. The button caption is
  the live percent (`eff_scale`, measured in `content()`); its menu
  lists actual size / fit window / fit width / 100→500% presets; the
  main click still means 100%.
- **Resize dialog**: pixels-only now (percent presets return later as
  a menu on the button); shows original/new dimensions live and keeps
  the aspect-ratio link.

## 2026-10-25 — Modal dialogs, paste-into-document, resize anti-aliasing

- **Modality**: internal dialogs (resize, rename, unsaved-confirm) are
  `egui::Modal` — dimmed backdrop that eats input, Esc/backdrop =
  cancel (confirm maps it to Cancel, the ambiguous-dismissal case).
  The separate save-as/settings *viewports* can't use Modal, so the
  viewer dims its own panel and allocates a click-eating rect while
  `Shared::modal_child_open()` — same visual+functional result.
- **Paste ≠ Import**: `Command::PasteActive` (toolbar Coller) drops the
  clipboard image onto the ACTIVE document as a floating layer —
  draggable, flattened by any outside press or Enter (`imageops::
  overlay`, alpha-composited, clamps to bounds), discarded by Esc.
  `Command::ImportClipboard` (Ouvrir ▾ / File menu) keeps its rule:
  always a NEW tab. The floating layer lives on `Tab`, so it survives
  tab switches and flattens before crop/resize.
- **Resize dialog**: gains the reference's remaining params — original /
  new dimensions with MP count and an "anti-aliasing" checkbox
  (Lanczos3 vs Nearest). Still pixels-only; percent presets stay on the
  future button menu.
- **Pixel-op undo/redo** (per tab, session-only): `Tab.undo`/`Tab.redo`
  store `PixelOp` records, not full snapshots — a flattened paste keeps
  only the covered patch + the pasted bitmap (~small), crops/resizes
  keep one pre-image (~20 MB on Retina). Capped at 10 ops/tab (worst
  case ≈ 200 MB only on a 10-full-image chain). Undo while a layer
  floats just abandons it. ⌘Z/⇧⌘Z/Ctrl+Y + native Edit menu (⌘Z/⇧⌘Z
  through AppKit); Windows strip gains an Édition menu. The stack is
  cleared when a save bakes annotations (the file then diverges from
  every stored step).
- **Paste outline = copied shape**: the paste layer records the shape
  of the last in-app copy (`copy_shape`, remembered across pastes so
  repeated Coller keeps its outline). Opaque-cornered bitmaps always
  outline as rects (external content); a transparent-cornered bitmap
  with no recorded shape probes the top edge to tell oval from round.
- **Select button**: the former "Rectangle" tool button is renamed
  "Sélection" (the menu items keep their shape names).

## Draw (Dessiner) annotation editor — architecture

- **Deps**: `imageproc` (default-features off, `text` feature only) + `ab_glyph` — rasterizing annotations (strokes, fills, flood fill, text, blur) to `image::RgbaImage`. One render path for canvas/viewer/export (`editor::render::render_to_image`) so WYSIWYG holds by construction.
- **Model**: `Document.annotations: Vec<Annotation>` (non-destructive, z-ordered, stable ids); `AnnoEdit` (Add/Remove/Replace) is the only mutation channel — chronological undo/redo stacks live on the viewer `Tab` (`anno_undo`/`anno_redo`, cap 50) so they survive Draw close/reopen.
- **Session**: `DrawSession` clones the document + stacks at open; OK writes them back (`Viewer::commit_draw`), Annuler/close drops the clone — the tab is untouched.
- **Flatten rule**: viewer pixel ops (paste-flatten, crop, resize, opening Draw with a floating paste) bake annotations into `base.image` via `Tab::flatten()` and clear the annotation stacks — objects become non-reselectable by design.
- **Eraser** is an opaque-colour stroke object (stays selectable/undoable); **Fill** stores its flood-fill mask computed at placement; **Blur** is a parameterized region object (Blur/Pixelate × Rect/Round/Oval).
- **Stamps** recolour embedded Lucide SVGs (`stamp_icon` map + `raw_svg` accessor) — no third-party assets.
- **Options bar**: per-tool fields mirror the reference's bottom bar; the Select tool additionally exposes the selected object's own style fields (re-editable).
- Text is rendered with the embedded Inter faces (bold → SemiBold, italic → faux shear), outline via 4-direction offset pass.

## 2026-10-25 — Tab context menu, capture metadata, screen-lock protection

- **Context**: captures open as tabs and needed the reference's
  right-click options menu (save/rename/delete-file/close-*/duplicate),
  plus a rare "Protection" option locking a tab behind a password, and
  per-capture metadata (internal id, capture date, save date, source
  path) preparing a future database restore of captures.
- **Decision**:
  - `Document` gains `meta: CaptureMeta` (`id`, `captured_at`,
    `saved_at`, `source_path`, `password_hash`) so the record travels
    with the document through Draw's clone/commit and duplication —
    the unit the future DB stores. `saved_at.is_some()` means "saved
    at least once"; every save path (`mark_saved`, `save_all`, Open)
    refreshes `source_path`/`saved_at` (Open takes the file mtime for
    `captured_at`).
  - **No new dependency** (user's call): `editor::meta::new_id()` mints
    a UUIDv4-shaped string from `RandomState` entropy + nanos + counter
    plus pid; `hash_password` is a salted iterated FNV-1a stored as
    `v1:salt:hash`. The lock is a *screen lock*, not encryption — pixels
    stay in memory and saved files are unprotected.
  - Tab strip: red dot on the left marks unsaved-or-modified tabs
    (reference-style indicator), a small padlock marks protected tabs,
    a 14 px `×` closes the tab. `egui::Popup::context_menu` gives the
    right-click menu (handles macOS Ctrl+click natively); no keyboard
    shortcuts are shown or bound.
  - **Closing a tab never warns** (user decision): the red dot is the
    warning. The session-level Save/Discard/Cancel confirmation still
    guards closing the whole viewer / quitting.
  - Unique names everywhere: `unique_title` appends `(n)` on clashes
    (push, duplicate), Rename validates case-insensitively against the
    other tabs.
  - **Protection**: menu-only entry ("Protection…"), per the feature's
    rarity. Set = password + confirmation; on a protected tab the
    current password must be proven, then a new pair re-arms or a blank
    new password disarms. The lock re-arms the moment the tab loses
    focus (`set_active` clears `unlocked`); a locked tab shows a centred
    password card instead of the canvas. Per user decision the lock
    only gates the *display* — copy/save/print/draw stay possible.
  - "Supprimer le fichier" deletes the tab's `source_path` behind a
    confirmation; the tab stays open, unsaved again.
- **Consequences**: `uuid`/`sha2` were considered and rejected — binary
  stays lean; the lock offers casual privacy only (documented in
  `editor/meta.rs` docs). Duplicate keeps the password hash but gets a
  fresh id/date. Drag-out payloads of a locked tab remain unrestricted.

## 2026-10-25 — rusqlite for session persistence

- **Context**: the session (viewer tabs: documents + annotations +
  undo/redo stacks) must survive quitting PixMe, per user decision.
  A small embedded database is wanted — not a large store, no pixel-op
  history.
- **Decision**: `rusqlite = { version = "0.41", features = ["bundled"] }`.
  Bundled compiles SQLite statically: no system dependency, identical
  behavior on macOS and Windows, single `session.db` file under
  `data_dir()`. `cargo audit` after the add: zero vulnerabilities; the
  7 reported warnings are all pre-existing transitive crates
  (unmaintained `proc-macro-error`, `unsafe-libyaml`, `ttf-parser`,
  `zbus*`; unsound `glib` — Linux-only code paths). `bundled` also
  enables `ffi-sqlite-wasm-rs` (sqlite-wasm-rs/rsqlite-vfs in the
  lockfile): wasm32-only crates, never compiled on our targets.
- **Consequences**: ~1-2 MB binary growth. Persistence writes only at
  app quit (one transaction; crash/kill loses the session — accepted
  by the user). Restore is silent when the viewer next opens.

## 2026-10-25 — session.db: what is stored, when it is written

- **Context**: the reference tool loses every capture + annotation at quit;
  PixMe's model is already non-destructive (a `Document` = base
  `Capture` + `Vec<Annotation>`, edits flow through `AnnoEdit`), so a
  full-fidelity restore is cheap to reach.
- **Decision** (`src/store/`):
  - One `documents` row per tab: `doc_id` (`meta.id`), title, saved
    flag, all `CaptureMeta` columns (captured/saved dates as RFC 3339,
    source path, password hash), the base image as a PNG blob, scale
    factor, monitor JSON, `next_id`, plus three postcard blobs —
    `annotations`, `anno_undo`, `anno_redo`. A `session` row keeps the
    active tab and the auto-title counter (`Capture N`).
  - `postcard` (default-features off, `alloc` + `use-std`) serializes
    the blobs compactly — JSON would inflate every embedded PNG byte
    ~3-4×; `serde_bytes` was avoided with a 20-line visitor instead.
    All `*Obj` types derive `Serialize`/`Deserialize` (egui types
    already serde through eframe `persistence`); only the two
    `RgbaImage` fields (`PicObj.image`, `FillObj.mask`) go through
    `#[serde(with)]` PNG helpers.
  - **PixelOps (Resize) are not persisted** — their payloads are whole
    pre-op images; the user wants a small database. After a restart,
    annotation undo survives, pixel undo does not.
  - **Writes happen at three points only**: app quit (one transaction:
    delete-all + insert-all, plus committing any open Draw session
    back into its tab first), tab close (the document row is deleted
    at once — "a closed capture stays closed"), session discard
    (store cleared). Crash/kill loses the delta — accepted.
  - **Restore is lazy and silent** on the first `enter(Viewer)`:
    launch stays resident/instant; every restored protected tab comes
    back LOCKED (`unlocked` is runtime-only); an empty restore never
    touches `dirty`.
  - Corruption: `meta.schema_version` guards the layout — a garbage
    file or a foreign version is renamed `session.db.bak` and rebuilt;
    a per-row decode failure skips just that tab; `conn: None` =
    degraded mode (persistence off, app keeps running).
- **Consequences**: enum layout changes in `AnnotationKind`/`AnnoEdit`
  must bump `SCHEMA_VERSION` (postcard encodes variant indices) — the
  DB is a convenience cache, wiped rather than migrated. An open Draw
  session is committed at quit, so mid-edit quits keep the edits.

## 2026-10-25 — session.db hardening

- **Context**: the persisted session holds real captures — possibly
  sensitive content — and several operational gaps needed policy:
  deleted-data remanence, `.bak` lifetime, store size, second process.
- **Decision**:
  - `PRAGMA secure_delete = ON` at every open: SQLite zeroes freed
    payload bytes instead of leaving them in free pages.
  - **VACUUM policy**: immediate `VACUUM` inside `clear()` (Discard is
    one rare user action on an emptied DB — cheap); per-tab deletions
    only set `needs_vacuum` and the `Drop` impl vacuums then — a
    per-close VACUUM would rewrite the whole file on every tab close,
    and `secure_delete` already zeroes content meanwhile. Plain quits
    with no deletions skip VACUUM entirely.
  - **`.bak` policy**: a corrupt/foreign file is parked as
    `session.db.bak` for manual recovery, then auto-deleted once its
    mtime is older than 7 days (`BAK_KEEP_DAYS`) — checked at every
    store open, so captures in it cannot linger forever.
  - **Limits — REMOVED**: early caps (`MAX_TABS`, `MAX_SESSION_BYTES`,
    `session.dropped` + status notice) were cut at the user's request —
    the app never deletes captures for size reasons, so schema v4 drops
    the `dropped` column and every tab is always persisted.
  - **Single instance**: `single-instance 0.3` — named mutex on
    Windows, `flock`ed file on macOS, keyed on
    `data_dir()/instance.lock` (backslashes→`/` for the Windows mutex
    name rule). A second process gets a translated `rfd` warning
    (`app.already_running`) and exits; a guard *failure* logs and lets
    the app run (a resident capture tool must not be blocked by it).
  - **Schema v2**: the `dropped` column bumps `SCHEMA_VERSION` to 2 —
    v1 files are wiped/rebuilt, never migrated.
  - **Golden test**: `postcard_layout_is_golden` freezes the postcard
    encoding of a fixed annotation set + undo/redo stacks; any drift
    fails with a message pointing at `SCHEMA_VERSION`.
  - **Lock guarantee (restated precisely)**: the protected-tab
    password is a *screen lock* only — it gates the DISPLAY of the tab
    (canvas + strip replaced by the unlock card, re-armed on every
    tab switch). It protects nothing else: pixels stay readable in
    memory; copy/save/print/draw/drag-out work on a locked tab; the
    file on disk (`source_path`) is unprotected; `session.db` stores
    pixels and the password hash plainly. The hash is salted iterated
    FNV-1a — deliberately non-cryptographic. Casual privacy only; no
    encryption unless the user asks for it.
- **Consequences**: `nix`/`widestring`/`winapi` enter the lockfile via
  `single-instance` (audit clean). `single_instance` has no callback
  API — a second instance cannot raise the first, it only exits.

## 2026-10-25 — session-database retention setting

- **Context**: persisting every capture forever was too crude; the
  user wants control over how long the database keeps them.
- **Decision**: `settings.db_retention` (`never | today | always`,
  default `always`), a third dropdown row in the settings window.
  `never` skips the quit-time write entirely; `today` deletes every
  row whose `captured_epoch` predates local midnight — applied both
  at store open (launch-time prune) and immediately when the setting
  is confirmed. Switching to a MORE restrictive option while the
  store holds captures parks the choice in egui temp state and asks
  for confirmation ("captures in the database will be lost"); Cancel
  reverts the shown value untouched. The `captured_epoch` INTEGER
  column (schema v3) exists because RFC 3339 strings cannot compare
  across offsets.
- **Consequences**: `document_count()` and `apply_retention()` on the
  store; schema v3 wipes v2 files (still a cache, never migrated).
- **Tab sort**: `settings.tab_sort` (`None|name|date`) persists the tab
  ordering chosen from the tab context menu — deliberately menu-only,
  never in the settings window. `Tab.insertion` keeps the original
  order restorable; re-sort happens on push, rename, duplicate, and
  restore. Numeric-aware compare keeps `Capture 9` before `Capture 10`.

## 2026-10-26 — Mosaic view mode: a runtime-only thumbnail grid

- **Context**: the viewer shows one capture at a time behind a tab
  strip; the user wants an overview mode — a grid of thumbnails with
  the tab strip hidden — whose zoom matches the reference's thumbnail
  strip, without ever uploading every full-resolution capture texture
  to the GPU at once.
- **Decision**:
  - `Viewer.view_mode: ViewMode` (`Tabs` default | `Mosaic`) — pure
    runtime state, never persisted: PixMe always opens in Tabs mode.
    The toolbar's new `LayoutGrid` button toggles; in Mosaic the tab
    strip is hidden and a trailing "new capture" tile (same
    `Command::ShowBar` as the "+" tab) ends the grid.
  - `mosaic_scale: f32` (1.0 default, 0.5–2.0): the ± zoom buttons
    step ×1.25/÷1.25, the percent button resets to 100%, its caret
    menu lists 50/75/100/125/150/200%. Base tile ≈ 220×160 pt; the
    column count adapts to the available width inside a vertical
    `ScrollArea`.
  - `Tab.thumb: Option<(revision, TextureHandle)>`: a per-tab,
    lazily-built thumbnail cache — `render_to_image` downscaled to
    ≤480 px on the longest side (Lanczos3, aspect kept), generated
    only while the tile is visible (`ui.is_rect_visible`) and only
    when the document revision changed. The full-resolution
    `Tab::texture` is never uploaded for the grid; the thumbnail is
    displayed fit-and-centred ("fit", never distorted).
  - Protected tabs never generate or display a thumbnail: `set_active`
    and the protection dialog clear a protected tab's cached thumb; a
    locked tile shows a centred padlock on `t.card`. Double-clicking a
    locked tile selects it and returns to Tabs mode, where the usual
    password card appears. Opening the already-active tile does not
    re-lock it (not a selection change).
  - Tile interactions: single left click selects (toolbar actions and
    ⌘C/⌘Z/⇧⌘Z keep working on the selection), double-click or Enter
    opens the active tile in Tabs mode, right-click opens the existing
    tab context menu (its sort zone reorders the grid immediately). A
    dark caption band (`from_black_alpha(165)`, always visible) shows
    the unsaved dot, the padlock and the elided title; hovering adds
    the capture date (same locale format as the status strip) plus a
    red-hover close cross and a grip-vertical anchor in a dark badge
    (visual + reserved interaction zone with a translated tooltip —
    the OS drag-out lands in a later unit).
- **Consequences**: thumbnail GPU memory is bounded (~0.9 MB per
  480 px texture) and proportional to visible tiles, not tabs; no new
  dependencies; thumbnails reuse `render_to_image`, so they are
  WYSIWYG with annotations by construction. `Viewer::default` is now a
  manual impl (`mosaic_scale` starts at 1.0, not 0.0).

## 2026-10-26 — Area highlighter options and SDF rendering

- **Context**: The area highlighter (`Tool::HighlightArea` / `AnnotationKind::HighlightArea`) exposed unused border and soft-edge checkboxes in tool mode, hid its shape selector once an object was selected, drew a rounded-rectangle preview regardless of the active shape during drag, and rasterized through integer polygons instead of the analytical SDF path.
- **Decision**: Share a single option strip (`highlight_area_options_row`) between `Tool::HighlightArea` and a selected `AreaObj` exposing only colour, shape (`Rectangle` / `Circle`) and percentage opacity (`10..=100%`, defaulting to `hl_opacity = 45%`). Render `HighlightArea` via `draw_shape_sdf` (sharp rectangle vs analytical ellipse) and match the live drag preview and selection outline to the chosen shape.
- **Consequences**: Creating or editing an area highlight shows the exact same controls, respects the selected shape during drag, selection and export, and eliminates the opacity jump on creation.

## 2026-10-26 — Draw tool set trimmed (user decision)

- **Context**: The user asked to remove four draw tools entirely: the line
  highlighter, the bucket fill, the stamps/cursors picker, and the fancy
  arrow.
- **Decision**: Deleted `Tool::HighlightLine`, `Tool::Fill`, `Tool::Stamps`,
  `Tool::FancyArrow` and their annotation kinds (`HighlightStroke`, `Fill`,
  `Stamp`, `Fancy`) plus all supporting code (`StampId`, `FillObj`,
  `StampObj`, `FancyObj`, `draw_fancy`, `draw_stamp`, `apply_fill`,
  `compute_fill_mask`, `stamp_icon`, `raw_svg`) and the now-unused icons
  (12 SVG assets removed). The area highlighter keeps its slot and now
  uses the `Highlighter` pen icon. The marquee's own "fill" button is
  unaffected — it paints a plain `RectObj`.
- **Consequences**: `SCHEMA_VERSION` bumped to 11 — annotations using the
  removed kinds could not be deserialized, so older session stores are
  wiped on open (accepted: pre-release). The strip now shows 16 tools.

## 2026-10-26 — Bracket / curly-brace tool (`Tool::Brace` / `AnnotationKind::Brace`)

- **Context**: The user asked to add a bracket / curly-brace tool ("Accolade")
  between Pencil and Eraser in the Draw tool strip, inspired by the functional
  reference: dragging draws a visible line that sets the bracket's direction and
  orientation, and releasing displays the bracket with two endpoint anchors to
  resize or re-orient it.
- **Decision**:
  - Added `Tool::Brace` (`Icon::Braces`, upstream Lucide `braces.svg`) between
    `Tool::Pencil` and `Tool::Eraser` (17 tools in the strip).
  - Added `BraceKind` (`CurlyLeft`, `CurlyRight`, `CurlyWideLeft`,
    `CurlyWideRight`, `SquareLeft`, `SquareRight`, `RoundLeft`, `RoundRight`)
    and `BraceObj` (`a`, `b`, `kind`, `color`, `width`, `dash`, `outline`,
    `outline_color`, `opacity`, `shadow`) under `AnnotationKind::Brace`.
  - While dragging (`Drag::Shape`), the canvas previews the straight segment
    between `a` and `b` (with Ctrl/Shift horizontal/vertical snapping). On
    release, the bracket is created and selected, exposing two circular
    endpoint anchors at `a` and `b` (`Handle::Endpoint`) without a bounding-box
    frame.
  - `render::brace_polyline` generates `C¹`-smooth quarter-ellipse hooks and
    central cusp for curly braces (`{` / `}`, narrow and wide), right-angle
    segments for square brackets (`[` / `]`), and a circular arc for
    parentheses (`(` / `)`), rendered through the analytical SDF stroke path
    (`stroke_freehand`).
  - `brace_options_row` shares one option strip between `Tool::Brace` and a
    selected `BraceObj` (colour, width, 8 painted `Forme:` buttons, dash,
    outline width + colour, opacity, shadow).
- **Consequences**: `SCHEMA_VERSION` bumped to 12 in `src/store/mod.rs`.

## 2026-10-27 — Modal windows gate commands via one eligibility table

- **Context**: on macOS the global menu bar stays fully active while a
  modal window (Settings, save-as dialog, Draw editor) is open — « Tout
  enregistrer », printing or captures could fire mid-settings. Windows
  has no global menu bar but the tray menu and (later) global hotkeys
  reach the same commands.
- **Decision**: one pure table — `app::is_command_enabled(command,
  modal_state)` in `src/app.rs` — is the single source of truth. It is
  an exhaustive `match` (no wildcard), so a new `Command` variant
  without a ruling fails to compile. `ModalState` (`None`, `Settings`,
  `SaveDialog`, `Draw`) is computed by `Shared::modal_state()`. Three
  consumers read it:
  - **Menu greying**: `platform::sync_menu_enabled` maps each item id
    through `command_from_menu_id` and calls `set_enabled` — covers the
    macOS app menu bar, the macOS status-item menu and the Windows tray
    menu. `Shared::pump` diffs `modal_state()` every pass and re-syncs
    on ANY transition (window X, Esc, programmatic close), then requests
    repaints on root, viewer and bar so egui surfaces reflect it too.
    All `set_enabled` calls run on the main thread: `pump` only runs
    inside viewport passes and the muda handles live in a thread_local.
  - **Dispatch safety net**: `dispatch()` drops any forbidden command
    before matching, so a stale menu event, a queued click or a global
    hotkey can never bypass the table.
  - **Widgets**: the Windows egui menu strip and the floating bar's
    buttons are drawn disabled through the same `command_enabled`.
  - Rulings: `Quit` is always live — it closes Settings/save-as modals
    first (an open Draw editor is committed by `save_session`, as
    before) then runs the existing Save/Discard/Cancel flow.
    `OpenSettings` while Settings is already open re-focuses the modal
    instead of re-opening. Under Draw, `UndoActive`/`RedoActive`/
    `CopyActive` route to the Draw session (existing dispatch arms) —
    under Settings or save-as they are blocked so they can never act on
    the background document; modal text fields keep native egui editing
    since those keys never become `Command`s. Confirmation answers and
    capture-overlay outcomes stay enabled (the modal itself pushes
    them); pin close/copy stay enabled (pins are autonomous); `SavePin`
    is blocked while a modal is up (no stacked modals).
  - **Draw is a real modal**: deferred viewports are plain top-level
    windows (no OS child-window modality on either OS), so the viewer
    pass steals focus for `draw_id` the moment it gains any — clicking
    or raising the viewer puts the editor back on top a frame later;
    closing Draw hands focus back to the viewer.
  - **Windows findings** (requested verification): (a) there is NO
    OS-level modality — egui child viewports are ordinary top-level
    windows and no `EnableWindow`/owner-disabling exists; the parent is
    blocked only by the egui dim overlay, the focus-steal and the
    dispatch guard.
    (b) The Windows notification-area icon exists by spec (rules 00 /
    decision 2026-10-24: "Windows tray + taskbar button", accent-coloured
    icon) and its menu carries exactly two entries: "Take a capture" and
    "Open PixMe" — both now grey out under a modal.
- **Consequences**: menu-enable state can never diverge from dispatch
  behaviour; every future `Command` must rule itself in the table.
  Manual coverage: MT-28 in `docs/audit/AUDIT_PROCESS.md`.

## 2026-10-27 — Settings window: sidebar pages, draft + OK/Cancel, filename templates

- **Context**: the settings sheet was a single column applied live; the
  functional reference has several settings categories and an editable
  filename-template system (batch-save names) the user wants.
- **Decision**:
  - The modal now has a left **sidebar** (`General`, `File name` —
    `SettingsPage`, extensible), the page's rows on the right and an
    **OK / Annuler** footer. Editing works on a cloned draft
    (`Shared.settings_draft`); `OK` writes it back + persists, while
    Annuler / the cross / Escape discard — including preset edits.
    The restrictive-retention warning moved from pick-time to OK-time
    and parks the whole draft (Cancel reverts only the retention field).
  - `Settings` gains `filename_template` (default
    `$Y-$M-$D_$H$N$$`), `filename_templates` (6 presets matching the
    reference, editable inline via +/− — no third window) and
    `filename_counter` (sequential `#`, first write yields `…001`).
  - `config::expand_template` handles `$Y/$M/$D/$H/$N/$S` (chrono),
    `#` runs as a zero-padded sequential, `@` runs as random
    alphanumerics (tiny xorshift — no new dependency) and `%VAR%` with
    cross-OS aliases (`%USERNAME%`→`USER|USERNAME`,
    `%COMPUTERNAME%`→`COMPUTERNAME|HOSTNAME`); unknown codes and unset
    vars expand to nothing/literal.
  - `save_all` now names files through the template (`unique_path`
    appends `_2`, `_3`… on collisions, e.g. templates without `#`);
    the counter persists only when files are actually written and the
    settings-page example never consumes it. The save-as dialog still
    proposes the tab title.
- **Consequences**: all changes live in `prefs.json`; tests cover the
  template expansion (dates, padding, `@` charset, `%VAR%`, literals).

## 2026-10-27 — Quit-time persistence is signature-gated and threaded

- **Context**: every quit ran `save_session` synchronously — a full
  `DELETE` + re-INSERT transaction that PNG-encoded every tab's image
  on the UI thread, even when nothing had changed. Many captures made
  the exit hang for seconds.
- **Decision**:
  - `viewer.sigs()` hashes two fingerprints: `content` (per-tab doc id,
    `doc.revision`, next_id, meta — order-insensitive sum) and `meta`
    (ordered id/title/saved + active + next_title). The baseline is
    recorded in `persisted_sigs` at restore. Invariant: every persisted
    mutation must bump `doc.revision`.
  - `quit_now` picks the cheapest correct path: identical signatures →
    close with no write at all; identical content → `Store::save_meta`
    (UPDATEs only: `ord`/title/saved per row + session row); real
    change → the tabs and the Store ride a worker thread, the viewer
    paints an opaque saving veil (`paint_loading` spinner +
    `quit.saving`) and `pump()` closes the app when the store returns.
  - `ModalState::Saving` disables every command during the write —
    even a second Quit.
  - Session PNG blobs encode with `CompressionType::Fast` — PNG is
    lossless, only size/encode time change.
- **Consequences**: an unchanged quit is instant; a meta-only drift is
  sub-millisecond; a content rewrite keeps the window up with a
  spinner instead of freezing silently. Tests cover the unchanged-skip
  and the threaded path (`drain_quit`/`drain_restore` helpers).

## 2026-10-28 — One modal chrome for every in-app dialog

- **Context**: in-app dialogs had drifted apart: the Redimensionner
  dialog used a card-tinted title band with a painted close cross,
  others had a plain title + hairline, the dup-names notice still used
  the native `egui::Window` chrome, and button rows mixed left-aligned
  and split layouts. Menu commands also stayed live behind in-viewer
  dialogs (only top-level modal surfaces were gated).
- **Decision**:
  - New shared chrome in `src/ui/widgets/modal.rs`: `modal_frame`
    (`egui::Modal` + dock fill + card border + `radius_lg` + card title
    band with the red-hover painted cross, 14 px body margin),
    `close_cross`, `modal_buttons`. The cross click is reported via
    `ModalResponse.inner.0`; backdrop click and Escape still go through
    `should_close()`.
  - The 7 in-app dialogs are all converted: resize, rename, protection,
    delete-file, unsaved confirm, dup names (dropped `egui::Window`),
    and the Draw editor's quit warning. Every action row is
    right-aligned with the PRIMARY (blue) button at the far right;
    destructive answers (Jeter/Ignorer) sit leftmost of the group.
  - `ModalState::ViewerDialog` greys menus behind in-viewer dialogs.
    `modal_state()` must not lock the viewer (viewer.show calls it
    while holding the lock — would self-deadlock), so the flag lives
    in `Shared::viewer_dialog_open` (atomic), synced every viewer pass
    from `Viewer::dialog_open()` (one frame of lag for menu greying is
    acceptable). Unlock/pin screens are not dialogs and stay exempt.
- **Consequences**: identical header band, colour, red close cross and
  `[Annuler][OK]` ordering in every modal; OS menus, tray menu, egui
  menu strip and dispatch all block behind any of the 7 dialogs
  (confirmation answers and Quit stay live, matching the existing
  rulings). The modal-command table test gained the ViewerDialog
  column (same expectations as SaveDialog).

## 2026-10-28 — Mosaic thumbnails: box-average downscale, shared scratch layer, revision-fed queue

- **Context**: opening the mosaic with 20 restored 5K captures froze
  the UI for ~3.2 s — every visible thumbnail was built synchronously
  in one frame. Measured breakdown per thumbnail (release): Lanczos3
  resample ~200 ms, `render_to_image` 8–18 ms bare / 264–405 ms
  annotated, texture upload ~0.1 ms.
- **Decision**: (A) `imageops::thumbnail` box-average replaces
  `resize(Lanczos3)` — ~200 → ~38 ms per 5K image, the right
  quality trade for ≤880 px tiles. (B) documents without annotations
  skip the full-resolution clone entirely. (E) layered annotations now
  rasterize into ONE shared full-frame scratch layer per
  `render_to_image` call (re-cleared only inside the painted bounds),
  and only the object's padded bounds get composited — pixel-exact
  parity with the old per-object full-frame layer, enforced by a
  `#[cfg(test)]` oracle implementation and a parity test battery
  (zero tolerance; a first translation-based variant was REJECTED —
  f32 non-associativity produced ±1 channel drift on AA edges).
  (C) a `thumb_queue` fed solely by revision drift
  (`sync_thumb_queue`, the single invalidation point) builds ≥1
  thumbnail per frame under an ~8 ms budget, visible tiles first via
  `prioritize_thumb`, prefetches the rest, and only calls
  `request_repaint` while work remains. Thumbnail resolution is fixed
  (880 px) so display zoom never invalidates the cache.
- **Measured after**: annotated-document `render_to_image` 264–405 →
  144–264 ms; unannotated thumbnails ~37–55 ms end to end; all 20
  tiles fill in ~3 s of background frames instead of one 3.2 s freeze;
  CPU returns to 0 % when the queue drains.
- **Consequences**: a single annotated-5K build still costs ~180–310 ms
  (one `render_to_image` cannot be sliced below itself on the UI
  thread), so frames during *pre*fill can still exceed the 100 ms
  target — background threads (option D) stay a justified candidate.
- **Future tracks (not implemented)**: decoded-image memory (~1.2 GB
  for 20×5K) — lazy base decoding; persisting thumbnails in a
  compatible `thumbs` table (no `SCHEMA_VERSION` bump, lazy load,
  same secure-delete purge as the session); `image` release
  `opt-level = 3` measured — no gain, rejected.

## 2026-10-28 — Thumbnails on a background worker; dev-tools Cargo feature

- **Context**: after A+B+E+C, single-build frames during thumbnail
  prefill still reached ~315 ms on the UI thread (one
  `render_to_image` cannot be sliced below itself). Edit-path
  suspicion confirmed as worth solving: any revision bump re-queued a
  ~180–310 ms UI-thread build next frame.
- **Decision**: one worker thread (`std::thread`, no new dependency)
  rasterizes thumbnails; the UI only dispatches jobs and stores
  results. `Capture.image` became `Arc<RgbaImage>` so a `Document`
  clone for a job shares the base image — only annotations copy.
  Jobs are `(tab id, revision)`; one job in flight; results whose
  revision no longer matches, or whose tab closed, are dropped.
  Worker panics are caught, logged, and the loop continues; a dead
  channel respawns the worker once. `request_repaint` is UI-side
  while work remains — `Context::request_repaint` from the worker
  thread did NOT wake the event loop in this eframe integration
  (verified empirically: the pipeline stalled after one job).
  Thumbnail textures get `mipmap_mode` (egui 0.36 supports it via
  `TextureOptions::mipmap_mode`) so far-zoomed tiles stay clean.
- **Measured (release, dev-tools)**: 20 thumbnails build in ~3 s of
  background work, UI never blocks; visible tiles ready at 1.28 s;
  annotation commit op 0.03 ms, undo/redo ~0 ms, resize op 877 ms
  (intrinsic Lanczos3 5K→4K cost — NOT thumbnail-attributable);
  CPU 0.0 % at rest. Frames >100 ms attributable to thumbnails: none.
- **dev-tools feature** replaces `cfg(debug_assertions)` gating:
  `--seed-session`, `PIXME_DEBUG_MOSAIC`, `PIXME_DEBUG_EDIT`, and the
  `PIXME_DATA_DIR` data-dir override exist only in
  `cargo build --features dev-tools` builds; `PIXME_DATA_DIR` makes
  seeding unable to touch a real session.
- **Consequences**: memory bound = 1 in-flight `Document` clone
  (Arc-shared base) + 1 result `ColorImage`; no unbounded queue of
  pixel data. The in-flight job may compute a stale result (dropped).
- **Future tracks (not implemented)**: persisted thumbnails in a
  compatible `thumbs` table (no `SCHEMA_VERSION` bump), lazy base
  decoding for decoded-image memory.

## 2026-10-28 — Emoji sticker tool (Noto Color Emoji PNGs, Apache 2.0)

- **Context**: the Draw editor needed an emoticon tool. The reference's
  `Draw.db` holds 138 embedded PNGs, but its assets are proprietary and
  the project rules forbid reusing them. Emoji-as-text was rejected:
  Inter has no emoji glyphs and egui's rasterizer (ab_glyph) cannot
  render colour fonts (COLR/CBDT/sbix).
- **Decision**: `AnnotationKind::Emoji(EmojiObj)` stores the emoji id
  (Unicode code point, u32) — never the pixels — appended LAST in the
  enum so postcard blobs written before the variant still decode (no
  `SCHEMA_VERSION` bump; verified by `pre_emoji_session_blob_still_
  deserializes`). Rendering resolves id → lazily decoded `RgbaImage`
  (per-entry `OnceLock`, thread-safe for the thumbnail worker) and
  reuses the same Lanczos3+composite path as `Picture`; unknown ids
  draw a neutral placeholder (dashed square + cross), never an error.
- **Asset provenance**: Noto Color Emoji PNG 128 px from
  googlefonts/noto-emoji, pinned to commit
  `41e31b110b4eb929dffb410264694a06205b7ad7` — the last commit whose
  root LICENSE is literally Apache 2.0 (image resources per README).
  UPSTREAM INCONSISTENCY ON RECORD: commit `254596e5f8` (2024-07-29)
  switched the root LICENSE to OFL while the README still claims image
  resources are Apache 2.0 — the pre-change commit keeps our paper
  trail unambiguous. No NOTICE file exists upstream (AUTHORS/
  CONTRIBUTORS only). 64 curated single-code-point ids
  (`assets/emoji/manifest.txt` + `emoji_u*.png`, build.rs-generated
  table — one file + one line per new emoji, permanent ids).
- **Size budget**: ≤ 350 KB embedded (measured ~290 KB raw at the pin;
  no oxipng — vendored verbatim, per user decision). Session impact: ~6 bytes per emoji
  vs a full PNG for `Picture`.
- **Interaction**: egui's own 6 pt `max_click_dist` is the click/drag
  threshold; click = 5 % of the image's small side bounded to
  [24, 96] × `scale_factor` (the Step-badge footprint family — Step
  itself uses FIXED 12/18/26 px radii, so on a Retina ×2 frame a Step
  looks half-size; emoji compensates instead of copying that bug);
  drag = directional square (aspect-locked, also on resize), capped at
  256 px (2× the 128 px source — beyond, the square just stops
  growing); last dragged side remembered per session; ghost preview
  under the cursor; sticky tool, Esc cancels an in-flight drag then
  exits to Select. Arrow keys nudge the selection (1 px, ⇧ = 10 px),
  a whole burst committing as ONE Replace.
- **Deferred**: tabbed categories/search/tooltips (option B) stay
  possible later without format change — the manifest already defines
  order and sections.
  - **Follow-up (same day)**: the picker became section tabs in the
    bottom bar (one translated chip per manifest section —
    `EmojiDef.section` parsed from the nearest `#` comment — cells of
    the active tab only), icons only: no opacity/shadow fields on
    `EmojiObj`. Cells are click-to-arm AND drag-out-to-canvas (drop =
    place at default side), matching the reference's emoticon strip.

## 2026-10-28 — Deferred-render drags (region-bounded re-render)

- **Cause**: `Drag::Moving`/`Resizing`/`Endpoint`/`Tail` bumped
  `doc.revision` per pointer move → `canvas()` ran a FULL
  `render_to_image` + `tex.set` per frame. Measured on a 5K
  (5120×2880) doc with 16 annotations, release: `render_to_image`
  ~310 ms + `ColorImage` ~14 ms → ~330 ms/frame — the drag stutter.
  Thumbnail-queue suspicion disproven by code: `DrawSession` edits a
  `doc.clone()`; `sync_thumb_queue` only reads the viewer tabs.
- **Fix**: doc + revision stay frozen for the whole gesture. The drag
  mutates a `live` clone; the canvas texture is patched by
  `render_region` (same draw fns/scratch/composites — bit-identical to
  the full render's crop, pinned by seeded parity tests at tolerance
  0), the overlay paints `live` via `DragPreview` (same pipeline,
  ~6 ms/frame). Release = ONE `Replace` + ONE region patch (union of
  old+new bounds). Esc re-patches the exclusion region — no
  revision/undo side effects. Arrow nudges use the same mechanism.
- **Approximations (documented)**: z-order — the preview always paints
  on top during the gesture (exact again at release); region effects
  (Blur/eraser-blur) show a plain accent frame as preview — the real
  pixels return on release/cancel.
- **Region growth**: `expand_for_effects` grows the patch region
  iteratively until it covers every overlapping effect's full
  footprint; >50 % of the image → full-render fallback.
- **Measurements (release, 5K×16 annos)** — before → after:
  per-frame 330 ms → ~5 ms (preview patch, 0 ms when stationary via
  `preview_dirty`); drag-start 330 ms → ~3–17 ms; release 330 ms →
  ~6–22 ms; Esc 330 ms → ~3–17 ms; full `render_to_image` 310 ms →
  ~66 ms.
  **Bézier `Line` optimization**: `draw_line_obj` uses 1-Lipschitz 8×8
  tile SDF culling (`tile_reach`) to skip empty tiles in one check and
  filter active tiles down to their 1–3 local curve segments —
  bit-identical at tolerance 0, cutting per-line rasterization from
  ~70 ms to ~3–6 ms.
- **Next unit**: per-annotation cached layers (painted patch + bounds,
  keyed by annotation revision) — region and full renders then become
  bounded composites of cached layers; also fixes commit-time
  re-renders and undo/redo. Not implemented here.

## 2026-10-29 — Crosshair follows the cursor across monitors, locks to the screen where the zone starts

- **Context**: with several monitors, the capture overlay always opened on the
  PRIMARY monitor: `begin_capture` and the overlay freeze both called
  `screen::primary_monitor()`. Worse, because a single overlay viewport only
  receives pointer events inside its own bounds, moving the mouse onto the
  other display froze the crosshair on the first screen and never showed it on
  the second — and starting from a bar on the secondary screen showed nothing
  until the cursor wandered back onto the primary.
- **Decision**: `warm_freeze` photographs every connected monitor up front
  (`screen::capture_all_monitors`). While no selection has started (`can_roam`
  — no active drag and no placed selection), the overlay polls the OS global
  cursor position (`platform::cursor_position`: `CGEvent::location` on macOS,
  `GetCursorPos` on Windows) each frame and hops the overlay viewport + texture
  to whichever monitor currently holds the cursor. As soon as the user clicks
  to start dragging a region (or places a fixed-region frame), roaming stops
  and the selection stays confined to that monitor — no cross-monitor spanning
  (user decision). Fixed-region and pin prefill memories also carry the
  `monitor_id` so a remembered rect on monitor A does not lock the overlay onto
  monitor B.
- **Consequences**: regardless of which screen holds the floating bar (or even
  when launched from the menu bar / status item), the crosshair and magnifier
  follow the cursor freely from one monitor to another until the user starts
  drawing, then stay clamped to the monitor where the drag began. Coordinate
  spaces between egui viewports and `MonitorInfo::rect` are bridged by
  `platform::ui_to_native` / `native_to_ui`. Opening the floating bar
  (`Command::ShowBar`, "+" button or status-item menu) always places it
  top-centred on the monitor where the cursor sits, and confirming a capture
  opens the viewer on the captured monitor (keeping its existing position when
  already on that monitor, or centring it there).

## 2026-10-29 — Pin capture behaves exactly like a region capture

- **Context**: the Pin capture pre-filled its selection from the last confirmed
  capture rect (`last_capture_rect`), so a frame appeared immediately on the
  remembered monitor and the crosshair never followed the cursor across
  displays. The user wants Pin to start exactly like a rectangle capture and
  only differ at confirmation: double-click creates the autonomous floating
  pin window instead of a viewer tab.
- **Decision**: Pin no longer pre-fills any selection — `restore_sel` is only
  fed by the fixed-region memory, and the `last_capture_rect` memory was
  removed. Pin now roams across monitors like Region, draws a free rectangle,
  and double-click confirms into a pin window at the drawn spot.
- **Consequences**: pin captures always reflect where the user actually
  clicked/drew, on any monitor; one less piece of cross-session state.

## 2026-10-29 — Capture delay shows a countdown card + tick sound

- **Context**: a non-zero `capture_delay_secs` used to sleep a worker thread
  then fire `Command::CaptureNow` — the user got no feedback during the wait.
  The delay menu also asked for a quick "5 s" pick above the 1–9 s range, and
  its popup turned out to be clipped: egui constrains popup `Area`s to the
  viewport rect, and the bar window only hugs the bar (~2 menu rows fit).
- **Decision**: the delayed wait is a UI-owned `Countdown` (deferred viewport,
  outside the state machine like the pins): a floating card pinned
  bottom-right of the monitor under the cursor — it follows the mouse across
  displays — showing the remaining seconds in a large monospace digit while
  `platform::tick()` sounds the OS alert each second (macOS `NSBeep`, Windows
  `MessageBeep`, plain FFI — no audio dependency). At zero it fires
  `Command::CaptureNow`; Esc cancels. The delay thread is gone. For the popup
  clipping, the bar viewport reserves extra transparent height below the bar
  while one of its menus is open (`MENU_RESERVE`) instead of reimplementing
  menu open/close semantics.
- **Consequences**: a delayed capture is visible and audible, cancellable with
  Esc, and works from every state (bar, viewer, idle); the countdown card can
  hop monitors with the cursor. The delay menu now shows "No delay", the
  quick 5 s pick, a separator, the full 1–9 s range and the auto-reset toggle
  without clipping (options menu benefits too).

## 2026-10-29 — Delay persists for the session; countdown anchors under the bar

- **Context**: the first countdown version parked the card at the monitor's
  bottom-right corner — unreadable under the macOS Dock (the Dock owns a
  full-screen window at layer 20, so no slab can be extracted by window
  enumeration; `NSScreen.visibleFrame` would have meant a new dependency).
  Separately, `delay_auto_reset` (default `true`) wiped the chosen delay after
  every capture.
- **Decision** (user): the countdown card is anchored just below the floating
  capture bar, centred on it, and follows the bar when it is dragged (the Bar
  viewport reports its outer rect every frame into `Shared::bar_rect`; the
  card repositions each repaint). With no visible bar (capture launched from
  a menu while the bar is closed) it falls back to the centre of the monitor
  under the cursor — no dock geometry is computed at all. `delay_auto_reset`
  is removed (the delay menu is exactly "No delay", a quick 5 s pick, a
  separator, then 1–9 s); `capture_delay_secs` is `#[serde(skip)]` — session
  state, reset at launch. The bar's clock button shows the accent-soft-fill+
  border "selected" style while a delay is armed.
- **Consequences**: the countdown digit is always readable next to the window
  that armed it, on whichever monitor the bar sits, and moves with the bar;
  the chosen delay stays armed for every capture until changed or the app
  quits, with a blue-framed clock icon as the tell. No dependency change.

## 2026-10-29 — Include-mouse-pointer stamps the real OS cursor with zero new dependencies

- **Context**: `Settings::include_pointer` was exposed in the floating bar's
  gear menu, but nothing in the capture pipeline read it and `xcap` never
  renders the mouse cursor into captured frames.
- **Decision**: when `include_pointer` is `true`, `platform::cursor_snapshot`
  fetches the actual OS cursor image, its global position and its hotspot at
  capture time — with zero new crate dependencies:
  - macOS queries `NSCursor.currentSystemCursor` (falling back to
    `NSCursor.currentCursor`), `hotSpot`, `image.size` and
    `CGImageForProposedRect:context:hints:` directly via the Objective-C C
    runtime (`objc_getClass`, `sel_registerName`, `objc_msgSend` from
    `libobjc`) and rasterizes the `CGImageRef` at the target monitor's
    `scale_factor` through `core-graphics` (`CGContext`), which is already a
    direct dependency.
  - Windows queries `GetCursorInfo` + `GetIconInfo` and renders the cursor via
    `DrawIconEx` into a 32-bit DIB section (`user32` / `gdi32` FFI).
  - If the OS cursor is hidden or cannot be retrieved, `cursor_snapshot`
    returns `None` rather than drawing a synthetic fallback arrow.
  `screen::capture_all_monitors(include_pointer)` stamps the snapshot once
  onto the monitor containing the cursor before any cropping (so Region,
  FixedRegion, FullScreen and Pin all inherit it), and
  `window::capture_window(id, include_pointer)` stamps it onto standalone
  window captures when the cursor lies inside the window bounds.
- **Consequences**: `include_pointer = false` keeps the exact previous
  behaviour; `include_pointer = true` composites the real system cursor
  (arrow, I-beam, hand, resize, etc.) at its exact hotspot and Retina scale
  with no added crate.

## 2026-10-30 — Original PixMe icon set wired to every OS surface

- **Context**: all OS-facing icons were drawn from the Lucide "Scan" glyph
  (macOS status item, Windows tray) or left to the OS default (window
  icons, Dock icon, executable icon). The user supplied an original PixMe
  icon set (`assets/app/`, see its README — no third-party assets).
- **Decision**:
  - macOS status item → `menubar-template@2x.png` (44 px, black on
    transparent, loaded templated so the menu bar recolours it).
  - macOS Dock → `macos::apply_native_options` attaches `dock_icon_data()`
    (`pixme-1024.png`, which carries the Apple icon-grid margins and
    baked shadow) to `eframe::NativeOptions`, and `eframe`'s
    `AppTitleIconSetter` installs it via `NSBitmapImageRep` +
    `setApplicationIconImage:`. `PixMe.icns` is kept in `assets/app/` for
    a future bundle.
  - Windows tray → `pixme-window-256.png` RGBA (full colour — the Windows
    tray has no template mode).
  - Window icons → `ViewportBuilder::with_icon` on every window builder
    with the same PNG decoded once (`egui::IconData`); Windows shows it
    in the taskbar/title bar, macOS ignores it.
  - Windows executable → `winresource` (target-gated build-dependency,
    `#[cfg(windows)]` in `build.rs`) embeds `PixMe.ico` into the PE.
  - The now-unused `icons::rgba` / `rgba_with` helpers were removed.
- **Consequences**: one new build-only dependency (`winresource`, no
  runtime code); the `.icns` has no consumer until an `.app` packaging
  step exists; menubar-template.png (1x) is kept for reference only (the
  status item always loads the 44 px 2x artwork, which the tray
  implementation clamps to 22 pt).

## 2026-10-30 — Mosaic tile OS drag-out via `drag = "=2.1.1"` (`src/platform/drag_out.rs`)

- **Context**: each mosaic tile exposed a top-left `GripVertical` badge with the
  tooltip "Drag to copy this capture into another app", but the widget was
  wired with `Sense::click()` and had no drag handler. egui/eframe has no
  built-in outbound OS drag-and-drop API.
- **Dependency audit (`drag = "=2.1.1"`, `crabnebula-dev/drag-rs`, audited
  2026-10-30 — verdict: GO WITH CAVEATS)**:
  - **Supply chain & licenses**: MIT OR Apache-2.0, maintained by CrabNebula /
    Tauri team (`lucasfernog-crabnebula`), released 2026-09-24 (~102k recent
    downloads), `build = false` (no build script), no network/process/filesystem
    writes in crate source. `cargo audit` reports zero advisories on `drag` or
    any of the 17 newly locked crates (the 7 warnings in the tree are all
    pre-existing transitive dependencies).
  - **Platform hygiene & caveats**:
    - `raw-window-handle` `^0.6` matches our `eframe 0.36` / `winit 0.30`
      (`0.6.2` shared, single copy).
    - macOS: uses `objc2 0.6.5` / `objc2-app-kit 0.3.2` / `objc2-foundation
      0.3.2` already in our lockfile via `muda` (no legacy `cocoa`/`objc` 0.2).
      Adds `core-graphics 0.24.0` + `core-foundation 0.10.1` alongside our
      direct `0.23.2` / `0.9.4`.
    - Windows: pulls `windows 0.52.0` + `windows-core 0.58.0` alongside the
      existing `windows 0.62.2` in the lockfile (compile-time / size caveat on
      Windows builds only).
  - **Fallback plan**: all `drag` crate usage is isolated inside
    `src/platform/drag_out.rs`. Replacing the crate with direct
    `NSDraggingSession` (macOS) + `IDropSource`/`DoDragDrop` (Windows) only
    touches that single module.
- **Decision**:
  - Pin `drag = "=2.1.1"` and `raw-window-handle = "0.6"` in `Cargo.toml`.
  - **Full-resolution payload, small cursor preview**: dragging a mosaic tile's
    grip renders the full capture (`editor::render::render_to_image(&tab.doc)`,
    base + annotations — never the 480 px mosaic thumbnail, and unrestricted on
    password-locked tabs per the protection spec), writes it atomically to
    `std::env::temp_dir()/pixme-drag/<sanitized-title>.png`, and passes that
    absolute file path as `DragItem::Files` (`DragItem::Data` is macOS-only in
    `drag` 2.1.1). The cursor-following drag icon (`Image::Raw`) is downscaled
    to ≤ 160 px on its longest side so 5K captures do not cover the screen
    during the drag (`drag-rs` issue #41).
  - **Temp file lifecycle**: files are never deleted inside the drop callback
    (receiving apps read the dropped path asynchronously after the drag returns);
    `drag_out::cleanup_stale_files()` removes files older than 1 hour at
    application startup.
  - **egui pointer state**: both `beginDraggingSessionWithItems` and
    `DoDragDrop` run their own modal tracking loop and swallow the physical
    mouse-up event, so the viewer injects a synthetic `PointerButton { pressed:
    false }` + `PointerGone` and calls `Context::stop_dragging()` immediately
    after `start_image_drag` returns.
  - **Scope**: only the mosaic tile grip initiates OS drag-out in this unit.
    Tab drag-out and pinned-window drag-out remain deferred to a later unit
    (tab drag will need a gesture rule distinguishing tab reorder from
    drag-out) and will call the same `platform::drag_out::start_image_drag`
    helper.
- **Consequences**: dragging any mosaic tile by its top-left grip drops the
  full-resolution rendered PNG into Finder/Explorer, browsers, Slack, Discord,
  Mail, etc. on macOS and Windows.

## 2026-10-30 — Native OS print dialog replaces `lp` / `mspaint /p`

- **Context**: `platform::print_file` queued a temp PNG straight to the
  default printer (`lp` on macOS, `mspaint /p` on Windows) — no print
  dialog, no printer choice, and Paint flashed open on Windows. The user
  wants the real OS print dialog.
- **Decision**: `platform::print_image(img, job_title)` shows the native
  print UI on both OSes, through plain FFI like the rest of `platform/`
  (zero new dependencies):
  - macOS: `NSPrintOperation` on an `NSImageView` holding the capture —
    `objc_msgSend` FFI. `NSFitPagination` on both axes scales the image to
    a single page; `setJobTitle:` carries the capture title into the
    queue. PNG bytes feed `NSImage` via `NSData` — no temp file.
  - Windows: `PrintDlgW` (printer picker, page range, copies) returns a
    printer DC; `StretchDIBits` writes the RGBA→BGRA-swapped bitmap
    letterboxed inside the physical page (`GetDeviceCaps`
    PHYSICALWIDTH/HEIGHT/OFFSET). `DOCINFOW.lpszDocName` = capture title.
    `CommDlgExtendedError` distinguishes cancel from failure.
  - `export::file::print_image(img, job_title)` is the shared entry point;
    callers pass the tab's title (`PrintActive`, Draw toolbar) with
    `"PixMe"` as fallback. The temp `pixme-print.png` file is gone.
- **Consequences**: the print button opens the real print panel on both
  OSes with the capture's name as the job title; the image prints fit to
  one page. Cancelling the dialog returns `Err` ("cancelled") — logged
  as a warning like before.

## 2026-10-31 — Pin windows: remove corner drag-resize and claim drag hits on chrome controls

- **Context**: clicking the top-left close dot on a pinned capture caused the
  window to jump downward or enter an OS window drag instead of closing on the
  first click. In `egui` 0.36, a `Sense::click()` widget stacked on top of a
  `Sense::drag()` widget (`pin-body` or the 20×20 corner resize grips) only
  claims the click hit — `hit_test_on_close` still returns the underlying
  drag-only widget as the drag hit, which becomes `dragged` on the press frame
   itself. Pressing inside `[7..20]²` latched a corner resize anchored at the
  bottom-right corner (shrinking the window downward on any pointer drift);
  pressing inside `[20..25]²` fired `ViewportCommand::StartDrag` on the body.
- **Decision** (supersedes the corner-resize part of 2026-10-24 "Pin resize,
  white chrome, and a real save-as window"):
  - **No user drag-resize on pins**: `resize_grips`, `GRIP`, `corner_zone`,
    `PinAnchor` and `resize_start` are removed. The zoom preset menu (`50%`,
    `100%`, `200%`) remains the sole way to scale a pin (`apply_zoom` anchored
    at the top-left corner).
  - **Chrome buttons use `Sense::click_and_drag()`**: the close dot, zoom pill,
    copy and save buttons register with `Sense::click_and_drag()` after
    `pin-body` so `egui`'s hit-tester assigns both `click` and `drag` to the
    button and never leaks `StartDrag` to the body behind it. Releasing while
    still over the button also fires when pointer drift exceeded
    `max_click_dist` (`drag_stopped() && contains_pointer()`).
- **Consequences**: clicking the close dot (or any bottom-strip button) closes
  or activates reliably on the first click without moving or resizing the pin,
  even with slight trackpad drift; dragging the image body or the centre grip
  pill still moves the window.

## 2026-10-30 — Lossy stdout writer for `tracing-subscriber` and panic-log-first hook

- **Context**: a macOS crash report (`pixme-2026-10-30-185600.ips`) after the
  resident app had been running all afternoon with an exited parent shell
  (`"parentProc": "Exited process"`) showed `SIGABRT` on the main thread when
  closing/cancelling the print panel (`Command::PrintActive` →
  `tracing::warn!("print failed: {e}")`), with no `panic.log` written.
  Disassembling the exact crash offsets revealed the full chain:
  1. Because the parent shell had exited, `stdout` and `stderr` were broken
     pipes (`EPIPE`).
  2. `tracing_subscriber::fmt` wrote the `warn!` event to its default
     `io::stdout` writer, got `Err(BrokenPipe)`, and executed its internal
     fallback `eprintln!("[tracing-subscriber] Unable to write an event to the
     Writer for this Subscriber! Error: …")`.
  3. Rust's `eprintln!` panics (`failed printing to stderr: Broken pipe`) when
     `stderr` also returns `Err(BrokenPipe)`.
  4. `install_panic_hook` caught that panic and called `tracing::error!("{line}")`
     on its very first line — before writing `panic.log` — which re-entered
     `tracing_subscriber::fmt`, hit the same `eprintln!` panic on the broken
     `stderr` pipe, and triggered a double-panic immediate `abort()`.
- **Decision**:
  - `main.rs` configures `tracing_subscriber::fmt().with_writer(|| LossyStdout)`
    where `LossyStdout` forwards to `std::io::stdout()` and swallows any write
    or flush error (`Ok(buf.len())`), so a closed terminal / broken pipe can
    never make `tracing-subscriber` call `eprintln!`.
  - `install_panic_hook` writes `data_dir()/panic.log` and `stderr` (`let _ =
    write_all`) **before** calling `tracing::error!`.
- **Consequences**: the resident app survives indefinitely after the launching
  terminal/shell closes; any future panic is guaranteed to land in `panic.log`
  first.

## 2026-10-30 — Five recent files under File → Open

- **Context**: the functional reference keeps a recent-files section in
  its File menu; PixMe had none — reopening a just-saved capture meant
  browsing the file picker again.
- **Decision**: `Settings::recent_files` persists up to five paths
  (newest first, dedup on re-push) via `push_recent`/`remove_recent`.
  The list feeds on every file that lands on disk through PixMe —
  single save dialog (viewer and pins), each `Save All` write, and
  every file opened — and on every successful re-open (MRU semantics).
  `Command::OpenPath(path)` reopens a file directly; an unreadable path
  is dropped from the list so dead entries clean themselves up. The
  File menu's Open section and the toolbar's Ouvrir dropdown share the
  same `recent_menu_items` helper (file names, full path on hover,
  hidden when empty), followed by an action zone — `OpenAllRecent`
  (reopens the whole list at once) and `ClearRecent` (empties it).
  `save_all`'s worker cannot touch `Shared`, so its
  job channel now carries `(tabs, saved_paths)` and `pump` pushes the
  recents on the main thread.
- **Consequences**: File → Open and the Ouvrir dropdown show the five
  most recent files; clicking one opens it as a clean tab. The macOS
  native File menu does not list them (egui menus only for now).

## 2026-10-30 — Centralised data paths (`src/paths.rs`) + file logging (`src/logging.rs`)

- **Context**: data locations were scattered — `session.db` under
  `ProjectDirs::data_dir()`, `prefs.json` beside the exe, `panic.log` in
  yet another computed dir, and logging went to stdout only. The spec
  asked for one resolver, a Windows portable layout, daily file logging
  and hard privacy rules.
- **Decisions**:
  - `src/paths.rs` is the ONLY module building app-data paths
    (`data_dir`, `logs_dir`, `settings_file`, `db_path`, `instance_lock`,
    `panic_log`), frozen in a `OnceLock` around pure `pick_dirs`/
    `strip_verbatim`/`probe_writable` helpers; `tests/data_paths.rs`
    fails the build if `ProjectDirs`/`current_exe`/`LOCALAPPDATA`/
    `config_dir`/`data_local_dir`/`home_dir`/`Library/Logs` appears
    outside `src/paths.rs` and `src/platform/`.
  - Windows = portable: `<exe>\data\` and `<exe>\logs\` from
    `current_exe()` (never the cwd — shortcuts change it), `\\?\` and
    `\\?\UNC\` prefixes stripped without canonicalization. A startup
    writability probe (create `data`+`logs`, write/remove a
    `.<pid>.tmp` file) falls back to `%LOCALAPPDATA%\PixMe\{data,logs}`
    when the exe dir is read-only; the user gets ONE deferred
    translated notice once the first window exists, plus a log warning.
  - macOS = system locations: `~/Library/Application Support/pixme`
    (data) and `~/Library/Logs/PixMe` (logs) — a `.app` bundle is never
    written into.
  - No session/log migration: a previous per-user `ProjectDirs` data
    dir is warned about in the log and left intact — captures and logs
    are user data, never moved silently.
  - `prefs.json` IS migrated once: copied (not moved) from the
    exe-adjacent or per-user-config location, only when the new file
    does not exist and the candidate parses as a valid `Settings`.
    User decision: losing settings on update would silently reset the
    app; settings carry no sensitive data, unlike captures.
  - Single instance is per-installation on Windows: `instance.lock`
    lives in `data\`, so two portable installs may run side by side
    (each owns its session + logs); a second launch from the SAME
    folder shows the existing translated `app.already_running` dialog.
    Bringing the first instance's window forward was skipped — no
    cross-process window handle exists without fragile Win32/AppleScript
    code. Global-hotkey conflict handling is moot (feature not
    implemented yet); when it lands, registration failure must run
    hotkey-less with one translated notice, not crash.
  - Logging: `tracing-appender` daily files `pixme.<UTC date>.log`
    (rotation is UTC — the crate's clock is `OffsetDateTime::now_utc`
    with no local-time option), `max_log_files(5)` plus a startup purge
    of dated files older than 5 days and an mtime-aged `pixme.panic.log`
    (foreign files never touched). Non-blocking lossy writer (8192-line
    queue) — under saturation lines are dropped, counted via
    `error_counter()` and reported once; chosen over backpressure
    because a resident UI app must never stall on logging. Size caps on
    the single daily file: >64 MiB keeps `warn`/`error` only (soft cap,
    announced once), >256 MiB stops all file output (hard cap); the
    panic hook writes `pixme.panic.log` synchronously and is NEVER
    capped. The `WorkerGuard` lives in `main` and is released in
    `logging::shutdown()` on every exit path (eframe return covers
    `quit_now`/menu quit; `process::exit` calls are pre-shutdown).
  - Timestamps: local time with explicit offset, the zone captured on
    the FIRST line of `main` before any thread exists (late reads of
    the local zone can fail), UTC on failure.
  - Privacy: `Sensitive<T>` redacts `Debug`/`Display`; paths are
    reduced (`file_name`/`tail_path`) at `info`, full user paths only
    in the one-time banner and the deferred notice; window titles stay
    `debug`-only; the diagnostics copy re-redacts the home dir (`~`).
  - Diagnostic mode is a session-only `debug` switch (Settings →
    General) implemented as a `reload` handle on the `EnvFilter`;
    `RUST_LOG` keeps precedence at startup. Stdout echo exists only in
    debug/dev-tools builds — release writes the file and stays silent
    (a dead console pipe must never kill a resident app).
- **Consequences**: replacing `pixme.exe` in `C:\portable\pixme\` never
  touches `data\`/`logs\`; an update of the portable folder's exe is a
  pure file swap. Dev builds writing beside `target\release\pixme`
  create `target/release/{data,logs}` — covered by the `target/`
  gitignore entry and by `cargo clean`; real sessions never live in
  `target/` because release-run data follows the exe inside `target/`
  only when the exe itself is there (dev runs), and `PIXME_DATA_DIR`
  overrides are dev-tools-only.

## 2026-10-30 — Centralised data paths (`src/paths.rs`) + file logging (`src/logging.rs`)

- **Context**: data locations were scattered — `session.db` under
  `ProjectDirs::data_dir()`, `prefs.json` beside the exe, `panic.log` in
  yet another computed dir, and logging went to stdout only. The spec
  asked for one resolver, a Windows portable layout, daily file logging
  and hard privacy rules.
- **Decisions**:
  - `src/paths.rs` is the ONLY module building app-data paths
    (`data_dir`, `logs_dir`, `settings_file`, `db_path`, `instance_lock`,
    `panic_log`), frozen in a `OnceLock` around pure `pick_dirs`/
    `strip_verbatim`/`probe_writable` helpers; `tests/data_paths.rs`
    fails the build if `ProjectDirs`/`current_exe`/`LOCALAPPDATA`/
    `config_dir`/`data_local_dir`/`home_dir`/`Library/Logs` appears
    outside `src/paths.rs` and `src/platform/`.
  - Windows = portable: `<exe>\data\` and `<exe>\logs\` from
    `current_exe()` (never the cwd — shortcuts change it), `\\?\` and
    `\\?\UNC\` prefixes stripped without canonicalization. A startup
    writability probe (create `data`+`logs`, write/remove a
    `.<pid>.tmp` file) falls back to `%LOCALAPPDATA%\PixMe\{data,logs}`
    when the exe dir is read-only; the user gets ONE deferred
    translated notice once the first window exists, plus a log warning.
  - macOS = system locations: `~/Library/Application Support/pixme`
    (data) and `~/Library/Logs/PixMe` (logs) — a `.app` bundle is never
    written into.
  - No session/log migration: a previous per-user `ProjectDirs` data
    dir is warned about in the log and left intact — captures and logs
    are user data, never moved silently.
  - `prefs.json` IS migrated once: copied (not moved) from the
    exe-adjacent or per-user-config location, only when the new file
    does not exist and the candidate parses as a valid `Settings`.
    User decision: losing settings on update would silently reset the
    app; settings carry no sensitive data, unlike captures.
  - Single instance is per-installation on Windows: `instance.lock`
    lives in `data\`, so two portable installs may run side by side
    (each owns its session + logs); a second launch from the SAME
    folder shows the existing translated `app.already_running` dialog.
    Bringing the first instance's window forward was skipped — no
    cross-process window handle exists without fragile Win32/AppleScript
    code. Global-hotkey conflict handling is moot (feature not
    implemented yet); when it lands, registration failure must run
    hotkey-less with one translated notice, not crash.
  - Logging: `tracing-appender` daily files `pixme.<UTC date>.log`
    (rotation is UTC — the crate's clock is `OffsetDateTime::now_utc`
    with no local-time option), `max_log_files(5)` plus a startup purge
    of dated files older than 5 days and an mtime-aged `pixme.panic.log`
    (foreign files never touched). Non-blocking lossy writer (8192-line
    queue) — under saturation lines are dropped, counted via
    `error_counter()` and reported once; chosen over backpressure
    because a resident UI app must never stall on logging. Size caps on
    the single daily file: >64 MiB keeps `warn`/`error` only (soft cap,
    announced once), >256 MiB stops all file output (hard cap); the
    panic hook writes `pixme.panic.log` synchronously and is NEVER
    capped. The `WorkerGuard` lives in `main` and is released in
    `logging::shutdown()` on every exit path (eframe return covers
    `quit_now`/menu quit; `process::exit` calls are pre-shutdown).
  - Timestamps: local time with explicit offset, the zone captured on
    the FIRST line of `main` before any thread exists (late reads of
    the local zone can fail), UTC on failure.
  - Privacy: `Sensitive<T>` redacts `Debug`/`Display`; paths are
    reduced (`file_name`/`tail_path`) at `info`, full user paths only
    in the one-time banner and the deferred notice; window titles stay
    `debug`-only; the diagnostics copy re-redacts the home dir (`~`).
  - Diagnostic mode is a session-only `debug` switch (Settings →
    General) implemented as a `reload` handle on the `EnvFilter`;
    `RUST_LOG` keeps precedence at startup. Stdout echo exists only in
    debug/dev-tools builds — release writes the file and stays silent
    (a dead console pipe must never kill a resident app).
- **Consequences**: replacing `pixme.exe` in `C:\portable\pixme\` never
  touches `data\`/`logs\`; an update of the portable folder's exe is a
  pure file swap. Dev builds writing beside `target\release\pixme`
  create `target/release/{data,logs}` — covered by the `target/`
  gitignore entry and by `cargo clean`; real sessions never live in
  `target/` because release-run data follows the exe inside `target/`
  only when the exe itself is there (dev runs), and `PIXME_DATA_DIR`
  overrides are dev-tools-only.

## 2026-10-30 — macOS app menus: the Capture submenu is removed

- **Context**: the native menu bar carried PixMe / Capture / File / Edit /
  Window. The Capture submenu duplicated the floating bar's modes and was a
  live path to launching a second capture while the overlay owned the
  screen — the user judged it added little.
- **Decision**: the Capture submenu (and its `menu.capture` locale key plus
  the `capture.*` id → `Command::Capture` mappings) is removed; the
  remaining menus are PixMe / File / Edit / Window, and captures start from
  the floating bar or the status item only.
- **Consequences**: no native-menu path can start a capture mid-capture;
  other commands (status item, File/Edit items) remain gated only by
  `ModalState`, which has no capture-in-progress variant — greying during
  an active capture is still unimplemented if wanted later.

## 2026-10-31 — Windows undecorated floating windows: `WS_CAPTION` stripping and native window regions (`SetWindowRgn`)

- **Context**:
  1. On Windows with Intel drivers or OpenGL (`glow`), `winit` 0.30 forces
     `WS_CAPTION | WS_BORDER` styles on all windows even when
     `decorations: false` is configured, relying on `WM_NCCALCSIZE` to hide
     them. The desktop compositor still reserves space for the caption,
     shifting content ~31 px down and clipping it at the window bottom
     (`emilk/egui#2770`, `libsdl/SDL#12105`, and PhotoCraft issue `#2246`).
  2. On Windows 11, DWM draws an accent line at y=0 on borderless windows
     (documented in `winit::platform::windows::WindowExtWindows::set_undecorated_shadow`:
     *"Enabling the shadow causes a thin 1px line to appear on the top of the window"*).
- **Decision**:
  - Adopt PhotoCraft's exact `#2246` fix in `src/platform/windows.rs`: strip
    `WS_CHROME` (`WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX`)
    on undecorated topmost floating windows via `SetWindowLongPtrW` +
    `SWP_FRAMECHANGED | SWP_NOCOPYBITS`.
  - Assign distinct internal titles to floating builders (`"PixMe Bar"`,
    `"PixMe Pin"`, `"PixMe Countdown"`). Because all three are undecorated and
    omitted from the taskbar, these titles are never visible to the user on
    either OS.
  - Apply native Win32 GDI window regions (`SetWindowRgn`) to tell DWM the true
    geometry of each window:
    - For `"PixMe Bar"` and `"PixMe Countdown"`: both carry a 14 pt transparent
      `SHADOW_MARGIN`, so clipping y=0..2 (`CreateRectRgn(0, 2, w + 1, h + 1)`)
      eliminates the DWM top accent line without touching the card or its soft shadow.
    - For `"PixMe Pin"`: apply a rounded-rectangle region (`CreateRoundRectRgn(0, 0, w + 1, h + 1, 16, 16)`)
      matching the card's 8 pt rounded corners (16 px diameter), cleanly
      eliminating the straight DWM line while preserving the 1 px border stroke
      on all four sides.
  - Reapply `SetWindowRgn` dynamically whenever styles are stripped or window
    dimensions change (winit triggers `SWP_FRAMECHANGED` on focus / child window
    transitions, which resets the non-client frame).
- **Consequences**:
  - Floating bar, countdown and pinned windows render crisply and completely on
    Windows with zero visual artifacts.
  - macOS version is 100% unaffected (`strip_undecorated_frame` is an inlined
    no-op on macOS in `src/platform/mod.rs`).

## 2026-11-01 — Session store: eager write-through on a dedicated writer thread

- **Context**: the session was persisted only at quit (one `save_session`
  transaction) plus per-tab deletes — a crash, a kill or a system blue
  screen between writes lost the whole session's changes. The user asked
  for captures to reach disk "as fast as possible" (the Draw editor
  excepted: its document is written when it exits).
- **Decision**:
  - A dedicated `pixme-store` thread owns the `Store` for the whole run
    (`src/store/writer.rs`) and drains a strictly-ordered job queue:
    `DocWrite` (full row, `base_png` re-encoded only when `base_dirty`),
    `Meta` (bulk meta-columns refresh — reorders, renames, save flips),
    `Session` (active tab + title counter), `Delete`, `Clear`,
    `Retention`, `Load`, `Finalize` (quit barrier: meta refresh +
    stale-row prune), `Flush` (reply barrier). No PNG encoding or SQLite
    I/O ever runs on the UI thread.
  - Change detection is a single drain (`Shared::sync_store`, tail of
    every pump): each tab carries `stored_fp` (a hash of everything the
    store persists); a drift re-queues that document. No per-mutation
    call sites — every current and future change is caught. "Handed to
    the writer" marks (`stored_fp`, `persisted_rev`, `base_dirty`) move
    at enqueue, not at commit: failed jobs retry from their snapshot.
  - Quit (and every clean exit) enqueues `Finalize` + `Flush` and waits
    on the barrier behind the saving veil — bounded by
    `QUIT_FLUSH_TIMEOUT` (10 s) after which a `warn` is logged and the
    app exits anyway. `Writer::drop` sends `Shutdown` and joins, so the
    `Store` Drop (VACUUM) still runs on forgotten queues.
  - Failures: a job is retried once in place, then parked and retried
    again at flush; the document reports `DocFailed` — its tab and
    mosaic tile show a `TriangleAlert` glyph (new Lucide icon) until a
    write succeeds, and a single translated notice (`app.store_write_failed`,
    EN/FR/ES) explains it once per run. No write failure ever blocks or
    crashes the app.
  - Single-file "Enregistrer" (save-as dialog) now encodes on a worker
    too (`SaveOneJob`) — a 5K PNG write no longer stalls frames; the
    dialog shows a spinner and greys its buttons.
- **Consequences**:
  - The loss window shrinks from "everything since launch" to
    enqueue→commit time (measured in the hundreds of ms on 5K frames —
    see `--bench-store` output and the `queued_ms` log field).
  - **Privacy trade-off (accepted, user-requested)**: captures land in
    `session.db` at creation even when never explicitly saved.
    Mitigations kept: `PRAGMA secure_delete` zeroes deleted payloads,
    tab close deletes the row, Discard empties the store, `.bak` files
    age out after 7 days, and no temp copy is written anywhere else.
  - `dev-tools` gains `pixme --bench-store [N]`: PNG compression
    trade-off, per-doc enqueue cost, `queued_ms`/`work_ms` stats and
    restore timing on N synthetic 5K captures.

## 2026-11-02 — Windows chrome-less viewports: hidden creation + `ShowWindow` bypassing winit

- **Context**: a Windows 0.30 winit quirk — `to_window_styles`
  (`window_state.rs`) re-applies `WS_CAPTION | WS_BORDER | WS_SYSMENU`
  **on every flag change, including `VISIBLE`** — means neither
  visible-at-creation NOR egui's `ViewportCommand::Visible(true)` (→
  `set_visible` → `apply_diff` → `SetWindowLongW`) can show a
  chrome-less window without a one-frame title bar. The caption reads
  *empty* because the strip also clears the title text.
- **Decision**: chrome-less viewports (bar, pins, countdown) are
  created `with_visible(false)` (Windows-only `#[cfg]`) and
  `strip_undecorated_frame`'s `visit` shows them itself —
  `ShowWindow(hwnd, SW_SHOW)` directly on the already-stripped hwnd
  (`SW_SHOWNOACTIVATE` for the countdown, mirroring `with_active`).
  winit never sees a `VISIBLE` flag diff, so its style re-application
  never runs. The overlay keeps its egui `Visible(true)` reveal —
  its `SetMonitor` sets `MARKER_BORDERLESS_FULLSCREEN` first, which
  makes `to_window_styles` drop `WS_OVERLAPPEDWINDOW`, so that path
  is already caption-free. `Role` split `CardWithMargin` into
  `Bar`/`Countdown` for the per-role show flag. Decorated windows,
  the root anchor and macOS are untouched.
- **Consequences**: no caption can ever be painted on
  bar/pin/countdown. `chrome_stripped_on_visible_window` (debug
  level) alarms if a window is ever caught visible-with-chrome —
  the flash signature. Residual: winit may re-add chrome mid-life on
  frame changes (re-stripped next pump). Guarded by the
  `chromeless_viewports_start_hidden` test.

## 2026-11-02 — Capture kiosk: real menu bar + Dock hidden during selection; overlay covers them

- **Context**: rectangular and window-pick captures could not reach the
  macOS menu bar or Dock (and the Windows taskbar): macOS clamps
  `setFrame` to the screen's `visibleFrame` (menu bar / Dock excluded),
  so the overlay never covered those strips even at
  `kCGScreenSaverWindowLevel`, and the Windows taskbar is itself
  TOPMOST. iScreenShot-style tools get past this by disabling the real
  chrome outright during capture — the user's explicit reference.
- **Decision**: while a non-instant capture is live (the frozen frames
  are already safe), enter a capture kiosk —
  `NSApplication.presentationOptions = HideDock | HideMenuBar |
  DisableProcessSwitching | DisableHideApplication`. The REAL chrome
  disappears (menu bar cannot be clicked or revealed, Cmd+Tab is dead);
  its pixels live on in the frozen frame painted by the overlay, so
  every screen strip stays selectable but unclickable. Because the
  visibleFrame clamp only releases as the hide animates (~0.3 s), the
  overlay re-sends `OuterPosition`/`InnerSize` every frame until the
  window covers the monitor's true frame, capped at 150 tries — the
  existing `screen_offset` mapping keeps image↔screen exact throughout.
  `enter()` calls `capture_chrome_exit()` on every transition out of
  `Capturing` (process exit also restores the options automatically).
  Windows needs no hiding: the overlay is topmost + borderless
  fullscreen over the taskbar — `strip_undecorated_frame` additionally
  re-asserts `HWND_TOPMOST` on the overlay at creation so the taskbar's
  own topmost band can never win.
- **Consequences**: region/fixed/pin captures can select over the menu
  bar, Dock and taskbar (their clock included — the user's stated goal);
  menus are truly inert during selection, matching iScreenShoter.
  Instant fullscreen captures never enter the kiosk (no flicker). If
  the kiosk hide ever fails, the overlay simply stays clamped below the
  menu bar as before — `screen_offset` keeps the capture correct.

## 2026-11-02 — Anchor gets an empty window region; bar geometry ledger resets on re-entry

- **Context**: two Windows-only artifacts reported together. (1) A
  small persistent ring floating on the desktop, sitting where the old
  console window's icon used to be — the invisible 1×1 root anchor is a
  real `WS_EX_TOPMOST` window with winit-forced `WS_CAPTION`, so
  Windows 11's DWM rounded border + top accent line draw around its
  degenerate rect as a small ring. `strip_undecorated_frame` clipped
  every floating window except the anchor (its "PixMe" title had no
  role). (2) The floating bar painted correctly on first show but was
  clipped on every re-show: eframe destroys a deferred viewport's HWND
  when it is not emitted (`remove_viewports_not_in` drops the `Viewport`
  → `Window::drop` → `destroy_window`) and rebuilds it at the builder's
  default `340×48` on re-entry — while `Toolbar.sync` (the
  `ViewportSync` ledger) survives in `Shared`, deduping the
  `InnerSize`/`OuterPosition` resend forever.
- **Decision**: (1) give the anchor its own `Role::Anchor` — title
  match "PixMe" + `WS_EX_TOPMOST` (the viewer is not topmost, so no
  other "PixMe" window can match), its title cleared, `WS_CHROME`
  stripped like the other floating windows, and an **empty**
  `SetWindowRgn` clipped so nothing can draw (a `w<=8 && h<=8` guard
  keeps a recycled hwnd from ever being blanked). (2)
  `Toolbar::reset_sync()` clears the ledger on every `enter(Bar)` — a
  recreated window gets its geometry commands resent, exactly like the
  first show.
- **Consequences**: the anchor can no longer draw anything (it paints
  nothing by design). Bar re-shows send size+position once, then the
  ledger dedupes normally — no resize feedback loops. If a small ring
  persists after this, it is an explorer/touch artifact (cf. the
  `EdgeUiInputTopWndClass` entry) outside PixMe's control, not our
  window.

## 2026-11-02 — The "empty title bar at the top of the screen" is explorer's edge-UI strip, not a PixMe window

- **Context**: a full-width empty strip at the top of the screen
  flashed whenever the capture overlay appeared or disappeared.
  A `SetWinEventHook` probe (used during diagnosis, since removed)
  showed the only window toggling at those exact moments is
  **`EdgeUiInputTopWndClass`, a 1585×4 explorer.exe window at the top
  of the screen** — Windows' touch edge-input strip, hidden by the
  shell while any screen-covering window exists and re-shown when it
  goes away.
- **Decision**: no code change — the strip's hide/show is explorer's
  own generic fullscreen detection, not a reaction to
  `MarkFullscreenWindow` (verified: it toggles identically with the
  manual `OuterPosition`/`InnerSize` placement, so the SetMonitor
  path was kept). Suppressing it would mean hiding another process's
  window — rejected as out-of-process tampering. It renders
  invisibly on healthy systems; a visible white strip suggests an
  explorer glitch (explorer restart can reset it).
- **Consequences**: the flash is a Windows shell artifact outside
  PixMe's control. Any screenshot/capture tool showing a
  screen-covering overlay triggers the same toggle.

## 2026-11-03 — Viewer-centered UI: the floating capture bar is replaced by the viewer's vbar

- **Context**: the app had two mutually-exclusive windows — a floating
  capture bar (`AppState::Bar`) and the tabbed viewer — each with its own
  viewport, geometry plumbing and OS integration. The `docs/audit/ui-overhaul.md`
  plan makes the viewer the single home of the app.
- **Decision**: `Bar` removed from the lifecycle (`Idle | Capturing |
  Viewer`). Capture controls now live in a fixed 36 pt `egui::Panel::right`
  inside the viewer ("vbar"): the five capture modes plus delay and
  options popups, always visible in Tabs and Mosaic modes, gated by the
  shared `is_command_enabled` table. Captures dispatch through
  `Shared::push_capture`; `global-hotkey` (declared, never used) and the
  dead capture/export stubs went out with the bar.
- **Consequences**: one fewer window, no bar geometry/focus/state code;
  all user capture entries funnel through one place. Manual drag of the
  viewer was removed with the move lock (next entries) so the vbar is
  always one strip away.

## 2026-11-03 — Monitor inventory: OS-stable ids, per-monitor prefs, status-strip row

- **Context**: the viewer needed per-monitor names and geometry; xcap
  monitor ids are unstable (Windows ids are pointer handles, macOS ids
  are per-session).
- **Decision**: `platform::MonitorIdentity { stable_id, info }` —
  `CGDisplayCreateUUIDFromDisplayID` on macOS, `EnumDisplayDevicesW` +
  `EDD_GET_DEVICE_INTERFACE_NAME` (PNP DeviceID) on Windows, a
  `rect:{x},{y},{w}x{h}` key as fallback. `Settings::monitors` persists
  `{ name?, rect_native? }` per stable id in `prefs.json`; entries are
  never dropped on disconnect — disconnected monitors stay listed greyed
  until clicked once (visual dismissal only). The inventory is a
  snapshot refreshed at startup and by the status strip's ↻ button —
  never re-enumerated per frame.
- **Consequences**: monitor state survives reconnects/reboots; the
  DeviceID is port-dependent on Windows (documented limitation — the
  rect key covers it poorly). `monitor_rows` is a pure function, unit
  tested.

## 2026-11-03 — Viewer move lock + per-monitor geometry restore

- **Context**: once monitor buttons become the relocation mechanism,
  the viewer itself must not be user-draggable (otherwise both paths
  fight and persisted rects go stale).
- **Decision**: `platform::viewer_move_lock` — macOS applies
  `setMovable(false)` on the "PixMe"-titled normal-level NSWindow;
  Windows subclasses the viewer HWND (`GWLP_WNDPROC`) swallowing
  `WM_NCLBUTTONDOWN`/`HTCAPTION` and `WM_SYSCOMMAND`/`SC_MOVE`. Borders
  stay resizable; programmatic moves are unaffected. Re-applied on every
  `enter(Viewer)` since the deferred viewport is destroyed/recreated.
  `viewer_target` carries `{ monitor, native_rect }` — `SelectMonitor`
  and the post-capture landing both go through `viewer_target_for` and a
  dedicated `ViewportSync` ledger (dedup on requested, never reported,
  geometry). `viewer_geo_settle` skips persisting the reported rect for
  2 frames after a move; `rect_native` writes are debounced to one
  `prefs.json` flush per half-second (flushed in `quit_now` too).
- **Consequences**: parity semantics on both OSes; the saved rect is
  OUTER so `InnerSize` subtracts the measured decoration delta. The
  Windows subclass is written but not compiled on macOS — needs a
  Windows build check.

## 2026-11-03 — Capture starts on the session monitor; Windows re-focus during Capturing

- **Context**: the overlay opened under the cursor; with a monitor
  "selected" in the viewer the user's intent is the viewer's monitor.
- **Decision**: `push_capture` records `viewer_monitor` directly;
  `begin_capture` and `warm_freeze` rank session monitor → remembered
  fixed-region rect → cursor → primary (tried in a loop so a stale id
  can't strand the overlay). While `Capturing`, `pump` re-issues
  `ViewportCommand::Focus` on the overlay every frame on Windows
  (`platform::refocus_capture_overlay`, best-effort vs Alt+Tab); the
  macOS kiosk needs nothing (no-op).
- **Consequences**: cursor roaming unchanged (the overlay still follows
  the cursor across monitors until a selection starts). Re-focus is
  best-effort: `SetForegroundWindow` can be refused — Esc then hits
  another app until a click; Win key / Ctrl+Alt+Del are never
  suppressible (documented limit, no `WH_KEYBOARD_LL` by choice).

## 2026-11-03 — Resident icon: ToggleViewer on left click, open/settings/quit menu, dock reopen

- **Context**: the tray/status item used to only open the viewer;
  the audit makes it a pure open/settings/quit surface.
- **Decision**: `Command::ToggleViewer` — `OpenViewer` from `Idle`,
  `ViewerClosed` when open — fired on `TrayIconEvent::Click` (left-down)
  with `with_menu_on_left_click(false)` (menu stays on right click).
  Both OSes get the same menu: Open PixMe / Settings… / Check for
  updates (disabled placeholder, no updater) / Close PixMe. macOS Dock
  clicks queue `OpenViewer` via `applicationShouldHandleReopen:
  hasVisibleWindows:` — `class_addMethod` when nothing answers it, else
  `method_setImplementation` wrapped onto winit's impl (original runs
  first). Windows taskbar unchanged: viewer keeps its native button,
  the 1×1 anchor keeps `taskbar(false)`.
- **Consequences**: captures can never start behind the user's back
  from the resident icon. The Dock hook swizzles the winit delegate —
  re-check after a winit/eframe bump.

## 2026-11-03 — Windows anchor white ring: hidden creation + unconditional empty region

- **Context**: Windows 11 DWM drew a rounded border + accent line on the
  1×1 anchor (§2.7): the window was created visible before the strip
  guard could clip it (startup race), and `SWP_FRAMECHANGED` reset the
  region without changing `(w,h)` so the ring reappeared.
- **Decision**: root viewport created `with_visible(false)` — the strip
  guard applies the empty region before the first `ShowWindow`; and
  `SetWindowRgn` is reapplied for `Role::Anchor` on EVERY pass (free on
  1×1) instead of only on style/size change. The ≤8 px guard stays so
  recycled HWNDs can't be blanked.
- **Consequences**: verified on macOS that an invisible root viewport
  keeps the event pump alive. Windows-side: needs a build/launch check
  on the Windows box.

## 2026-11-03 — Viewer bounds clamp replaces the hard move lock (user feedback)

- **Context**: the move lock (`setMovable(false)` / caption-drag
  swallow) made the viewer fully immovable — the user never asked for
  that; the requirement is only that the window can't be dragged
  beyond its monitor's limits (crossing monitors stays on the status
  strip buttons).
- **Decision**: the OS move lock is removed. The viewer callback now
  clamps the reported `outer_rect` to the CURRENT monitor's UI bounds
  every settled frame (`clamp_rect_inside` + `viewer_sync`), falling
  back to the monitor under the centre when the current one went away.
  `Settings.last_monitor` (stable id) is persisted alongside
  `rect_native` in the same debounced flush — written during a gesture
  but flushed to disk only after ~500 ms of quiet, so persistence lands
  at the END of a move. At launch the last monitor's saved rect is
  restored through the normal `viewer_target` path.
- **Consequences**: dragging inside the monitor is free again; the
  boundary is a rubber wall (one frame of lag). The macOS/Windows
  lock implementations were deleted — nothing platform-specific
  remains to verify for this.

## 2026-11-04 — Free cross-monitor drag; geometry written once at gesture settle

- **Context**: the monitor-boundary clamp made the viewer unmovable past
  its monitor's edge. An intermediate design snapped the window to the
  target monitor's memorized rect on drop — rejected by the user: a
  move must stay completely normal, only persistence reacts.
- **Decision**: `clamp_rect_inside` and its per-frame boundary push are
  removed — the OS moves the window, PixMe never interferes. The
  hosting monitor follows the window's physical position (centre
  hit-test on the cached inventory) in normal time; during the topology
  pin the PREVIOUS host is preferred so a mid-shuffle dump onto a
  reconnected display can't steal the window. `rect_native` and
  `last_monitor` are written exactly ONCE per gesture — when the
  reported `outer_rect` has been quiet for 250 ms (`viewer_move_at`,
  polled via `request_repaint_after`) — under the monitor the window
  actually sits on, so the status-strip monitor buttons always land on
  each display's last real position. The unplug restore (`viewer_target`
  arm) is gated on the previous host being disconnected — a normal
  drop never relocates.
- **Consequences**: dragging works like any OS window; prefs stay clean
  (single write per gesture, still disk-flushed via the existing
  `monitor_geom_dirty_at` debounce). Two self-deadlock traps documented
  for this loop: `if let` scrutinee `MutexGuard`s live through the
  whole body — never re-lock the same `Shared` mutex inside such a
  body.

## 2026-11-04 — Taskbar semantics for monitor buttons and the resident icon

- **Context**: the status-strip monitor buttons only relocated the
  viewer, and the resident icon's left click was a plain open/close
  toggle — the user wants per-monitor taskbar behavior (each macOS
  display carries its own menu bar / each Windows display its taskbar).
- **Decision**: `SelectMonitor` gained toggle semantics — clicking the
  monitor the viewer is shown AND focused on hides it (`ViewerClosed`);
  every other case (other monitor, viewer hidden in Idle, minimized,
  or behind another app) arms `viewer_target` for that monitor's saved
  rect and dispatches `OpenViewer` (which also un-minimizes and raises).
  `ToggleViewer` (resident icon left click) resolves the display under
  the cursor at click time via `platform::cursor_position` and forwards
  to `SelectMonitor` — falling back to the previous plain toggle when
  the cursor sits off every known display. Shared by the strip buttons:
  clicking the selected monitor's button now hides the viewer too.
- **Consequences**: one consistent gesture — "click the display's icon
  to summon PixMe there, click again to dismiss". Windows gets the same
  behavior through the shared command path.

## 2026-11-04 — Windows capture-overlay chrome: DWM border suppression + capture exclusion

- **Context**: three field reports on the region overlay. (a) Windows:
  a white line along the screen's top edge on every PC (also via RDP) —
  the DWM accent line drawn on borderless windows; the existing
  `SetWindowRgn` fix used by the countdown/pins cannot apply to the
  overlay (it would clip real frozen-frame pixels). (b) Both OSes: the
  crosshair/veil stops ~1 px short of the top edge — on Windows the DWM
  line covers that row, on macOS the nudge loop's 1 pt tolerance could
  leave a genuinely uncovered strip. (c) One Windows laptop: the torn-down
  viewer still appears as a watermark in the frozen frame — xcap reads
  the DWM composition via GDI `BitBlt`, and ~2 warm-up frames are not
  always enough for the composition to drop the window's pixels.
- **Decision**: in `strip_undecorated_frame`, the overlay gets
  `DwmSetWindowAttribute(DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE)` (Windows
  11; harmless elsewhere) instead of a window region. Every enumerated
  PixMe window EXCEPT the pins gets
  `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` — viewer, overlay,
  countdown, dialogs can never photograph themselves, whatever the
  teardown timing. Pins stay capturable: a pinned capture is screen
  content. On macOS the overlay-alignment tolerance tightens from 1.0 pt
  to 0.25 pt so the window must actually reach the monitor's top edge.
- **Consequences**: PixMe's own UI is now invisible to ALL screen capture
  tools on Windows 10 2004+, not only its own (the designed use of the
  API). The macOS nudge loop retries harder against the visibleFrame
  clamp — still capped by `MAX_NUDGE_TRIES`. `windows.rs` compiles only
  on Windows: verified on this branch's Windows build, not on macOS.
  **Superseded**: the permanent exclusion made PixMe invisible to Teams
  screen sharing and every other capture tool — replaced by the
  transient guard below (same day).

## 2026-11-04 — Transient capture exclusion around the freeze (+ DwmFlush)

- **Context**: the permanent `WDA_EXCLUDEFROMCAPTURE` above hid every
  PixMe window from ALL capture tools — Teams screen sharing and
  third-party screenshooters could no longer see the viewer.
- **Decision**: the flag is now armed only for the capture `BitBlt`'s
  duration, via `platform::capture_exclusion()` — an RAII
  `CaptureExclusionGuard` that (a) toggles the flag on every PixMe hwnd
  except pins by LIVE `EnumThreadWindows` enumeration (a window created
  since the last `strip_undecorated_frame` pass is covered — no registry
  staleness; pins are skipped via a `PIN_HWNDS` registry plus a live
  "PixMe Pin" title check for not-yet-classified pins), (b) calls
  `DwmFlush()` so the affinity change is inside the composed frame
  before the readback, and (c) lifts the exclusion on drop — error and
  panic paths included. The same guard is shared by all three capture
  sites (`screen::capture`, `capture_all_monitors`, `window::capture`).
- **Unverified assumptions — to validate on a Windows build**:
  1. `DwmFlush` lands early enough for DWM to honour the affinity change
     before xcap's `BitBlt` (DWM applies affinity at its own cadence).
  2. `BitBlt` being synchronous, the guard scope still covers the whole
     xcap grab on every path.
  Instrumented at debug level (`capture_exclusion_armed` /
  `capture_exclusion_lifted`: hwnd count, `DwmFlush` duration, active
  duration) — validate with `RUST_LOG=pixme=debug` on the laptop that
  showed the ghost.
- **Fallback**: if the ghost reappears despite `DwmFlush`, the next step
  is decided WITH the user — not implemented unilaterally.
- **WGC note**: Windows.Graphics.Capture also honours
  `WDA_EXCLUDEFROMCAPTURE` and cannot express "everything except my
  windows" — it does not solve this problem. Any WGC evaluation
  (latency, HDR, mixed-DPI monitors) is a separate track.
- **Consequences**: PixMe windows are only invisible for the grab's
  duration (ms) — imperceptible to Teams/others; macOS unchanged
  (no-op guard). `windows.rs` still compiles only on Windows.

## 2026-11-05 — Capture-bar icon styles (Standard / Color / Duotone)

- **Context**: the vbar's monochrome Lucide set read as austere; the user
  asked for a more colourful, modern look selectable in Settings.
- **Decision**: a persisted `icon_style` setting (`standard | color |
  duotone`, default `standard`) on the General page, applying to the
  vertical capture bar AND the horizontal toolbar (`tool_button`s) —
  the draw tools, tabs, menus and status strip keep the standard
  theme-tinted Lucide glyphs so the visual reference remains the
  identity. `Color` reuses the Lucide SVGs with a fixed per-icon hue
  (`theme::icon_hue`, theme-independent like the annotation palette);
  `Duotone` swaps in eighteen embedded Phosphor duotone SVGs (MIT,
  `assets/icons-duotone/`) — `timer` stands in for `Clock`,
  `app-window` for `SquareDashedMousePointer`, `rows` for `PanelTop`.
  Both coloured styles keep the widget-state overrides: disabled greys
  out, the armed delay marker stays `danger`. (First cut covered the
  vbar only; the user meant the horizontal toolbar — scope widened to
  both the same day.)
- **Consequences**: one more asset directory + licence
  (`LICENSE-phosphor.txt`), a `DUOTONE_DATA` table beside `DATA` in
  `icon_data.rs`, and `icons::styled_paint` + `icons::hue` behind
  `widgets::icon_button_styled` / `widgets::tool_button_styled` as the
  style-aware render path. Adding a fourth style means a new asset
  table plus a hue entry per styled icon.

## 2026-11-05 — Ghost fallback: early exclusion, real-teardown gate, countdown hides the viewer

- **Context**: the ghost reappeared on the user's fastest PC — the torn-down
  viewer watermarking the frozen frame, exactly the fallback case the
  2026-11-04 entry reserved for a user decision. Root cause confirmed: the
  BitBlt-time arming loses two races on a fast machine — a dead hwnd can't
  be flagged at all, and two warm frames can fit inside one DWM
  composition so `DwmFlush` returns before the flag/demolition is visible
  to the readback. With a capture delay the viewer additionally stayed
  mounted for the whole countdown and was destroyed at `CaptureNow`, a
  few frames before the freeze.
- **Decision** (agreed with the user — their own hypothesis confirmed):
  1. `begin_capture` arms `WDA_EXCLUDEFROMCAPTURE` while the displaced
     windows are still alive (`OverlaySession::early_excl`), held until
     the freeze lands — the flag is inside the composed stream before any
     teardown begins.
  2. `warm_freeze` no longer counts blind frames only: the freeze waits
     until `viewer_id`/`countdown_id` leave `raw.viewports` (winit-acked
     native teardown), with a ~2 s escape hatch against a stuck entry.
  3. Arming a delayed capture hides the viewer immediately
     (`Command::Capture` → `enter(Idle)` when the state is `Viewer` and no
     Draw modal is up); the origin is stashed in `capture_return_to` so a
     cancelled countdown restores it and a cancelled capture still lands
     back on the viewer.
- **Consequences**: shared-code change — macOS gets the same
  hide-at-arm behaviour (desirable anyway) and the teardown gate
  (harmless); the early arm is a no-op guard there. The transient
  exclusion's coverage is unchanged: still lifted the moment the frozen
  frames are taken, pins still photograph themselves.

## 2026-11-05 — Per-monitor Draw editor geometry (`draw_rect_native`)

- **Context**: the Draw editor always opened at egui's default placement;
  the user wants its position/size remembered per monitor across
  openings and sessions — centred on the monitor when nothing is saved,
  cleared by "Reset monitors" like the rest of the per-monitor state.
- **Decision**: `MonitorPrefs` gains `draw_rect_native` (same native-units
  `[x, y, w, h]` convention as `rect_native`). `OpenDraw` stashes a
  `draw_target` (the viewer's monitor + its saved rect, reused
  `ViewerTarget` shape); the draw viewport pass applies it once —
  saved rect through `target_ui_rect`'s clamped path, else centred via
  `monitor_ui_rect` — then tracks user moves with the viewer's
  last-outer/250 ms-quiet settle before writing the clamped rect under
  the monitor under the window's centre (`clamped_native_rect`, the
  DPI-inflation guard extracted from the viewer's inline block).
  `ResetMonitors` needs no extra code: `monitors.clear()` wipes the
  field with its entry.
- **Consequences**: draw geometry survives restarts and follows the
  display it lives on; the gesture settle reuses the debounced
  `monitor_geom_dirty_at` flush, no new save path. Shared code (works
  identically on macOS).

## 2026-11-07 — Second-instance relaunch surfaces the viewer (Windows named event)

- **Context**: on Windows, double-clicking the PixMe desktop shortcut while
  the app is resident shows the `app.already_running` dialog — the user
  wants the running instance's viewer to come forward instead. Earlier
  entries skipped foreground-raising ("no cross-process window handle
  without fragile Win32 code"); the agreed resolution is a named event,
  not window-handle hunting.
- **Decision**:
  - **IPC = one named auto-reset event** `Local\PixMeWake-<fnv1a64(key)>`,
    where `key` is `paths::instance_key()` — the same per-installation
    string the single-instance mutex uses, so the wake can only ever
    reach the SAME install's resident process (two portable folders keep
    independent sessions AND independent wake events). `Local\` scopes it
    to the user session; the kernel destroys the object with the last
    handle, so a crash leaves nothing stale. FNV-1a keeps the name far
    under the 260-char object limit on any path length.
  - **Resident side** (`platform::serve_instance_wakeup`, installed in
    `PixMeApp::new`): a parked `WaitForSingleObject` thread — zero
    wakeups while nothing happens — queues `Command::WakeViewer` through
    the same `queue_menu_command` + `wake_viewports` plumbing tray
    clicks use. `WakeupServer` is held by `PixMeApp` for the run; its
    `Drop` stops and joins the thread (tests use it).
  - **Signaler side** (`platform::signal_instance_wakeup`, in `main`
    before the dialog): `OpenEventW` retried ~3 s — the resident takes
    the guard in `main` but installs the listener once the event loop is
    up, so a quick relaunch can land in that gap — then
    `AllowSetForegroundWindow(ASFW_ANY)`: the just-launched process owns
    the foreground right `SetForegroundWindow` checks and hands it to
    every process, so the resident's `Focus` lands even while it sits
    behind other apps. Then `SetEvent` and a silent `exit(0)`. Any
    failure falls through to the existing translated dialog.
  - **`Command::WakeViewer`**: `enter(Viewer)` +
    `bring_viewer_forward` (un-minimize → `Focus` → `pending_focus`
    retries), then re-raises every live modal child (Draw, pin save
    dialog) ABOVE the viewer — deferred viewports are top-level windows,
    so a relaunch during Draw would otherwise strand the modal behind
    its own dimmed parent. Ruling: enabled under every modal except
    `Capturing` (stealing the overlay's focus would eat its Esc/clicks)
    and `Saving` (the quit flush rejects everything anyway). It is
    unreachable from menus — no menu id maps to it.
  - `paths::instance_key()` single-sources the `\`→`/` derivation the
    guard and the event share.
- **Rejected alternatives**: a socket or named pipe (port/handle
  lifecycle, a listener thread anyway, heavier); no IPC at all (the
  dialog — the actual complaint); `single-instance`'s API has no
  callback (recorded 2026-10-25); posting a registered Win32 message to
  the resident's windows (needs HWND discovery — the same fragile code
  foreground-raising was skipped over); the signaler calling
  `CreateEventW` itself (a set event dies with the signaler's handle
  before the resident can exist — the race remains, `OpenEvent`+retry
  is deterministic).
- **macOS is not concerned**: a relaunch can never spawn a second
  process — LaunchServices delivers `applicationShouldHandleReopen:` to
  the RUNNING app, and the Dock-reopen hook already queues `OpenViewer`.
  `serve_instance_wakeup`/`signal_instance_wakeup` are `None`/`false`
  stubs there, so the `app.already_running` dialog stays the
  (unreachable) fallback with zero IPC machinery — no socket, no
  thread.
- **Still to validate on Windows** (manual): relaunch from the shortcut
  in four states — viewer open, viewer minimized, viewer closed with
  PixMe in the notification area, and during an open Draw. Every time
  the viewer (plus its modal) must land frontmost, never behind, with
  no dialog. Edge cases worth a look during validation: foreground
  rights while a UAC prompt or another always-on-top app holds the
  foreground (`ASFW_ANY` should cover the standard cases), and the
  ~3 s signal-retry window during a cold resident start.

## 2026-11-07 — Global hotkeys via `global-hotkey` crate

- **Context**: two configurable OS-wide shortcuts (Open PixMe, capture a
  rectangular area) that fire while every PixMe window is hidden.
- **Decision**: `global-hotkey` 0.9 (`serde` feature) — already in the
  approved dependency list — wraps `RegisterHotKey` (Windows) and Carbon
  `RegisterEventHotKey` (macOS, no Accessibility permission). One
  `HotkeyManager` (`src/hotkeys.rs`) owns the registrations and an
  `id → action` index; a listener thread on
  `GlobalHotKeyEvent::receiver()` pushes the matching `Command`
  (`OpenViewer`, `Capture(Region)` via `push_capture`, which records the
  viewer's monitor like a bar click) and calls `wake_viewports` — egui
  does not repaint while hidden. Bindings persist as the crate's
  accelerator strings in `Settings::hotkeys`; defaults are EMPTY (no
  surprise conflicts). Conflict policy: internal-duplicate check, a
  per-OS reserved list (Cmd+Shift+3/4/5 on macOS — project rule — plus
  Cmd+Space/Tab/Q/W/H/M; Win+L/D/E and Ctrl+Alt+Del/Esc on Windows), and
  the authoritative OS register attempt — a replacement only unregisters
  the old combo AFTER it registers successfully; failure keeps the
  previous binding and surfaces a per-action error. `HotkeysChanged`
  re-syncs on settings apply; the manager's `Drop` unregisters at quit.
- **Consequences**: new actions need a `HotkeyAction` variant + a
  settings row (extensible by design). Known limit: the Windows/Super
  modifier cannot be captured through egui input (not reported as a
  modifier), so Win-combos are unbindable via the recorder.

## 2026-11-07 — Hotkey probe must reuse the app's single manager

- **Context**: recording a combo in the settings never stuck (field
  dropped back to "Unset") on macOS. The recorder validated each combo
  with `HotkeyManager::probe`, which built a throwaway
  `GlobalHotKeyManager` per attempt — and `GlobalHotKeyManager::new`
  calls Carbon `InstallEventHandler` on the application target, which
  fails the second time (same handler, same event types), so every
  probe errored with `Taken` once the app's real manager was installed.
- **Decision**: `probe` is now a `&self` method that registers and
  immediately unregisters the candidate on the app's single manager;
  the listener cannot fire because the `id → action` index is never
  touched. Re-recording the exact combo an action already holds skips
  the probe (the same-process register would legitimately report
  `Taken`). A missing manager (init failure) reports `Taken`.
- **Consequences**: macOS can host exactly ONE
  `GlobalHotKeyManager` per process — never create a second one.
  Verified end-to-end: record → OK → `prefs.json` → re-arm → the combo
  fires `OpenViewer` while the app is running.

## 2026-11-07 — macOS hotkey "Test" button (silently-taken combos)

- **Context**: a combo owned by another app (swallowed by a
  head-insert event tap) records, arms and persists without ever
  firing — Carbon `RegisterEventHotKey` accepts registrations it
  cannot detect as taken, so neither the probe nor the re-arm reports
  it. User hit exactly this with Cmd+Shift+A. An earlier design
  forced a confirmation press right after each recording — rejected
  as awkward UX.
- **Decision**: each bound field on the Hotkeys settings page gets a
  "Test" button (macOS only). Clicking it opens a 5 s listen window:
  `Shared::hotkey_verify` holds the combo's id, the listener consumes
  the matching press WITHOUT dispatching its action, and the field
  shows "press the shortcut…". A received press toasts "shortcut
  works"; a timeout toasts "probably used by another application".
  A combo not yet armed is registered transiently
  (`register_verify`) for the window; an armed one is only flagged in
  the consume slot — the lib keeps a single Carbon ref per id, so
  re-registering would orphan the live registration. No synthetic
  events, no Accessibility permission — the user's own press is the
  probe. Windows skips it (`RegisterHotKey` fails when taken).
- **Consequences**: recording stays one press (no forced confirm);
  silently-taken bindings are diagnosable on demand instead of being
  caught at record time. Startup re-arm still cannot detect them.

## 2026-11-08 — macOS display captures read the framebuffer (Sidecar cursor)

- **Context**: with an iPad connected via Sidecar, the user's cursor was
  baked into region captures even though `CGDisplayHideCursor` returned
  success on every display. Diagnosis pointed at the
  `CGWindowListCreateImage` window-list composite (`OptionAll`,
  hardcoded by xcap): Sidecar's cursor mirroring plausibly surfaces as
  a list entry the hide cannot reach — an unproven theory, but every
  alternative (wrong display, stale index, second capture path) was
  ruled out. `CGAssociateMouseAndMouseCursorPosition` was rejected
  (freezes motion without hiding) and ScreenCaptureKit would add a
  dependency the user does not want yet.
- **Decision**: monitor frames on macOS come from
  `CGDisplayCreateImage` (`platform::macos::capture_frame`) — a
  framebuffer read with no window list, so no list-resident cursor
  artifact can leak in. Only what is actually on screen is captured,
  which also removes the `OptionAll` offscreen-window ghosting. The
  conversion to RGBA goes through a shared bitmap-context helper
  (`cg_image_to_rgba`, factored out of `cursor_snapshot`). If a display
  cannot produce a framebuffer image (some virtual displays), the code
  falls back to the xcap window-list path. Windows keeps xcap
  unchanged; window captures (`CGWindowListCreateImage` on a single
  window) are untouched — there is no framebuffer-level equivalent for
  one window's backing store.
- **Consequences**: no new dependency (`core-graphics` was already
  direct). `capture_exclusion` stays a macOS no-op — framebuffer reads
  cannot see a torn-down viewer. Needs on-device verification with
  Sidecar attached: if macOS ever bakes the cursor into the
  framebuffer itself, the fallback won't help either and
  ScreenCaptureKit (`showsCursor = false`) becomes the only remaining
  fix.

## 2026-11-08 — Monitor ghosts only for mid-session disconnects

- **Context**: every persisted monitor (prefs.json `monitors`) absent
  from the live inventory produced a greyed status-strip ghost at
  launch — stale entries accumulated forever (dismissal was
  runtime-only, so they came back on every start) and even the ↻
  refresh could not clear them. The "greyed until clicked" rule made
  sense for a monitor that disappears mid-session but not for entries
  that were never connected this run.
- **Decision**: a ghost row now requires the monitor to have been SEEN
  connected during the current run (`Viewer::seen_monitors`, fed by
  every inventory that lands). Absent-at-launch prefs entries produce
  no row at all — the strip effectively verifies monitor presence at
  startup. Mid-session disconnects keep the greyed-until-clicked
  affordance, prefs are still never dropped, and a reconnect brings
  the monitor back as a normal connected button.
- **Consequences**: `monitor_rows` takes a `seen` set in addition to
  `dismissed`; the launch-time strip only shows real monitors. Users
  who want a gone monitor's button back must reconnect it — the
  rename affordance on never-seen entries is gone with the row.

## 2026-11-08 — Workspaces: named capture buckets, filter not reload

- **Context**: every capture lived in one flat pool, with the naming
  mask, its `#` counter and the last-used save folder all global in
  `prefs.json`. The feature brief asked for minimal "workspaces" —
  each with its own mask, counter and output folder — without making
  daily use more complex.
- **Decision**:
  - `Workspace { id, name, name_mask, output_dir, counter }` lives in
    `prefs.json` (`workspaces` + `active_workspace_id`); `main` is the
    reserved, undeletable-but-editable default, seeded once from the
    legacy global `filename_template`/`filename_counter`/`last_save_dir`
    keys, which are `skip_serializing` now (read once, never written
    back). The mask preset bank (`filename_templates`) stays a single
    shared list — presets are a bank, not per-workspace state.
    `Settings::save` writes atomically (temp file + rename).
  - `workspace_id` is a REAL SQL column on `documents` (schema 15)
    with an index — never a postcard field, since extending a
    postcard struct breaks decoding of old rows. The 14 → 15 upgrade
    is the first in-place migration: `ALTER TABLE … ADD COLUMN`
    instead of the drop-and-rebuild every other version bump takes,
    so existing captures keep their rows (assigned `main`) and no
    `.bak` is produced.
  - `last_save_dir` is gone as a concept: the workspace's `output_dir`
    inherits its role — proposed save folder, updated to the folder
    actually used after each successful save of one of its tabs.
    Pin saves seed from the active workspace but update nothing (a
    pin owns no workspace). There is no auto-save to disk and none is
    wanted: `output_dir` seeds the save dialog and targets Save All.
  - The viewer's `tabs` still holds the WHOLE session (persistence,
    dirty flags, store sigs unchanged); the workspace switch is a
    filter (`visible_indices`/`tab_ws`), never a reload. Every
    push path stamps the active workspace on the document; a tab
    tagged with an id prefs no longer lists resolves to `main` so a
    prefs reset never hides captures. Close-* menu entries, toolbar/menu
    Save All and tab cycling act on the visible set only — hidden
    workspaces' captures survive a "close all" / batch save. Quit-time
    `ConfirmSave` (`db_retention = Never`) is the deliberate exception:
    since quitting discards the entire in-memory session, it writes
    every unsaved capture across all workspaces into each workspace's
    own `output_dir`.
  - "Empty" = zero live tabs; closing tabs frees the workspace for
    deletion (there is no soft-delete: a closed tab's row is already
    gone, and `writer.delete_workspace` hard-purges stranded rows as
    a belt-and-braces `DELETE WHERE workspace_id`). Deletion never
    touches files or folders on disk.
  - UI: a compact bordered chip at the leading edge of the status
    strip (left of the capture counter/metadata) shows the active
    workspace's name (elided, full name on hover), sized to a fixed
    width matching the longest workspace name (names are capped at 30
    characters in the dialog) so switching never shifts the status bar.
    Whenever the active workspace is not `main`, the chip wears an
    accent pill (`accent_soft` fill, `accent_border` stroke, `accent`
    folder icon, Inter SemiBold `accent_text` label measured in the
    rendered font). Left click lists/switches workspaces and offers
    "New workspace…"; right click opens the icon-prefixed management
    menu (`Plus` New / `Pencil` Edit / `Trash` Delete — greyed on
    `main` or non-empty — / `Broom` Delete empty workspaces with a
    toast). The create/edit dialog is an in-app `modal_frame` reusing
    the settings window's mask editor verbatim, moved out of the
    settings sidebar (the File name page is gone).
- **Consequences**: purge/retention stays global across workspaces
  (test-pinned). Counters, masks, save folders and image formats are
  independent per workspace; collisions between workspaces sharing a
  folder are resolved at write time by `unique_file`'s `_2`/`_3`
  suffixing under each workspace's own extension. No keyboard
  shortcuts, no workspace colours, no moving captures between
  workspaces.

## 2026-11-11 — Duplicate names are workspace-scoped; per-workspace save format

- **Context**: title uniqueness was global — a capture named `a` in
  `main` made a capture named `a` in another workspace look
  "duplicated" (danger-tinted title, read as *unsaved*), even though
  each bucket may target its own folder. Cross-workspace dup tracking
  proved too noisy to be useful (the twin lives on a hidden tab).
- **Decision**: a name clash is only a problem INSIDE one workspace
  (same name + same folder = a real file collision): `title_taken` /
  `unique_title_in` filter by the tab's workspace, the tab chip and
  mosaic caption show an amber `files` icon (`viewer.tab.dup_name`
  tooltip) instead of the danger title tint, and `save_all` still
  blocks same-workspace twins behind the rename modal. Different
  workspaces may reuse names freely — `unique_file` writes
  `stem.ext`, `stem_2.ext`… under each workspace's `format`
  (PNG/JPEG/WebP, chosen in the workspace dialog; the single-file
  save dialog keeps its own format radio, seeded with it). The
  `files` icon (Lucide) is bundled for the marker. Quit-time
  `ConfirmSave` is exempt from the rename gate: with
  `db_retention = Never` the session is wiped on exit, so it is a
  save-at-all-costs path — same-workspace twins also land via the
  `_2`/`_3` suffixing instead of trapping the user in a rename loop.
- **Consequences**: renaming a workspace's output folder or format
  needs no dedup bookkeeping — clashes are recomputed per frame from
  (workspace, title) pairs only. Same-workspace duplicate tabs still
  require a rename before an interactive Save All.

## 2026-11-11 — macOS AppKit `terminate:` routed through `Command::Quit`

- **Context**: quitting PixMe from the macOS Dock menu (or a `Cmd+Q`
  that fell through to AppKit) invoked `-[NSApplication terminate:]`
  directly. Because `winit` 0.30 does not implement
  `applicationShouldTerminate:`, the process exited without dispatching
  `Command::Quit`, bypassing the Save/Discard/Cancel confirmation when
  `db_retention = Never`.
- **Decision**: `platform::macos::install_ui` replaces
  `-[NSApplication terminate:]` via `class_replaceMethod` with a
  handler that queues `Command::Quit` and wakes the egui viewports;
  when `Command::Quit` arms the dirty-session confirmation, it also
  calls `bring_viewer_forward` so a minimized or backgrounded viewer
  is de-minimized and focused.
- **Consequences**: every quit gesture on macOS (PixMe menu, `Cmd+Q`,
  Dock Quit) and Windows (viewer menu, tray "Close PixMe") flows
  through the same `Command::Quit` funnel and honours the unsaved
  confirmation.

## 2026-11-12 — Moving captures between workspaces

- **Context**: follow-up to the workspaces feature — captures could be
  bucketed at creation time but never re-assigned afterwards.
- **Decision**: the capture context menu (tab strip AND mosaic — they
  already share `Viewer::tab_menu`) grows a "Workspace →" submenu right
  under "Rename", listing every workspace but the capture's own,
  `main` first then alphabetical (the new shared
  `config::workspace::ordered` helper also drives the status-strip
  switcher, which now marks the active workspace with a check icon).
  A move updates `doc.meta.workspace_id` in memory and enqueues a
  dedicated `Job::Move` on the store thread — ONE transaction via
  `Store::move_documents` — rather than riding the meta-drift path
  (`MetaRow` deliberately stays workspace-less so a parked meta job
  can never resurrect a stale bucket; a parked `Doc` snapshot gets
  its `workspace_id` re-tagged in place). On failure the job is
  dropped, not parked, and `WriteEvent::MoveFailed` makes the UI roll
  the capture back into its source workspace behind a localized error
  toast — a late flush success could contradict what the user sees.
  Files on disk, names, annotations and mask counters are untouched:
  a re-save still overwrites `source_path` in place, while Save All
  already resolves each tab's own workspace folder/format.
- **Consequences**: the store layer gains a third per-workspace verb
  (`Move`) with explicit revert semantics; the single-selection
  viewer applies it to one capture but the job is batch-shaped if a
  multi-selection ever lands. No undo, no drag-and-drop — out of
  scope per spec.

## 2026-11-12 — Duplicate annotation (Ctrl/Cmd+D)

- **Context**: the draw editor needed a Duplicate action for the
  selected annotation (hotkey, strip button, context-menu entry) —
  single selection only.
- **Decision**: `DrawSession::duplicate_anno` clones the annotation
  with a fresh id, offsets it `DUP_OFFSET` (24 px — 12 read as nearly
  invisible at 1:1) down-right in image
  coordinates — per-axis flipped or clamped by `dup_delta` so the copy
  stays inside the picture at any zoom — and commits ONE
  `AnnoEdit::Add`. A cloned Step badge takes the next counter of its
  mode via `next_step_counters` (bank + live fields reseeded like
  `open` does). Undoing the clone restores the previous selection
  through a session-only `dup_sources: HashMap<copy_id, source_id>`
  consulted in `undo_once`/`redo_once` — chosen over adding a provenance
  field to `AnnoEdit`, which is postcard-serialized into `session.db`
  and cannot grow a field without breaking stored undo stacks. The
  strip button reuses `Icon::Copy` (Lucide overlapping rectangles —
  the exact duplicate glyph) inside the same `icon_button` widget,
  greyed via `add_enabled_ui` when nothing is selected.
- **Consequences**: duplication is a plain reversible edit — undo
  restores the source selection, redo re-selects the clone; repeated
  duplications staircase off the latest copy. `dup_sources` is
  session-scoped: undoing a pre-restore duplicate after re-entering
  the editor just clears the selection (previous behaviour).

## 2026-11-13 — Full-screen eyedropper via a transparent overlay

- **Context**: the Pipette tool and swatch pickers needed to sample any
  pixel on the desktop (other apps, menu bar, all monitors) while the
  Draw window stays visible, with the validating click swallowed and
  PixMe's own cursor/loupe kept everywhere.
- **Decision**: interaction lives in a borderless transparent
  always-on-top deferred viewport (`pixme.pick_overlay`) covering the
  monitor union — raised only while a pick is armed AND the cursor
  roams outside the Draw window (inside, the canvas pick path keeps
  ownership). An alpha-1 veil fills the frame: fully-transparent pixels
  are click-through on both OSes, so the veil is what actually swallows
  the click. The overlay paints PixMe's loupe card at the cursor and
  pushes a `PickOutcome` through ctx data (`PICK_SCREEN_RESULT`) that
  the Draw session drains. Pixel reads never go through the overlay:
  `platform::picker` samples the real framebuffer
  (`CGDisplay::image_for_rect` on macOS — the capture permission path
  already granted — `GetDC(NULL)`/`GetPixel` on Windows).
- **Rejected**: `NSColorSampler` (macOS) + `WH_MOUSE_LL` (Windows) —
  the sampler imposes its own round OS loupe instead of PixMe's cursor,
  cannot be dismissed programmatically, and a nil callback cannot be
  told apart from a lost session; the hook needs dedicated-thread
  plumbing for something the overlay does for free. First implemented
  then replaced after user testing.
- **Consequences**: identical interaction on both OSes (one code path);
  `objc2`/`objc2-app-kit`/`block2` dependencies were added then removed
  — none shipped. macOS without Screen Recording gets a localized toast
  pointing at System Settings (`PickOutcome::Denied`). Windows pixel
  reads use the virtual-screen space (negative coordinates supported);
  the overlay's rect is re-pinned to the monitor union every frame.
