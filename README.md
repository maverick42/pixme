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
