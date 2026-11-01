# AGENTS.md

## Platform isolation rule (IMPORTANT)

The macOS version must not be broken by Windows work. All `#[cfg(target_os)]`
code lives in `src/platform/` (`macos.rs`, `windows.rs`, `mod.rs`, `drag_out.rs`).

- Safe to edit without warning: `src/platform/windows.rs` and
  `#[cfg(target_os = "windows")]`-gated blocks only.
- Warn the user first before touching shared code: `src/platform/mod.rs`,
  non-cfg code, `build.rs` (has per-OS logic), `Cargo.toml`, assets, locales.
