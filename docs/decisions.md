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
  the **same size and position** as the previous one (FastStone parity).
- **Implementation**: `Shared.last_fixed_region` stores the confirmed
  selection in monitor-logical space (`selection + screen_offset`).
  `begin_capture` passes it to `OverlaySession` as `restore_fixed`; the
  overlay materializes it (`translate(-screen_offset)`) once the real
  window↔monitor offset is known, clamped to the visible bounds via
  `slide_inside`. `placed` starts `true` so the frame never rides the
  cursor — it stays movable/resizable; whatever is confirmed becomes the
  new memory.
- **Scope**: session memory only (survives bar/viewer cycles, cleared on
  quit). Persisting to `settings.toml` like FastStone is a possible
  follow-up.

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
- **Stamps** recolour embedded Lucide SVGs (`stamp_icon` map + `raw_svg` accessor) — no FastStone assets.
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

- **Context**: FastStone loses every capture + annotation at quit;
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

