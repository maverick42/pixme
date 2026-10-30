# PixMe audit process

Manual verification scenarios ("MT") run before significant releases,
alongside `cargo test` (unit + i18n), `cargo clippy -D warnings`,
`cargo fmt --check`, `cargo build --release` and `cargo audit`
(mandatory after any dependency change).

Environment notes: capture tests on macOS need a signed `.app` bundle
(ad-hoc is fine) and the Screen Recording permission; DPI behavior is
checked per monitor. Equivalent outcomes are expected on macOS and
Windows.

## 1. Capture

- MT-01 — Region capture: the frozen frame shows the dim veil, the
  marquee has 8 handles, Enter/double-click confirms, Esc cancels; the
  captured region opens as a new viewer tab.
- MT-02 — Full screen, active window, window-or-object, fixed region
  (resizable frame remembered), pin to screen (autonomous floating
  window: move, copy, save, close).

## 2. Annotation editor

- MT-03 — Draw tools round-trip: objects stay selectable/undoable in
  chronological order; OK commits, Annuler discards; pixel ops (blur,
  marquee cut/fill) render identically in canvas and export.
- MT-04 — Marquee (SelectArea): frame, move, resize, cut/copy/paste/
  delete/blur/fill; hole fills with the memorised seed colour; paste
  honours the OS clipboard and stays undoable.

## 3. Session management

- MT-05 — Quit then relaunch: every capture reopens (title, order,
  active tab, saved flag, annotations, annotation undo/redo). After
  Save-All + Quit the restored session is clean; an unsaved session
  comes back dirty and the Save/Discard/Cancel flow still applies.
- MT-06 — Closing a tab deletes its stored row: close one tab, quit,
  relaunch — the closed capture does not come back; the others do.
- MT-07 — Discard from the dirty confirmation: quit, confirm Discard —
  the store is emptied; relaunching shows no resurrected captures.

## 4. Tabs and protection

- MT-08 — Red dot on unsaved tab, unique titles (space + `(n)` suffix), close
  without warning, context-menu actions (save, rename, delete file,
  close this/others/saved/left/right/all, duplicate).
- MT-09 — Protected tab: password set/change/remove; lock re-arms on
  every tab switch; the unlock card replaces the canvas. The lock
  gates the DISPLAY only — copy/save/print/draw still work (documented
  screen lock, not encryption). After relaunch the tab is locked again.
- MT-10 — Delete file on disk behind confirmation; the tab stays open,
  unsaved again.

## 5. Viewer

- MT-11 — Zoom modes (fit/width/actual), copy to clipboard, resize
  dialog, drag-out of a tab's PNG into another app.
- MT-12 — "+" tab returns to the capture bar; viewer and bar are never
  visible together.

## 6. Settings and i18n

- MT-13 — Language switch at runtime (EN/FR/ES), persisted; `auto`
  follows the OS locale. Appearance auto/dark/light, persisted.
- MT-14 — `fr.yml`/`es.yml` cover every `en.yml` key (test-gated).

## 7. Lifecycle

- MT-15 — Launch is resident-only (no window): macOS shows the status
  item + Dock icon, Windows the tray + taskbar button; menus carry
  "Take a capture" and "Open PixMe".
- MT-16 — Closing the bar opens the viewer when the session has
  captures, else Idle; closing the viewer returns to Idle with the
  session kept in memory.
- MT-17 — Session restore after Quit, then Discard: quit with tabs,
  relaunch (tabs restored — MT-05), mark dirty, Quit → Save/Discard/
  Cancel → Discard — store emptied; next launch shows no captures.
- MT-18 — Second instance: launching PixMe while it runs shows the
  translated "already running" dialog and exits; the first instance is
  unaffected.

## 8. Store integrity (session.db)

- MT-19 — Corrupt `session.db` is parked as `session.db.bak` and a
  fresh store takes over; the app keeps running. `.bak` older than
  7 days is auto-deleted.
- MT-20 — No session caps: keep many tabs, quit, relaunch — every
  capture comes back; nothing is dropped for size reasons.
- MT-21 — Retention setting: `never` stops persistence (no session.db
  write at quit); `today` drops pre-today captures at launch; switching
  to a more restrictive option with captures stored asks "captures
  will be lost" — Confirm applies + prunes, Cancel reverts.
- MT-22 — Tab sort: context menu second zone toggles by-name/by-date
  (re-click returns to insertion order); ordering persists across
  restarts and re-sorts on capture, rename and duplicate.

## 9. Mosaic view mode

- MT-23 — Toggle: the toolbar's grid button switches to the thumbnail
  grid and hides the tab strip; the viewer always starts in Tabs mode;
  the trailing "+ New capture" tile returns to the capture bar.
- MT-24 — Mosaic zoom: ± buttons step ×1.25 (clamped 50–200 %), the
  percent button resets to 100 %, its caret menu lists 50/75/100/125/
  150/200 %; columns reflow with the window width; thumbnails stay
  sharp and keep their aspect ratio.
- MT-25 — Tiles: single click selects (toolbar buttons and ⌘C/⌘Z/⇧⌘Z
  act on the selection), double-click or Enter opens the tile in Tabs
  mode, right-click opens the tab menu (sort by name/date reorders the
  grid); the close cross closes only that capture; hovering shows the
  capture date in the UI language's local format.
- MT-26 — Protected capture in Mosaic: the tile shows a centred
  padlock, never a thumbnail; double-click returns to Tabs mode with
  the password card; switching selection re-locks and drops the
  thumbnail.
- MT-27 — Thumbnails: annotations appear in the tiles (same render
  path as the canvas); thumbnails rebuild after edits; with many tabs
  only visible tiles are textured (memory stays bounded).

## 10. Modal gating

- MT-28 — With Settings open, the menu bar greys out « Tout
  enregistrer », « Imprimer », all captures and the status-item
  entries; their shortcuts are inert. ⌘Q still works, including from
  the icon menu. Same check with the Draw editor and the save-as
  dialog; under Draw, undo/redo/copy act on the editor only. On
  Windows the same holds for global shortcuts and the tray menu, and
  the egui menu strip / bar buttons show disabled.

## 11. Data locations and logging (2026-10-30)

Security/privacy step — check on every release:

- No secrets in logs: scan `logs/pixme.*.log` — no clipboard text, no
  annotation text, no window titles above `debug`, no full user-profile
  paths outside the one-time `startup` banner and the fallback notice.
- Levels: `ERROR` = user-visible failure, `WARN` = handled degradation,
  `INFO` = lifecycle/actions, `DEBUG` = timings only. No per-frame lines.
- `Sensitive<T>` stays `<redacted>` in `Debug`/`Display` (test-gated).
- Diagnostics copy redacts the home dir (`~`) — verify on both OSes.
- Locations: nothing writes outside `paths::data_dir()`/
  `paths::logs_dir()` (source scan `tests/data_paths.rs` gates this).

Manual scenarios:

- MT-29 — Windows read-only install: run `pixme.exe` from a folder the
  user cannot write (e.g. `C:\Program Files\pixme` or a `chmod 555`
  dir) — data+logs land in `%LOCALAPPDATA%\PixMe\{data,logs}`, one
  translated notice shows where, log carries the `attempted`/`used`
  warning.
- MT-30 — Path edge cases: exe in `C:\Mon Dossier été\pixme\`, launch
  via a shortcut whose working dir differs — data+logs still resolve
  beside the exe; `\\?\`-prefixed and `\\server\share` installs behave
  identically.
- MT-31 — Log retention: plant `pixme.<6-days-ago>.log` and a foreign
  `notes.txt` in `logs\`, relaunch — the dated file is gone, the
  foreign file stays; >5 daily files → `max_log_files` keeps 5.
- MT-32 — Diagnostic mode: Settings → General → Diagnostic mode
  switches the running session to `debug` (uncheck → back to startup
  level), persists nothing; `RUST_LOG` still wins at launch.
- MT-33 — Copy diagnostics: Settings → About → button copies banner +
  log tail with the home dir replaced by `~`; the two folder buttons
  open Finder/Explorer.
- MT-34 — Per-install instances (Windows): two PixMe folders run side
  by side; a second launch from the SAME folder shows the translated
  warning; kill -9 / End Task then relaunch — no orphan lock blocks it.
- MT-35 — Settings migration: a `prefs.json` beside an old exe is
  copied (not moved) into `data\` on first launch when none exists —
  settings survive, the old file stays.
