# Changelog

All notable changes to PixMe are documented in this file. It is updated
as part of every version bump, in English, French and Spanish
(`CHANGELOG.md`, `CHANGELOG-fr.md`, `CHANGELOG-es.md`).

## [0.3.0] - 2026-11-13

### Added

- Workspaces: captures are bucketed into workspaces — each owns a name
  (capped at 30 characters), a naming mask + counter and an output
  folder. A compact chip at the leading edge of the status strip shows
  and switches the active workspace; captures move between workspaces
  through the tab context menu's "Workspace" submenu. Each workspace
  keeps its own last-selected capture, and Save All / quit-time saving
  act on the visible set only.
- Workspace switcher: every entry shows its live capture count as a
  pill badge — accent-tinted on the active workspace, hollowed out
  when the workspace holds no capture.
- Duplicate annotation: `Ctrl+D` / `Cmd+D`, a strip button right of
  Print (greyed out without a selection) and a context-menu entry clone
  the selected annotation with a small offset; cloned step badges take
  the next counter of their own mode, and a single undo entry removes
  the copy while restoring the previous selection.
- Step numbering: the number and letter sequences are now fully
  autonomous — duplicating or switching a selected step's mode
  re-numbers it into the target sequence, and the letter counter
  accepts typed letters.

### Fixed

- The quit-time unsaved-changes confirmation no longer stalls on
  same-workspace name clashes and saves captures across every
  workspace.
- The unsaved-session confirmation now appears on macOS Dock quit.

## [0.2.1] - 2026-11-09

### Added

- Configurable global shortcuts, plus macOS framebuffer captures.
- macOS packaging script producing the `PixMe.app` DMG.
- Windows: relaunching the app surfaces the viewer instead of a second
  instance.
- Draw editor: step counters are seeded from the document when the
  editor opens; each tool's option bank can be reset to factory
  defaults from its chip's context menu.
- Viewer: minimum window size raised to 580×420; clicking the shown
  monitor's taskbar icon re-centres the viewer.

### Fixed

- Disconnected monitors keep their place in the status-strip order
  instead of being dropped.
- The session store keeps its connection alive when a startup VACUUM is
  skipped.
- Capture freeze waits for the compositor to settle, and region
  selections auto-confirm on release for Create/Place drags.

## [0.2.0] - 2026-11-06

Initial release — screenshot capture and annotation.

### Added

- Capture: full screen, region selection on a frozen frame with
  magnifier card and resizable handles, window/object pick,
  fixed-size region, pin-to-screen floating windows, capture delay
  countdown, clipboard import.
- Annotation editor: arrows with bezier curves, text and text boxes,
  shapes with fills, outlines and shadows, numbered/lettered step
  markers, area highlighter, blur and pixelate eraser, freehand
  pencil, marquee area selection with pixel operations, emoji stickers
  and a colour picker — with per-tool styling persisted across
  sessions and full undo/redo.
- Viewer: tabbed captures plus a mosaic grid, the vertical capture
  bar, a status strip with per-monitor buttons, session persistence
  across restarts, save/export (PNG, JPEG, WebP), printing, clipboard
  copy and OS drag-out.
- Resident app: macOS status item with native menus, Windows tray
  icon, single-instance guard.
- Settings: English, French and Spanish UI, Pro and Pro Light themes,
  monitor management, icon styles and per-OS shortcuts.
