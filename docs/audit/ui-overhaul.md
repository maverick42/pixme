# PixMe UI Overhaul — consolidated audit & architecture proposal

Status: **design document — no implementation started**. Branch `feature/ui-overhaul`.
This document supersedes ad-hoc chat output and is the reference for the rework.
Every decision below is either a code fact (verified during the audit) or an
explicit user decision (marked **[decision]**). Points still open are listed in §12.

---

## 1. Current architecture (audit findings)

### 1.1 State machine and viewports

One shared lifecycle machine in `src/app.rs`: `Idle | Bar | Capturing | Viewer`,
with `Bar` and `Viewer` mutually exclusive. Every user action is a `Command`
dispatched through `Shared::pump()`/`dispatch()` (`src/app.rs:562-1127`).

Viewport topology: the egui root viewport is a permanent **invisible 1×1 anchor
window** (`taskbar(false)`, transparent, always-on-top) that keeps the event pump
alive. All real windows are **deferred child viewports** registered/deregistered
each frame according to state (`src/app.rs:2050-2552`):

| Viewport id        | Role                                  | Lifetime              |
|--------------------|---------------------------------------|-----------------------|
| ROOT               | 1×1 invisible anchor                  | always                |
| `pixme.bar`        | floating capture bar                  | state `Bar`           |
| `pixme.viewer`     | main window                           | state `Viewer`        |
| `pixme.capture_overlay` | region-selection overlay         | state `Capturing`     |
| `pixme.countdown`  | capture-delay digit                   | outside the machine   |
| `pixme.pin_<N>`    | pinned captures (N dynamic)           | outside the machine   |
| `pixme.save_dialog`| save-as window                        | `ModalState::SaveDialog` |
| `pixme.draw`       | annotation editor                     | `ModalState::Draw`    |

Modality is governed by `ModalState` (Settings / SaveDialog / Draw / ViewerDialog /
Capturing / Saving) and a single eligibility table `is_command_enabled`
(`src/app.rs:195-248`) shared by native menus, tray items and every button.

### 1.2 Feature surfaces

- **Floating bar** (`src/ui/toolbar.rs`): drag grip, 5 capture buttons
  (WindowOrObject, Region, FullScreen, FixedRegion, Pin), delay menu
  (0 / 5 s / 1–9 s), options menu (`include_pointer`, `copy_to_clipboard`),
  close X. Borderless/transparent/always-on-top viewport, `SHADOW_MARGIN` 14 pt,
  `MENU_RESERVE` 360 pt, `ViewportSync` geometry ledger.
- **Viewer** (`src/ui/viewer.rs`): egui menu strip on Windows, toolbar strip
  (Ouvrir▾ / Save / Zoom± / Zoom%▾ / Mosaic toggle / Draw / Resize / Copy /
  Print / Close), tab strip with a trailing "+" button, content area (canvas
  fit/zoom/pan or mosaic grid with a trailing "+" tile), status strip (count,
  dimensions, date, file size, sort mode, version, DB read/write dots),
  dialogs (rename, resize, protect, delete-file, unsaved confirm, dup names),
  settings modal (General/Filename/Folders pages), About modal (Windows),
  keyboard shortcuts (⌘/Ctrl+C, ⌘Z/⇧⌘Z/⌘Y, ⌘S/⇧⌘S, ⌘O, ⌘P, ⌘⌥V, ⌘`,`, ⌘Q).
- **Draw** (`src/ui/draw.rs`): annotation editor, own viewport, modal to viewer.
- **Capture overlay** (`src/ui/capture_overlay.rs`): freezes ALL monitors at
  once (`capture_all_monitors`), follows the cursor across displays while no
  selection has started (`can_roam()`, lines 249-258), locks to the monitor
  where the first drag begins; 8 resize handles, NEAREST magnifier + `#RRGGBB`
  + `C` copy, Esc/Enter/double-click semantics, `WindowOrObject` pick mode,
  blind-drag handling for OS-eaten edge presses, synthetic-Esc grace window,
  macOS nudge loop fighting `visibleFrame` clamping (lines 461-477).
- **macOS platform** (`src/platform/macos.rs`): template status item (menu:
  `take_capture`, `open_viewer`), native app menus (PixMe/File/Edit/Capture/
  Window) via muda `init_for_nsapp`, capture kiosk via
  `setPresentationOptions` (HideDock | HideMenuBar | DisableProcessSwitching |
  DisableHideApplication — `capture_chrome_enter`, lines 1121-1139), overlay
  elevation to `kCGScreenSaverWindowLevel` (1000), `dismiss_active_menus`
  (synthetic Esc to third-party popup menus at layer ≥ 100), single-pass
  `CGWindowListCopyWindowInfo` window list, `NSCursor` snapshot, per-display
  `CGDisplayHideCursor`, `NSPrintOperation` print.
- **Windows platform** (`src/platform/windows.rs`): notification-area tray
  (`take_capture`, `open_viewer`, `quit`), `strip_undecorated_frame` —
  `WS_CAPTION` stripping, `SetWindowRgn` shaping, `ShowWindow` bypassing
  winit, roles Bar/Countdown/Pin/Overlay/Anchor (lines 211-441), `EndMenu`
  dismissal, `GetAsyncKeyState` edge-drag polling, `PrintDlgW` print,
  `PlaySoundW` tick, portable data dirs.
- **DPI layer** (`src/platform/mod.rs`): `NATIVE_SPACE` (Physical on Windows,
  Points on macOS), `monitor_physical_rect`, `winit_monitor_index`,
  `monitor_ui_rect`, `window_ppp`, `ui_to_native`/`native_to_ui` — all
  conversions use the **receiving window's own ppp**, never the monitor's.
- **Persistence**: `prefs.json` (`src/config/settings.rs`), `session.db`
  SQLite with a dedicated write-through writer thread (`src/store/`),
  `prefs.json` legacy migrations, `drag_out` OS files in temp dir.
- **Single instance**: `single-instance` lock on `data/instance.lock`.

### 1.3 All current capture triggers (verified exhaustive)

| Trigger | Location | Maps to |
|---|---|---|
| 5 mode buttons | `src/ui/toolbar.rs:126-152` | `push_capture(mode)` |
| "+" tab-strip button | `src/ui/viewer.rs:2595-2600` | `Command::ShowBar` |
| "+" mosaic tile | `src/ui/viewer.rs:3184-3224` | `Command::ShowBar` |
| `take_capture` | status item + tray menu (`macos.rs:192`, `windows.rs:87`) | `Command::ShowBar` |
| `capture.launch` | macOS Capture submenu + Windows egui Capture menu (`macos.rs:339`, `viewer.rs:2496-2507`) | `Command::ShowBar` |
| `PIXME_CAPTURE`, `PIXME_CAPTURE_AFTER` | dev env hooks (`app.rs:1942-2037`) | `Command::Capture(mode)` |

No keyboard shortcut triggers a capture anywhere (verified). No global hotkey
is wired — `global-hotkey` is declared in `Cargo.toml` but never used.

---

## 2. Target architecture

### 2.1 Simplified state machine

`Idle | Capturing | Viewer`. State `Bar`, commands `ShowBar`/`CloseBar`,
`Shared::{toolbar, bar_pos, bar_rect, bar_monitor}`, `bar_id()`, `bar_builder()`,
`ui_pos_on_anchor`, `Toolbar::place_on_monitor`/`reset_sync` are **deleted**.

`return_to` on a capture session simplifies to `{Viewer, Idle}`: captures are
launched from the viewer only, so they always return to `Viewer` (dev hooks
may land on `Idle`).

### 2.2 The viewer's vertical capture bar ("vbar")

Layout (inside the existing `CentralPanel`, `SidePanel` registered before
content so it always owns its zone):

```
┌──────────────────────────────────────────────┬────┐
│ (Windows only) egui menu strip               │    │
│ toolbar strip (Ouvrir/Save/Zoom/Mosaic/…)    │ vb │
│ tab strip (NO "+" button)                    │ ar │
│                                              │    │
│   content: canvas OR mosaic grid (NO "+")    │    │
│                                              │    │
│ status strip: info | monitor buttons + ↻     │    │
└──────────────────────────────────────────────┴────┘
```

- `egui::SidePanel::right("viewer.vbar")`, fixed width (~36–40 pt), `Frame`
  fill `t.dock`, one `hairline` separating it from the content. egui gives us
  all required properties for free: fixed zone, never overlapped by the zoomed
  canvas `ScrollArea`, always visible in both view modes — no viewport, no
  chrome, no DPI surface.
- Buttons, top → bottom (all `widgets::icon_button`, tooltips translated):
  the 5 capture modes, `vline`, clock (delay popup), `vline`, gear (options
  popup). `delay_menu`/`options_menu` bodies move verbatim from
  `toolbar.rs:187-255`; `egui::Popup::menu` works unchanged inside the panel.
- Buttons read `shared.command_enabled(&Command::Capture(mode))` so they grey
  out under modals, same as menu items today.

**[decision]** Captures are triggered **exclusively** from the vbar:
- macOS native `Capture` submenu → deleted entirely (`macos.rs:334-343`).
- Windows egui `Capture` menu in `menu_strip` → deleted (`viewer.rs:2496-2507`).
- `take_capture` status/tray entry → removed (new tray menu in §2.6).
- Tab-strip "+" and mosaic "+" tile → deleted; nothing replaces them — the
  vbar covers both view modes.
- `command_from_menu_id`: `take_capture` and `capture.launch` mappings removed.
- `Command::ShowBar`/`CloseBar` deleted; `capture.launch`'s old role (open the
  mode picker) has no equivalent — the vbar IS the picker, always visible.
- Dev hooks `PIXME_CAPTURE`/`PIXME_CAPTURE_AFTER` stay (debug only, not user
  UI — flag in open points).
- `ImportClipboard` stays in File/Ouvrir menus: it creates a document from
  clipboard content, it is a file action, not a screen-capture trigger.

### 2.3 Monitor management

**Inventory**: `capture::screen::monitors()` at startup (already called in
`log_banner`, `app.rs:1898`) + a **Refresh** button in the status strip that
re-enumerates and reconciles.

**Stable monitor identity** — needed for persisted names and per-monitor
geometry that survives disconnect/reconnect:

- macOS: `xcap::Monitor::id()` = `CGDirectDisplayID` (stable per session).
  Better: `CGDisplayCreateUUIDFromDisplayID` — UUID derived from
  vendor/model/serial, persists across reconnects and reboots. Implement in
  `platform/macos.rs` (FFI pattern already established there).
- Windows: xcap ids are **not stable across enumerations** (already noted at
  `app.rs:1871-1872`). Use `EnumDisplayDevices` with
  `EDD_GET_DEVICE_INTERFACE_NAME` → the monitor's PNP DeviceID (stable per
  physical monitor on the same port; may change on port change — acceptable,
  document it). Implement in `platform/windows.rs`.
- Model: `struct MonitorIdentity { stable_id: String, info: MonitorInfo }`;
  `platform::monitor_identities() -> Vec<MonitorIdentity>`; fallback key
  `rect:{x},{y},{w}x{h}` when no stable id can be read.

**Persistence** — in `prefs.json` (settings, not `session.db`):

```json
"monitors": {
  "<stable_id>": {
    "name": "Dell bureau",
    "rect_native": [x, y, w, h],
    "draw_rect_native": [x, y, w, h]
  }
}
```

`rect_native` is stored in the **monitor's native units** (physical px on
Windows, points on macOS), independent of any window ppp — conversion happens
at apply time with the receiving window's own scale (`monitor_ui_rect`).
This removes the whole class of mixed-DPI move bugs: nothing volatile is ever
stored. `draw_rect_native` is the same convention for the Draw editor window
(`None`/absent = centred on its monitor at open).

**Status-strip monitor row**: one `icon_button` (`Icon::Monitor`) per known
monitor, active state = the viewer's current monitor; tooltip = custom or OS
name; a `Refresh` button (`Icon::RefreshCw`) re-scans. Right-click on a
monitor button → rename dialog (reuse the `rename_dialog` pattern); the name
is persisted in the `monitors` map.

**Viewer locked to its current monitor** [decision]:
- macOS: `setMovable(false)` on the viewer's NSWindow — the window cannot be
  user-dragged at all but stays resizable; programmatic `setFrame`/
  `OuterPosition` still works. Reachable via `objc_msgSend` on the NSWindow
  found through `NSApp.windows` (same enumeration trick as `elevate_overlay`;
  match the window titled "PixMe" that is NOT the topmost anchor). Apply once
  on viewer creation and re-assert after `enter(Viewer)` since the viewport
  is destroyed/recreated on every state entry.
- Windows: the user decision was "Topmost + re-focus, no `WH_KEYBOARD_LL`"
  — that ruling concerns **capture modality** (Alt+Tab during selection), see
  §2.5. For the viewer-move lock itself the parity-equivalent of
  `setMovable(false)` is a `SetWindowSubclass` on the viewer HWND swallowing
  `WM_NCLBUTTONDOWN` when `HITTEST == HTCAPTION` (and/or `WM_SYSCOMMAND`
  `SC_MOVE`) — the window becomes immovable by dragging while remaining
  resizable through its borders (`HTLEFT/…` untouched). Same FFI style as
  `strip_undecorated_frame`. **Open point #1** (alternatives: clamp in
  `WM_WINDOWPOSCHANGING`, or no lock at all).
- Manual drag between monitors is deliberately removed from the UX; monitor
  buttons are the only relocation mechanism. Selecting a monitor applies the
  saved `rect_native` for that monitor (first visit: centre with the window's
  current size, clamped inside the monitor) via a viewer-dedicated
  `ViewportSync`, position+size computed with the receiving window's ppp and
  a one-frame settle before `viewer_rect` is re-sampled from reported rects.

**Disconnected monitors**: keep them listed, visually marked (greyed icon +
"disconnected" tooltip); Refresh updates `connected` flags but never drops
stored data. Clicking a disconnected monitor's button removes the button from
the UI but **keeps its `prefs.json` entry** (name + geometry survive for the
next reconnect). The disappearance is visual only — persisted info is never
lost.

### 2.4 Simplified geometry/DPI story

- The manual-drag-across-monitors scenario disappears by design (§2.3).
- Programmatic viewer moves: saved native rect → convert with receiving
  window's ppp → `OuterPosition` + `InnerSize` through a `ViewportSync`
  ledger (dedup against last *requested* geometry, never reported rects —
  `src/ui/viewport_sync.rs` already encodes exactly this fix).
- `viewer_monitor` resolution already derives from the window's real outer
  rect centre (`app.rs:2113-2119`) — keep; add a settle frame after issuing a
  move before re-sampling `viewer_rect` so transient DPI rects are never
  persisted.
- Mouse-anchor offsets during capture are already handled: `screen_offset`
  tracks real window origin vs monitor origin every frame
  (`capture_overlay.rs:403-414`), `img_ratio` derives image-px-per-window-pt
  from the actual rendered rect (`img_pos`, lines 604-626), and blind-drag
  polling (`left_mouse_down` + `cursor_position`) covers OS-eaten presses.

### 2.5 Capture cycle

**Start monitor** [decision]: the overlay opens on the monitor **selected in
the viewer** (= the monitor the viewer sits on, since selecting a monitor
moves the viewer there — single source of truth: `viewer_monitor` /
`capture_start_monitor` fed by the vbar's `push_capture`). Change
`warm_freeze`'s initial-monitor priority (`capture_overlay.rs:289-308`) from
cursor-first to session-monitor-first, keeping cursor/primary as fallbacks.

**Cursor roaming vs selection confinement** [decision]: already correct in
code — while no selection exists, the overlay follows the cursor across
monitors (`can_roam`); the moment a drag starts, `can_roam()` returns false
and the overlay stays on that monitor forever; `handle_drag` clamps the
selection to the window bounds (`capture_overlay.rs:634-636`), so a selection
can never span monitors. No change needed beyond the start-monitor priority.

**Whole-screen coverage** (menu bar, Dock, taskbar in the capture): already
achieved — macOS kiosk hides real chrome after the freeze while the nudge
loop covers the full monitor edge-to-edge; Windows overlay is topmost and
re-asserted above the taskbar (`windows.rs:330-338`). xcap captures chrome
pixels in the frozen frame (verified by design: frames are taken before the
kiosk engages).

**Capture modality**:
- macOS: full kiosk already in place — menu bar + Dock hidden,
  Cmd+Tab/Cmd+H disabled via `presentationOptions`, overlay at screensaver
  level, foreign popup menus dismissed with a synthetic Esc (grace-handled).
  Unblockable by design: Cmd+Opt+Esc (Force Quit), Mission Control gestures —
  document as limits.
- Windows **[decision: Topmost + re-focus, no `WH_KEYBOARD_LL`]**: the
  overlay is `HWND_TOPMOST` covering the full screen; if Alt+Tab (or any OS
  mechanism) steals focus, the viewer pass re-issues
  `ViewportCommand::Focus` for the overlay while `Capturing`. Limits to
  document honestly: the Alt+Tab switcher may briefly appear; focus re-grab
  is best-effort (`SetForegroundWindow` can be refused — then Esc goes to the
  other app until the user clicks the overlay; clicks anywhere still hit the
  overlay since it covers the screen); Win key, Ctrl+Alt+Del and the secure
  desktop can never be suppressed — acceptable, documented.
- Menus stay static during capture: `ModalState::Capturing` already greys
  every command through `is_command_enabled` — unchanged.

**Escape-only cancel**: unchanged (Esc → `CancelCapture`; window close → same).

### 2.6 Tray icon, Dock, taskbar [decision]

New resident-menu (identical on both OSes):
`Ouvrir PixMe` (`open_viewer`) / `Réglages` (`settings` → `OpenSettings`) /
`Vérifier les mises à jour` (disabled `MenuItem`, no command — same pattern as
the greyed `visit_site` item) / `Quitter` (`quit`).

- Left click on the status item (macOS) / tray icon (Windows) = **toggle
  viewer** (`TrayIconEvent::Click` with `with_menu_on_left_click(false)`;
  right click shows the menu — tray-icon 0.26 behaviour, verify at
  implementation). New `Command::ToggleViewer` = `OpenViewer` from `Idle`,
  `ViewerClosed` from `Viewer`.
- macOS Dock icon click → `applicationShouldHandleReopen`: bring the viewer
  to front **without moving it** (`Minimized(false)` + `Focus` + existing
  `request_focus`). winit does not surface a clean "dock reopen" signal —
  implementation options: hook the NSApplication delegate method, or observe
  `NSApplicationDidBecomeActiveNotification` while no window exists.
  **Open point #2** (implementation detail, not architecture).
- Windows taskbar: the viewer keeps its own button while open (native
  click-to-restore). The 1×1 anchor keeps `taskbar(false)` — a permanent
  ghost button in Alt+Tab/taskbar is worse than none; the tray covers the
  resident case.

### 2.7 White circle at Windows startup — diagnosis

**Culprit: the 1×1 anchor window.** The code itself documents it
(`windows.rs:392-405`): Windows 11 DWM draws a rounded border + top accent
line on borderless windows; on the degenerate 1×1 window this renders as a
small ring on the desktop. The existing fix (`CreateRectRgn(0,0,0,0)` on
`Role::Anchor` when ≤8 px) is insufficient:

1. **Startup race**: the root anchor is created *visible* (`app.rs:2779-2794`
   has no `with_visible(false)`); the first `pump → strip_undecorated_frame`
   only runs frames later — on slow/Intel laptops the ring shows meanwhile.
2. **Region loss**: `SetWindowRgn` is reapplied only on style/size change;
   a `SWP_FRAMECHANGED` from winit (DPI change, new child window, focus)
   resets the region without changing `(w,h)` → ring reappears and stays.
   Matches "some laptops" (docking/mixed-DPI churn).
3. **RDP**: no DWM composition in the remote session → no ring. Consistent
   with the observed absence.

Fix directions (implementation phase): create the anchor hidden
(`with_visible(false)` — verify the event pump still runs, expected yes) or
apply the empty region at creation via subclass, AND reapply it
unconditionally for `Role::Anchor` (cost-free on 1×1) instead of only on
style/size change. Removing the bar also shrinks `strip_undecorated_frame`'s
role set to Pin/Countdown/Overlay/Anchor.

---

## 3. Feature inventory → future home (exhaustive)

| Feature | Today | After refactor |
|---|---|---|
| 5 capture modes + delay + capture options | floating bar | vbar (viewer) |
| Show bar ("Take a capture", "capture.launch", "+") | tray/status, menus, tab "+", mosaic "+" | **removed** — vbar is always visible |
| Close bar → Viewer/Idle | bar X | n/a (window close on viewer) |
| Open/New/Import/LastDir/Recents/OpenAll/ClearHistory | Ouvrir▾ menu, File menu | unchanged |
| Save/SaveAll/Rename/DeleteFile/Print | toolbar, menus, tab menu | unchanged |
| Undo/Redo/Copy | Edit menu, shortcuts | unchanged |
| Draw editor, Resize, Zoom, Mosaic | toolbar | unchanged |
| Protection, duplicate, close-*, sort | tab context menu | unchanged |
| Mosaic OS drag-out | tile grip | unchanged |
| Pins, countdown, save-dialog, Draw, settings, About | viewports/modals | unchanged |
| Quit | PixMe menu (macOS), File menu + tray (Win) | unchanged + new tray menu keeps Quit |
| Monitor pick/refresh/rename/per-monitor geometry | — | **new** (status strip + prefs) |
| Toggle viewer on icon click | — | **new** (TrayIconEvent) |

No feature disappears without a destination.

## 4. Dead code to remove (Phase E)

- `src/ui/toolbar.rs` (whole file).
- `src/app.rs`: `AppState::Bar`, `Command::{ShowBar, CloseBar}`,
  `Shared::{toolbar, bar_pos, bar_rect, bar_monitor}`, `bar_id()`,
  `bar_builder()`, the `Bar` match arm, `ui_pos_on_anchor`, bar-related
  branches of `capture_start_monitor`, `menu.capture` egui menu
  (`viewer.rs:2496-2507`), macOS `Capture` submenu (`macos.rs:334-343`),
  `take_capture`/`capture.launch` in `command_from_menu_id` and tray/status
  menus, "+" tab button (`viewer.rs:2595-2600`), "+" mosaic tile
  (`viewer.rs:3184-3224`), obsolete tests (`closing_bar_*`,
  `bar_and_viewer_are_never_shown_together`, `ShowBar` rows in command table).
- `src/capture/region.rs` (stub returning `Unsupported` — never called).
- `src/export/file.rs::save_capture_with_dialog`/`save_with_dialog` and
  `src/export/clipboard.rs::copy` (unused stubs — verify no callers).
- `Capturer` trait in `src/capture/mod.rs` (never implemented).
- `global-hotkey` from `Cargo.toml` (declared, never used — run `cargo audit`
  after removal).
- `Role::Bar` handling in `strip_undecorated_frame` and `"PixMe Bar"`
  title matching; `MENU_RESERVE`/`SHADOW_MARGIN` bar constants.
- i18n keys made orphan: `menubar.take_capture`, `menu.capture`,
  `menu.capture.launch`, `toolbar.close.tooltip`, `viewer.new_capture.tooltip`
  (re-check before deleting; `toolbar.*` capture/delay/options keys are
  **reused** by the vbar).

## 5. Files to modify

- `src/app.rs` — machine, commands (`ToggleViewer`, `SelectMonitor`,
  `RefreshMonitors`, `RenameMonitor`), viewer `ViewportSync`, per-monitor
  geometry, capture start monitor = viewer monitor, deleted Bar plumbing,
  `harness()` test struct.
- `src/ui/viewer.rs` — vbar `SidePanel`, status-strip monitor row + Refresh +
  rename dialog, remove "+" elements, remove Capture menu (Windows strip).
- `src/ui/countdown.rs` — anchor on `viewer_monitor` only (bar anchor gone).
- `src/ui/capture_overlay.rs` — start-monitor priority (session monitor first).
- `src/ui/toolbar.rs` — **deleted**.
- `src/config/settings.rs` — `monitors` map (`MonitorPrefs`: name + rect_native).
- `src/platform/mod.rs` — `MonitorIdentity`, `monitor_identities()`, menu-id
  mapping cleanup, `ToggleViewer` plumbing.
- `src/platform/macos.rs` — status-item left/right click, new menu, stable
  display UUID, `setMovable(false)` on the viewer, dock-reopen hook.
- `src/platform/windows.rs` — tray left-click toggle, new menu, DeviceID
  monitor identity, viewer subclass (move lock), anchor white-ring hardening,
  overlay re-focus while `Capturing`.
- `src/platform/drag_out.rs` — unchanged.
- `locales/{en,fr,es}.yml` — see §8.
- `Cargo.toml` — drop `global-hotkey` → `cargo audit` before building.
- `docs/decisions.md`, `docs/theme.md`, `.devin/rules` UI section, `AGENTS.md`
  if platform-isolation text needs updating.
- `src/ui/viewer.rs` tests, `src/app.rs` tests — update for new commands/states.

## 6. macOS / Windows incompatibilities

| Topic | macOS | Windows |
|---|---|---|
| Viewer move lock | `setMovable(false)` — clean, native | subclass + `WM_NCLBUTTONDOWN`/`HTCAPTION` (or `WM_WINDOWPOSCHANGING` clamp) — native subclassing needed |
| Capture modality | presentationOptions kiosk — complete | topmost + re-focus — partial by OS design (no way to block Alt+Tab/Win key without hooks, which were excluded) |
| Monitor stable id | `CGDisplayCreateUUIDFromDisplayID` | `EnumDisplayDevices` DeviceID (port-dependent) |
| Resident click | status item left/right semantics via tray-icon | same crate API — verify `TrayIconEvent` on both |
| App-icon click | Dock `applicationShouldHandleReopen` — needs delegate hook | taskbar button exists only while viewer is open (native behaviour) |
| Full-chrome capture | kiosk + nudge loop (in place) | topmost overlay + z-order re-assert (in place) |

## 7. User-facing impact

- One fewer window: no floating palette — the viewer is the single home;
  capture controls are always one fixed strip away, in both view modes.
- The viewer is **immovable** (both OSes, parity semantics): repositioning
  happens only via monitor buttons; resize stays free.
- Tray/status item becomes a pure open/settings/quit surface; captures never
  start behind the user's back from the resident icon.
- A disconnected monitor remains visible (marked) until clicked once —
  its name and geometry persist silently afterwards.
- Capture: overlay opens on the viewer's monitor; cursor still roams freely;
  selection can never span monitors (unchanged structural behaviour).

## 8. Translations

**Reusable as-is** (vbar keeps `toolbar.*` strings): `toolbar.window.tooltip`,
`toolbar.region.tooltip`, `toolbar.fullscreen.tooltip`,
`toolbar.fixed_region.tooltip`, `toolbar.pin.tooltip`, `toolbar.delay.tooltip`,
`toolbar.options.tooltip`, `delay.none`, `delay.second`, `delay.seconds`,
`options.include_pointer`, `options.copy_to_clipboard`, `menubar.open_viewer`,
`menu.app.preferences`, `menubar.quit`, `draw.ok`, `draw.cancel`.

**New keys** (draft EN/FR/ES — to review):

| Key | EN | FR | ES |
|---|---|---|---|
| `menubar.check_updates` | Check for updates | Vérifier les mises à jour | Buscar actualizaciones |
| `viewer.monitor.refresh` | Refresh monitors | Actualiser les moniteurs | Actualizar monitores |
| `viewer.monitor.disconnected` | Disconnected | Déconnecté | Desconectado |
| `viewer.monitor.rename` | Rename monitor | Renommer le moniteur | Renombrar monitor |
| `viewer.monitor.rename_title` | Monitor name | Nom du moniteur | Nombre del monitor |
| `viewer.monitor.current` | Current monitor | Moniteur actuel | Monitor actual |

**Removed keys**: `menubar.take_capture`, `menu.capture`,
`menu.capture.launch`, `toolbar.close.tooltip`, `viewer.new_capture.tooltip`.

## 9. Regression risks

- macOS: `TrayIconEvent` + right-click menu semantics on tray-icon 0.26
  (verify); dock-reopen hook could fight winit's delegate (fallback:
  DidBecomeActive observer); `setMovable(false)` also blocks intra-monitor
  moves — accepted per decision; do not disturb the kiosk's
  synthetic-Esc/nudge machinery while touching `macos.rs`.
- Windows: HWND subclassing for the move lock is new native code (same risk
  class as `strip_undecorated_frame` — keep it in `platform/windows.rs`);
  re-focus loop must not fight the user (bounded retries like
  `maybe_focus`); anchor hardening must not re-break `ShowWindow` reveal
  ordering.
- Cross: `is_command_enabled` is exhaustive — every new `Command` needs a
  ruling (compile-time safety); `Shared` harness in tests mirrors every
  field (mechanical update); `pump()` runs from every viewport — monitor
  enumeration cost must be bounded (cache + explicit Refresh, not per-frame);
  a monitor disappearing between sessions must fall back to primary + clamp.

## 10. Tests

- Unit: `is_command_enabled` table for new commands (exhaustiveness test
  pattern already enforced); monitor identity matching + disconnected
  reconciliation (pure logic); `monitors` map serde round-trip in settings;
  native↔ui geometry for programmatic moves (extend `platform/mod.rs` tests);
  i18n completeness (existing gate).
- Integration (harness `drive()`): `ToggleViewer` both directions, capture
  launched from selected monitor, `ViewerClosed` → `Idle` keeps session.
- Manual (must be validated on hardware): white ring at boot (affected
  laptop + RDP cross-check), viewer lock both OSes, Alt+Tab during capture
  on Windows (document residual behaviour), tray left/right click, dock click
  with viewer closed, programmatic move 4K↔laptop with mixed DPI, capture
  including menu bar/Dock/taskbar pixels.

## 11. Implementation slicing (independent, validatable steps)

1. **Dead-code cleanup**: delete bar state/commands/files, `global-hotkey`,
   stubs; app compiles, quality gate green. *(small)*
2. **vbar**: `SidePanel` + 5 modes + delay + options; remove "+" elements.
   *(medium)*
3. **Monitor inventory + persistence**: `MonitorIdentity` per OS, `monitors`
   map in prefs, status-strip buttons + Refresh + rename + disconnected UI.
   *(medium-large)*
4. **Viewer lock + per-monitor geometry**: viewer `ViewportSync`, native-rect
   save/restore per `stable_id`, `setMovable(false)` macOS + Windows subclass.
   *(large)*
5. **Capture start monitor + modality**: session-monitor priority in
   `warm_freeze`, Windows re-focus loop while `Capturing`, kiosk re-check.
   *(medium)*
6. **Tray/Dock**: `ToggleViewer`, new menus, dock reopen. *(medium)*
7. **White ring**: anchor hidden-at-creation or unconditional empty region.
   *(small — may be pulled forward)*
8. **i18n + docs + full quality gate**: `fmt`, `clippy -D warnings`,
   `build --release`, `test`, `cargo audit`; update `docs/decisions.md`,
   `docs/theme.md`, rules' UI section.

## 12. Open points to decide

1. **Windows viewer lock technique**: `WM_NCLBUTTONDOWN/HTCAPTION` swallow
   (full immovable parity with macOS — recommended) vs `WM_WINDOWPOSCHANGING`
   monitor clamp (movable within the monitor) vs none.
2. **Dock reopen**: delegate hook vs `NSApplicationDidBecomeActive`
   observer — implementation detail, decide at code time.
3. **`PIXME_CAPTURE`/`PIXME_CAPTURE_AFTER` dev hooks**: keep for debugging
   (they bypass the exclusive-trigger rule but are not user-facing) — confirm.
4. **Toolbar "Close" button** in the viewer toolbar strip (`vbar.close`):
   duplicates the window's own close — keep or remove.
5. **Status-strip layout**: monitor buttons right-aligned before the version
   label, or a dedicated left group — cosmetic, decide at implementation.

---

*Audit performed on `feature/ui-overhaul` at commit `8f3d728`. No code was
modified, no commits made.*
