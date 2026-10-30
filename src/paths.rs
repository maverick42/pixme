//! Single source of truth for every application data location.
//!
//! [`paths()`] resolves all directories once at startup (a `OnceLock`
//! around the pure [`resolve`]/[`pick_dirs`] functions); no other module
//! builds a data path itself — `tests/data_paths.rs` fails the build if
//! `ProjectDirs`, `current_exe`, `LOCALAPPDATA` or friends appear outside
//! `src/paths.rs` and `src/platform/`.
//!
//! Layouts (chosen per OS in `platform::resolve_base_dirs`):
//! - Windows (portable): `<exe dir>\data\` and `<exe dir>\logs\`, with a
//!   writability probe at startup; a read-only exe dir falls back to
//!   `%LOCALAPPDATA%\PixMe\{data,logs}` and [`fallback_from`] reports it.
//! - macOS: `~/Library/Application Support/pixme/` and
//!   `~/Library/Logs/PixMe/` — an `.app` bundle is never written into.
//!
//! `PIXME_DATA_DIR` / `PIXME_LOGS_DIR` override the result ONLY in
//! `dev-tools` builds, so tests and `--seed-session` can never touch a
//! real session. There is no automatic migration: a previous per-user
//! data dir is reported through [`legacy_data_dir`] (logged, left
//! intact).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::platform;

/// The two OS-chosen base directories, plus why the preferred location
/// was rejected when it was.
pub struct BaseDirs {
    /// Holds `session.db` (+ backups), `prefs.json`, `instance.lock`.
    pub data: PathBuf,
    /// Holds `pixme.<date>.log` and `pixme.panic.log`.
    pub logs: PathBuf,
    /// The preferred location that failed the writability probe
    /// (Windows: the exe-adjacent dir). `Some` = the user must be told
    /// where their data actually went.
    pub fallback_from: Option<PathBuf>,
}

/// The resolved locations for this process.
struct Paths {
    base: BaseDirs,
    /// The pre-relocation per-user data dir when it still exists on
    /// disk — a warning is logged, nothing is migrated.
    legacy_data: Option<PathBuf>,
}

static PATHS: OnceLock<Paths> = OnceLock::new();

/// The resolved locations; computed on first call, frozen afterwards.
fn paths() -> &'static Paths {
    PATHS.get_or_init(resolve)
}

/// Explicit startup call — resolves (and creates) every directory before
/// the logging subscriber needs `logs_dir`.
pub fn init() {
    let _ = paths();
}

/// Pure resolution: dev-tools overrides first, then the OS choice.
/// Parent dirs are created best-effort — later writers re-check anyway.
fn resolve() -> Paths {
    #[cfg(feature = "dev-tools")]
    if let Some(dir) = std::env::var_os("PIXME_DATA_DIR") {
        let data = PathBuf::from(dir);
        let logs =
            std::env::var_os("PIXME_LOGS_DIR").map_or_else(|| data.join("logs"), PathBuf::from);
        let _ = std::fs::create_dir_all(&data);
        let _ = std::fs::create_dir_all(&logs);
        return Paths {
            base: BaseDirs {
                data,
                logs,
                fallback_from: None,
            },
            legacy_data: legacy_data_dir(),
        };
    }
    let base = platform::resolve_base_dirs();
    let _ = std::fs::create_dir_all(&base.data);
    let _ = std::fs::create_dir_all(&base.logs);
    // The pre-relocation dir is only worth a warning when it differs
    // from the new one (macOS keeps the same Application Support dir).
    let legacy = legacy_data_dir().filter(|p| *p != base.data && p.exists());
    Paths {
        base,
        legacy_data: legacy,
    }
}

/// The pre-relocation per-user data dir (`directories` AppData /
/// Application Support). `None` when the OS reports none.
fn legacy_data_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "pixme").map(|d| d.data_dir().to_path_buf())
}

/// `data/` — session database, backups, instance lock, preferences.
pub fn data_dir() -> &'static Path {
    &paths().base.data
}

/// `logs/` — daily log files and the panic log.
pub fn logs_dir() -> &'static Path {
    &paths().base.logs
}

/// The persisted preferences file (`data/prefs.json`).
pub fn settings_file() -> PathBuf {
    data_dir().join("prefs.json")
}

/// The session database (`data/session.db`).
pub fn db_path() -> PathBuf {
    data_dir().join("session.db")
}

/// The per-installation single-instance lock (`data/instance.lock`).
/// Keyed on this path, two installs in different folders can run side
/// by side — each owns its own session and logs.
pub fn instance_lock() -> PathBuf {
    data_dir().join("instance.lock")
}

/// The panic dump (`logs/pixme.panic.log`) — separate from the daily
/// file: the panic hook must never share a writer with the appender.
pub fn panic_log() -> PathBuf {
    logs_dir().join("pixme.panic.log")
}

/// Today's rolling log file (`logs/pixme.<YYYY-MM-DD>.log`). The date
/// is UTC — tracing-appender rotates on `OffsetDateTime::now_utc` with
/// no local-time option (recorded in docs/decisions.md).
pub fn today_log_file() -> PathBuf {
    let date = chrono::Utc::now().format("%Y-%m-%d");
    logs_dir().join(format!("pixme.{date}.log"))
}

/// The exe-adjacent dir that failed the writability probe, when the
/// run fell back to `%LOCALAPPDATA%` (Windows only — the caller shows
/// the deferred user notice and the log warns).
pub fn fallback_from() -> Option<&'static Path> {
    paths().base.fallback_from.as_deref()
}

/// The previous per-user data dir when it still holds files — never
/// migrated, only reported.
pub fn old_data_dir() -> Option<&'static Path> {
    paths().legacy_data.as_deref()
}

/// Previous `prefs.json` locations, oldest convention first: beside the
/// executable (the pre-`data/` portable layout on Windows, dev runs on
/// macOS) and the per-user config dir (`.app` bundles, first releases).
/// The caller copies the first VALID one into [`settings_file`] — it
/// never deletes the source.
pub fn legacy_settings_files() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(dir) = exe_dir() {
        out.push(dir.join("prefs.json"));
    }
    if let Some(dir) = legacy_settings_dir() {
        out.push(dir.join("prefs.json"));
    }
    out
}

/// The pre-`prefs.json` dir holding `settings.toml` (named
/// `legacy_settings_dir` so call sites stay free of path-builder
/// tokens — see tests/data_paths.rs).
pub fn legacy_settings_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "pixme").map(|d| d.config_dir().to_path_buf())
}

/// The user's home directory — for path redaction only
/// (`logging::redact`); app data never lives directly under it.
pub fn user_home() -> Option<PathBuf> {
    directories::UserDirs::new().map(|u| u.home_dir().to_path_buf())
}

/// The executable's own directory, verbatim prefix stripped. Never the
/// current directory — shortcuts can launch with any working dir.
fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(strip_verbatim))
}

/// Removes a Windows verbatim prefix: `\\?\C:\foo` → `C:\foo`,
/// `\\?\UNC\server\share` → `\\server\share`. Other paths pass through
/// unchanged — including regular `\\server\share` UNC paths. Pure
/// string work: no canonicalization unless the OS hands us the prefix.
pub fn strip_verbatim(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        path.to_path_buf()
    }
}

/// Windows layout: `<preferred>\data` + `<preferred>\logs` when the
/// writability probe passed, else the `fallback` base (and
/// `fallback_from` records the rejected dir for the user notice).
/// Pure — the probe itself stays in [`probe_writable`].
pub fn pick_dirs(preferred: &Path, fallback: &Path, preferred_writable: bool) -> BaseDirs {
    if preferred_writable {
        BaseDirs {
            data: preferred.join("data"),
            logs: preferred.join("logs"),
            fallback_from: None,
        }
    } else {
        BaseDirs {
            data: fallback.join("data"),
            logs: fallback.join("logs"),
            fallback_from: Some(preferred.to_path_buf()),
        }
    }
}

/// Startup writability check: creates `data` and `logs`, then writes
/// and removes a uniquely-named probe file inside `data` (the process
/// id keeps parallel installs from colliding). Any failure → the
/// caller falls back.
pub fn probe_writable(data: &Path, logs: &Path) -> bool {
    if std::fs::create_dir_all(data).is_err() || std::fs::create_dir_all(logs).is_err() {
        return false;
    }
    let probe = data.join(format!(".pixme-write-{}.tmp", std::process::id()));
    match std::fs::write(&probe, b"ok") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// Last `n` components of `path` — logs never print a full user
/// profile (`file_name` alone loses which `data/` a `prefs.json` sits
/// in, two components keep it readable).
pub fn tail_path(path: &Path, n: usize) -> String {
    let parts: Vec<_> = path
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect();
    if parts.len() <= n {
        return path.display().to_string();
    }
    parts[parts.len() - n..].join("/")
}

/// Just the file name — for `info` events where the location is already
/// known (the banner records the directories once).
pub fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| tail_path(path, 2))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The portable reference case: `C:\portable\pixme\pixme.exe` puts
    /// its data in `C:\portable\pixme\data\` and logs in `...\logs\` —
    /// replacing the exe on update must never move either.
    #[test]
    fn portable_install_keeps_dirs_beside_the_exe() {
        let exe_dir = Path::new(r"C:\portable\pixme");
        let dirs = pick_dirs(exe_dir, Path::new(r"C:\Users\u\AppData\Local\PixMe"), true);
        assert_eq!(dirs.data, exe_dir.join("data"));
        assert_eq!(dirs.logs, exe_dir.join("logs"));
        assert!(dirs.fallback_from.is_none());
    }

    /// A launch from a subdirectory (shortcut with a different working
    /// dir) resolves from the exe dir, not the cwd — `pick_dirs` simply
    /// never sees a cwd-dependent input.
    #[test]
    fn read_only_exe_dir_falls_back_to_local_app_data() {
        let preferred = Path::new(r"C:\Program Files\pixme");
        let fallback = Path::new(r"C:\Users\u\AppData\Local\PixMe");
        let dirs = pick_dirs(preferred, fallback, false);
        assert_eq!(dirs.data, fallback.join("data"));
        assert_eq!(dirs.logs, fallback.join("logs"));
        assert_eq!(dirs.fallback_from.as_deref(), Some(preferred));
    }

    #[test]
    fn verbatim_prefixes_are_stripped() {
        assert_eq!(
            strip_verbatim(Path::new(r"\\?\C:\portable\pixme")),
            Path::new(r"C:\portable\pixme")
        );
        assert_eq!(
            strip_verbatim(Path::new(r"\\?\UNC\server\share\pixme")),
            Path::new(r"\\server\share\pixme")
        );
        // Already-plain UNC and local paths pass through untouched.
        assert_eq!(
            strip_verbatim(Path::new(r"\\server\share\pixme")),
            Path::new(r"\\server\share\pixme")
        );
        assert_eq!(
            strip_verbatim(Path::new("/Applications/PixMe.app/Contents/MacOS")),
            Path::new("/Applications/PixMe.app/Contents/MacOS")
        );
    }

    #[test]
    fn probe_writable_accepts_a_writable_dir() {
        let base = std::env::temp_dir().join(format!("pixme-probe-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        assert!(probe_writable(&base.join("data"), &base.join("logs")));
        assert!(base.join("data").is_dir() && base.join("logs").is_dir());
        // The probe file is removed afterwards — no litter.
        assert_eq!(std::fs::read_dir(base.join("data")).unwrap().count(), 0);
        std::fs::remove_dir_all(&base).unwrap();
    }

    /// A directory the process cannot create files in fails the probe —
    /// on unix a mode-555 dir simulates `C:\Program Files`.
    #[cfg(unix)]
    #[test]
    fn probe_writable_rejects_a_read_only_dir() {
        use std::os::unix::fs::PermissionsExt;
        let base = std::env::temp_dir().join(format!("pixme-ro-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o555)).unwrap();
        assert!(!probe_writable(&base.join("data"), &base.join("logs")));
        std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::remove_dir_all(&base).unwrap();
    }

    /// Paths with spaces and accents resolve verbatim — no mangling,
    /// no reparse through the OS.
    #[test]
    fn accented_and_spaced_dirs_join_verbatim() {
        let preferred = Path::new("/tmp/Mon Dossier été/pixme");
        let dirs = pick_dirs(preferred, Path::new("/fallback"), true);
        assert_eq!(dirs.data, preferred.join("data"));
    }

    #[test]
    fn tail_path_keeps_the_last_components() {
        assert_eq!(tail_path(Path::new("/a/b/c/d.txt"), 2), "c/d.txt");
        assert_eq!(tail_path(Path::new("/a.txt"), 2), "/a.txt");
        assert_eq!(file_name(Path::new("/a/b/session.db")), "session.db");
    }
}
