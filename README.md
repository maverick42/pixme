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

### Performance test data

`pixme --seed-session [N]` (default 20) writes N synthetic 5120×2880
captures — half carrying annotations — into the real `session.db` and
exits, so thumbnail and session-restore costs are measurable on a
realistic session. Seeding **replaces** the current session; the
existing database is copied aside to `session.db.preseed` first.

In dev builds, `PIXME_DEBUG_MOSAIC=1` opens the restored session
straight into the mosaic grid (release builds always open in Tabs —
the hook is compiled out). Combined with `RUST_LOG=pixme=debug`, the
per-stage timings (`session capture decoded`, `mosaic thumbnail
built`, `mosaic frame`) give the full first-paint breakdown.

To reproduce the mosaic benchmark on Windows (PowerShell):

```powershell
cargo build                                   # dev build keeps the hook
.\target\debug\pixme.exe --seed-session    # 20 synthetic 5K captures
$env:PIXME_DEBUG_MOSAIC="1"; $env:RUST_LOG="pixme=debug"
.\target\debug\pixme.exe                   # log shows the timings
```

Dev-profile dependencies already build at `opt-level = 2`, so the
numbers are representative (though not identical to release).
