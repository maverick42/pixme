//! Guards the `src/paths.rs` contract: no other module may build an
//! application-data path itself. This scan fails when a forbidden
//! token (`ProjectDirs`, `current_exe`, `LOCALAPPDATA`, `config_dir`,
//! `data_local_dir`, `home_dir`, `Library/Logs`) appears outside the
//! files allowed to resolve locations: `src/paths.rs` and
//! `src/platform/`.
//!
//! `directories::UserDirs`/`picture_dir` stay legal anywhere — export
//! destinations are user-visible choices, not app data.

use std::path::{Path, PathBuf};

/// Tokens that only `paths`/`platform` may contain.
const FORBIDDEN: &[&str] = &[
    "ProjectDirs",
    "BaseDirs",
    "current_exe",
    "LOCALAPPDATA",
    "data_local_dir",
    "config_dir",
    "home_dir",
    "Library/Logs",
    "Library\\Logs",
];

/// Path fragments whose files are allowed to touch those tokens.
fn allowed(file: &Path, src_root: &Path) -> bool {
    let rel = file.strip_prefix(src_root).unwrap_or(file);
    rel == Path::new("paths.rs") || rel.starts_with("platform")
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn no_data_path_is_built_outside_the_paths_module() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    let mut violations = Vec::new();
    for file in files {
        if allowed(&file, &src) {
            continue;
        }
        let text = std::fs::read_to_string(&file).unwrap();
        for (lineno, line) in text.lines().enumerate() {
            for token in FORBIDDEN {
                if line.contains(token) {
                    violations.push(format!(
                        "{}:{}: {token} — {line}",
                        file.display(),
                        lineno + 1
                    ));
                }
            }
        }
    }
    assert!(
        violations.is_empty(),
        "data paths must live in src/paths.rs or src/platform/:\n{}",
        violations.join("\n")
    );
}
