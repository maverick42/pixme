# AGENTS.md

## Platform isolation rule (IMPORTANT)

The macOS version must not be broken by Windows work. All `#[cfg(target_os)]`
code lives in `src/platform/` (`macos.rs`, `windows.rs`, `mod.rs`, `drag_out.rs`).

- Safe to edit without warning: `src/platform/windows.rs` and
  `#[cfg(target_os = "windows")]`-gated blocks only.
- Warn the user first before touching shared code: `src/platform/mod.rs`,
  non-cfg code, `build.rs` (has per-OS logic), `Cargo.toml`, assets, locales.

## Packaging

`scripts/package-macos.sh` builds the release binary and packages it as
an ad-hoc-signed `PixMe.app` inside
`target/release/pixme-<version>-macos-<arch>.dmg` (version from
`Cargo.toml`, arch from `uname -m` — the GitHub-release naming). Run it
after any release build the user wants to install — never assemble the
DMG by hand.
