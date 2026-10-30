# pixme

A fast, lightweight screen capture and annotation tool for macOS and Windows.

Interface inspired by [PhotoCraft](https://github.com/storytold/photocraft), the open-source Rust image editor. See `ATTRIBUTION.md` for bundled assets and adapted code, `docs/decisions.md` for the decision log and `docs/theme.md` for the visual spec.

## Development

Requires stable Rust (the committed `rust-toolchain.toml` selects the channel and components; rustup installs them automatically on first build).

```sh
cargo run            # run the app
cargo fmt --check    # formatting gate
cargo clippy --all-targets -- -D warnings
cargo build --release
cargo test           # includes the locale completeness test
cargo audit          # dependency vulnerability check (required on dep changes / release)
```

Notes:

- `cargo clippy` and `cargo test` compile separately, so the first run of each recompiles dependencies — subsequent runs are incremental.
- On Windows, excluding `target/` and `~/.cargo` from antivirus scanning measurably speeds up builds; Windows Smart App Control may block an unsigned locally-built binary (choose *Run anyway* or sign it).
- Optional speed-ups (not required, not enforced): `cargo install cargo-nextest` then `cargo nextest run`; `sccache` for shared compile caching; a faster linker (`lld`/`mold` via a local, uncommitted `.cargo/config.toml`).

### Manual tests

Tests that need a real display, OS capture permission or a live desktop are marked `#[ignore]` so CI never requires them. Run them explicitly on a real machine:

```sh
cargo test -- --ignored
```

Currently none exist; capture/permission tests land with the capture feature.

### Performance test data and benchmarks

All measurement tooling is behind the `dev-tools` Cargo feature — **off
by default, never compiled into a distributed binary**. Enable it for
profiling builds only: `cargo build --release --features dev-tools`.

It provides:

- `pixme --seed-session [N]` — writes N synthetic 5120×2880 captures
  (half annotated) into `session.db` and exits, for realistic thumbnail
  and session-restore benchmarks.
- `PIXME_DATA_DIR` — overrides the per-user data directory **in
  dev-tools builds only**. Point `--seed-session` and benchmark runs at
  a scratch directory and your real session is untouchable:
  `PIXME_DATA_DIR=/tmp/pixme-bench ./target/release/pixme --seed-session`.
- `PIXME_DEBUG_MOSAIC=1` — opens the restored session straight into
  the mosaic grid (first-paint cost measurable without driving the UI).
- `PIXME_DEBUG_EDIT=1` — replays annotation-commit → resize → undo →
  redo on the first annotated tab, one op per frame, timing each
  (`debug edit op` log lines) for edit-path jank measurement.
- `RUST_LOG=pixme=debug` — per-stage timings: `session capture decoded`,
  `mosaic thumbnail built`, `pixel undo`/`pixel redo`/`annotation commit`.

Windows procedure (PowerShell):

```powershell
cargo build --release --features dev-tools
$env:PIXME_DATA_DIR="$env:TEMP\pixme-bench"
.\target\release\pixme.exe --seed-session
$env:PIXME_DEBUG_MOSAIC="1"; $env:RUST_LOG="pixme=debug"
.\target\release\pixme.exe
```

### Data and log locations

All app-data paths are resolved once at startup by `src/paths.rs`.

- **Windows (portable)**: everything lives beside `pixme.exe` —
  `<exe dir>\data\` (`session.db`, `.bak` backups, `prefs.json`,
  `instance.lock`) and `<exe dir>\logs\` (`pixme.<date>.log`,
  `pixme.panic.log`). If the exe dir is not writable the app falls back
  to `%LOCALAPPDATA%\PixMe\{data,logs}` and tells you once where it went.
- **macOS**: `~/Library/Application Support/pixme/` and
  `~/Library/Logs/PixMe/`.
- `PIXME_DATA_DIR` / `PIXME_LOGS_DIR` overrides exist **in dev-tools
  builds only** — a distributed binary ignores them, so a test can
  never touch a real session.

Windows reference install and updates:

```text
C:\portable\pixme\
    pixme.exe      ← replace ONLY this file on update
    data\          ← session.db, prefs.json, instance.lock (untouched)
    logs\          ← pixme.<date>.log, pixme.panic.log (untouched)
```

An update = swap the exe; `data\` and `logs\` always survive. Launching
the exe from a subfolder (e.g. `C:\portable\pixme\old\pixme.exe`) makes
THAT folder's `data\`/`logs\` the live ones — locations follow the exe,
never the working directory. Dev builds (`cargo run`,
`target\release\pixme.exe`) therefore create their data inside
`target\` — gitignored and wiped by `cargo clean`, never a real
session.

### Diagnosing a problem

- Logs: `logs_dir()/pixme.<UTC date>.log`, 5 files kept — see
  `docs/logging.md` for the event catalogue and privacy rules.
- **Diagnostic mode** (Settings → General): `debug` level for the
  running session only, never persisted. `RUST_LOG` still overrides the
  startup level.
- **Copy diagnostic information** (Settings → About): banner + last 200
  log lines to the clipboard, home-dir redacted — review before sharing.
