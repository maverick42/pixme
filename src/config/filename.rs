//! Filename-template expansion for batch/auto saves (functional
//! reference parity): `$Y/$M/$D/$H/$N/$S` date fields, `#` sequential
//! number padded to the run width, `@` random alphanumeric per position
//! and `%VAR%` environment variables with cross-OS aliases.

use chrono::{DateTime, Datelike, Local, Timelike};

/// Expands `template` with the date `now` and sequential `counter`.
/// Unknown `$` codes and unset `%VAR%`s expand to nothing.
pub fn expand_template(template: &str, counter: u64, now: &DateTime<Local>) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '$' => match chars.next() {
                // `$$` is the escape for a literal dollar sign.
                Some('$') => out.push('$'),
                Some('Y') => out.push_str(&format!("{:04}", now.year())),
                Some('M') => out.push_str(&format!("{:02}", now.month())),
                Some('D') => out.push_str(&format!("{:02}", now.day())),
                Some('H') => out.push_str(&format!("{:02}", now.hour())),
                Some('N') => out.push_str(&format!("{:02}", now.minute())),
                Some('S') => out.push_str(&format!("{:02}", now.second())),
                Some(other) => {
                    out.push('$');
                    out.push(other);
                }
                None => out.push('$'),
            },
            '#' => {
                let mut width = 1;
                while chars.peek() == Some(&'#') {
                    chars.next();
                    width += 1;
                }
                out.push_str(&format!("{counter:0width$}"));
            }
            '@' => {
                let mut count = 1;
                while chars.peek() == Some(&'@') {
                    chars.next();
                    count += 1;
                }
                out.push_str(&random_alnum(count));
            }
            '%' => {
                let mut name = String::new();
                let mut closed = false;
                for c in chars.by_ref() {
                    if c == '%' {
                        closed = true;
                        break;
                    }
                    name.push(c);
                }
                if closed {
                    out.push_str(&env_value(&name));
                } else {
                    out.push('%');
                    out.push_str(&name);
                }
            }
            other => out.push(other),
        }
    }
    out
}

/// Characters that can never appear in a file name on macOS/Windows —
/// the template input filters them out and `sanitize_filename` maps
/// them to `-` when a tab title becomes a file name.
pub fn is_forbidden_filename_char(c: char) -> bool {
    matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control()
}

/// Turns `name` into a safe file stem: forbidden characters become `-`.
pub fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| {
            if is_forbidden_filename_char(c) {
                '-'
            } else {
                c
            }
        })
        .collect()
}

/// Resolves `%VAR%` placeholders: USERNAME/COMPUTERNAME get cross-OS
/// aliases (`USER`/`HOSTNAME` on macOS), anything else reads the
/// environment verbatim; unset vars expand to the empty string.
fn env_value(name: &str) -> String {
    let var = |key: &str| std::env::var(key).ok();
    match name {
        "USERNAME" => var("USERNAME").or_else(|| var("USER")),
        // `HOSTNAME` is a shell variable — absent in GUI-launched apps —
        // so fall back to the `hostname` command (present on both OSes).
        "COMPUTERNAME" => var("COMPUTERNAME")
            .or_else(|| var("HOSTNAME"))
            .or_else(hostname_cmd),
        other => var(other),
    }
    .unwrap_or_default()
}

/// The machine name via `hostname` — the reliable last resort when the
/// env vars are unset (Finder-launched macOS apps have no `HOSTNAME`).
fn hostname_cmd() -> Option<String> {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

/// `count` random characters from `[0-9a-zA-Z]` — a tiny xorshift avoids
/// pulling a random crate just for filename placeholders.
fn random_alnum(count: usize) -> String {
    const CHARSET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let mut seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9E37_79B9_7F4A_7C15)
        | 1;
    (0..count)
        .map(|_| {
            // xorshift64*
            seed ^= seed >> 12;
            seed ^= seed << 25;
            seed ^= seed >> 27;
            CHARSET[(seed.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 58) as usize % CHARSET.len()]
                as char
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> DateTime<Local> {
        Local
            .with_ymd_and_hms(y, mo, d, h, mi, s)
            .single()
            .expect("valid test date")
    }

    #[test]
    fn expands_date_codes() {
        let now = at(2026, 10, 27, 8, 43, 15);
        assert_eq!(
            expand_template("$Y-$M-$D_$H$N$S", 1, &now),
            "2026-10-27_084315"
        );
        // `$$` is the escape for a literal `$`.
        assert_eq!(expand_template("a$$b", 1, &now), "a$b");
    }

    #[test]
    fn hash_is_a_padded_sequential() {
        let now = at(2026, 10, 27, 8, 43, 15);
        assert_eq!(expand_template("Capture######", 32, &now), "Capture000032");
        assert_eq!(expand_template("#", 7, &now), "7");
    }

    #[test]
    fn at_expands_one_random_char_each() {
        let now = at(2026, 10, 27, 8, 43, 15);
        let out = expand_template("@@", 1, &now);
        assert_eq!(out.len(), 2);
        assert!(out.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn percent_vars_use_cross_os_aliases() {
        let now = at(2026, 10, 27, 8, 43, 15);
        // USERNAME resolves to the OS env user on this machine — never
        // the literal placeholder, never empty on a real session… but a
        // CI container may lack both, so only assert it does not panic
        // and expands deterministically.
        let out = expand_template("%USERNAME%_$Y", 1, &now);
        assert!(out.ends_with("_2026"));
        // An unset variable expands to nothing, and an unclosed `%`
        // stays literal.
        assert_eq!(expand_template("%PIXME_NEVER_SET_XYZ%", 1, &now), "");
        assert_eq!(expand_template("abc%", 1, &now), "abc%");
    }

    #[test]
    fn sanitize_replaces_forbidden_chars() {
        assert_eq!(
            sanitize_filename("a/b\\c:d*e?f\"g<h>i|j"),
            "a-b-c-d-e-f-g-h-i-j"
        );
        assert_eq!(sanitize_filename("normal name_1"), "normal name_1");
    }

    #[test]
    fn literals_and_text_pass_through() {
        let now = at(2026, 10, 27, 8, 43, 15);
        assert_eq!(expand_template("Screenshot", 1, &now), "Screenshot");
        assert_eq!(expand_template("$Y", 1, &now), "2026");
        // Unknown `$` code is kept literally.
        assert_eq!(expand_template("$Z", 1, &now), "$Z");
    }
}
