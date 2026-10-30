# Logging catalogue

PixMe writes one line per event to `logs_dir()/pixme.<YYYY-MM-DD>.log`
(**UTC** date in the file name — `tracing-appender` rotates on
`OffsetDateTime::now_utc`; line timestamps are local with explicit
offset). Five files are kept (`max_log_files`) and anything older than
five days is pruned at startup. `pixme.panic.log` holds panics
(message, location, backtrace) and shares the 5-day purge.

Format: `2026-10-30T14:23:45.123+01:00 LEVEL target{span{fields}}: message fields`.

## Levels

- `ERROR` — a failure the user can see.
- `WARN` — a degraded but handled case (fallback, dropped lines, unreadable file).
- `INFO` — lifecycle and user actions (default level in release).
- `DEBUG` — timings and queues (diagnostic mode / `RUST_LOG`).
- `TRACE` — never enabled by default.

An error is logged ONCE, at the boundary, with its cause chain (`{err:#}`).
No per-frame logging.

## Events

| Event | Level | Fields | Meaning |
|---|---|---|---|
| `startup` | INFO | version, commit, os, arch, screens, language, appearance, log_level, data_dir, logs_dir, fallback, screen_recording | One banner per run — the only place full dirs are logged. |
| `monitor` | INFO | id, w, h, scale, primary | One per connected screen, after `startup`. |
| `lifecycle` | INFO | state | `Idle`/`Bar`/`Capturing`/`Viewer` transitions. |
| `capture_start` | INFO | mode | Inside `capture{capture_id}` span. |
| `capture_finished` | INFO | mode, w, h, ms | Inside `capture{capture_id}` span. |
| `capture_empty` | INFO | mode | Confirmed but nothing to keep — same span. |
| `capture_cancelled` | INFO | mode | Esc / close — same span. |
| `capture_failed` | ERROR | mode, error | Freeze/permission failure — same span. |
| `capture stored` | INFO | title | Document pushed into a viewer tab. |
| `session saved` | INFO | tabs | Quit-time persist (or save at settings apply). |
| `session restored` | INFO | tabs, ms | Session decode on first viewer open. |
| `session save failed` / `session restore failed` | WARN | error | Persistence problem — never fatal. |
| `session db rebuilt` | WARN | error, parked file | Corrupt db parked as `.bak`. |
| `tab saved` / `saved` | INFO | file | Export to disk (file name only). |
| `save-all failed` | WARN | file, error | One failed save in a batch. |
| `clipboard copy failed` / `auto clipboard copy failed` | WARN | error | Clipboard write problems. |
| `clipboard import failed` / `clipboard holds no image` | WARN / INFO | error | Paste pipeline. |
| `command` | INFO | command | EVERY dispatched user action (`Capture(Region)`, `Quit`, `OpenPath(file.png)` — paths reduced to file names). |
| `document_opened` | INFO | file | An image loaded into a viewer tab. |
| `clipboard_copy` | INFO | — | Image pushed to the clipboard (viewer or pin). |
| `clipboard_imported` | INFO | — | Clipboard image imported as a new tab. |
| `printed` | INFO | — | OS print job submitted. |
| `drag_out` | INFO | — | OS-level drag of a capture into another app started. |
| `tabs_closed` | INFO | count | Viewer tabs closed (stored rows deleted). |
| `view_mode` | INFO | mode | Tabs ↔ mosaic switch (all paths funnel through one setter). |
| `mosaic_zoom` | INFO | scale | Mosaic tile scale change (50–200 %). |
| `tab_sort` | INFO | sort | Tab ordering change (by-name / by-date / off). |
| `tab_renamed` | INFO | — | Rename dialog applied (title itself is user text — not logged). |
| `tab_duplicated` | INFO | — | Tab duplicated via the tab menu. |
| `tab_unlocked` | INFO | — | Password accepted on a protected tab. |
| `protection_changed` | INFO | — | Password set, changed or removed. |
| `file_deleted` | INFO | file | Tab's file deleted on disk. |
| `zoom` | INFO | zoom | Viewer zoom mode change (fit / width / factor). |
| `pin_created` | INFO | id | Pinned capture window opened. |
| `draw_closed` | INFO | — | Draw editor closed (open is `command=OpenDraw`). |
| `resized` | INFO | from, to, antialias | Active document resampled (`WxH` → `WxH`). |
| `draw_committed` | INFO | — | Draw editor annotations committed. |
| `session_discarded` | INFO | — | Session deliberately discarded. |
| `save-all blocked: duplicate tab titles` | WARN | — | Batch save refused (name clash). |
| `tab_unlock_failed` | WARN | — | Wrong password on a protected tab. |
| `color_copied` | INFO | — | Overlay magnifier hex colour copied (`C`). |
| `folder_opened` | INFO | folder | Settings "open folder" button. |
| `diagnostics_copied` | INFO | — | "Copy diagnostic information" clicked. |
| `settings_changed` | INFO | setting, value | One event per modified preference. |
| `migrated prefs.json from previous location` / `migrated settings.toml` | INFO | file | One-shot settings copy (source kept). |
| `single-instance guard unavailable` | WARN | error | The guard failed — the app still runs. |
| `previous data directory found — left in place, not migrated` | WARN | previous | Old per-user dir detected, untouched. |
| `exe-adjacent directory not writable — data/logs redirected` | WARN | attempted, used | Windows portable fallback. |
| `stale log files removed` | INFO | pruned | Startup purge count. |
| `daily log soft cap reached — debug/info silenced` | WARN | bytes | >64 MiB/day: only warn+error reach the file. |
| `daily log hard cap reached — file logging off until next rotation` | ERROR | — | >256 MiB/day: file output stops. |
| `log lines dropped — writer queue saturated` | WARN | dropped | Non-blocking queue full, counted once. |
| `diagnostic mode` | INFO | enabled | Settings toggle (session-only `debug`). |
| `status item installed` / `tray icon` / `menu item` | INFO / WARN | error | OS chrome lifecycle. |
| `shutdown` | INFO | — | Clean exit; the writer guard flushes after it. |
| `panic on '<thread>': …` | ERROR | panic info | Mirrored into `pixme.panic.log` + stderr first. |

Global-hotkey registration events are reserved for when the feature
lands: `hotkey_registered` (INFO) and `hotkey_conflict` (WARN, app
keeps running without shortcuts).

## Privacy rules

- Never: pixels, clipboard contents, annotation text.
- Window titles / app names: `debug` only.
- Paths: file names (`file_name`) or last components (`tail_path`) —
  the full data/log dirs appear ONCE, in `startup`.
- `Sensitive<T>` prints `<redacted>` for `Debug` and `Display` — wrap
  anything potentially sensitive.
- "Copy diagnostic information" re-redacts the home dir to `~` and adds
  a translated review-before-sharing reminder in the UI.

## Reading the logs

One capture = one `capture_id`: `grep 'capture_id=7' pixme.*.log` shows
its whole life (start → freeze → confirm → stored). The span is created
in `begin_capture` and re-entered in `confirm`/`cancel`/overlay errors.

Load: the writer is non-blocking (8192-line buffer) — a saturated queue
drops lines and reports the count once (`dropped` field), never stalls
the UI. `WorkerGuard` flushes on every exit path via `logging::shutdown()`.
