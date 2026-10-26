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
