//! Capture metadata: internal identity, provenance and the optional
//! screen-lock password — the fields the future database persistence
//! restores a capture from.
//!
//! No external dependency: the internal id is a home-grown UUID-shaped
//! value (timestamp + counter + pid + OS-seeded `RandomState` entropy),
//! and the password hash is a salted iterated FNV-1a. Neither is
//! cryptographic — the protection is a *screen lock*: it keeps casual
//! eyes out, nothing more. Pixels stay readable in memory and a saved
//! file is never protected.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{DateTime, Local};

/// Persistent identity + provenance of a capture.
#[derive(Clone, Debug)]
pub struct CaptureMeta {
    /// Internal unique id (UUID-shaped, never displayed).
    pub id: String,
    /// When the capture was taken (the source file's mtime on Open).
    pub captured_at: DateTime<Local>,
    /// Last successful save (`Some` = written to disk at least once).
    pub saved_at: Option<DateTime<Local>>,
    /// Source file / last save location — enables "Delete file" and
    /// shows where a re-save would land.
    pub source_path: Option<PathBuf>,
    /// Salted password hash; `Some` = the tab is a screen lock that asks
    /// for the password every time it becomes visible. Carried in the
    /// meta so a future database round-trip keeps the protection.
    pub password_hash: Option<String>,
}

impl CaptureMeta {
    /// Metadata for a brand-new capture: fresh id, captured now.
    pub fn fresh() -> Self {
        Self {
            id: new_id(),
            captured_at: Local::now(),
            saved_at: None,
            source_path: None,
            password_hash: None,
        }
    }

    /// `true` when the capture asks for a password on activation.
    pub fn is_protected(&self) -> bool {
        self.password_hash.is_some()
    }

    /// Arms the screen lock (`""` disarms it).
    pub fn set_password(&mut self, password: &str) {
        self.password_hash = if password.is_empty() {
            None
        } else {
            Some(hash_password(password))
        };
    }

    /// Password check against the stored hash (always `false` unarmed).
    pub fn check_password(&self, password: &str) -> bool {
        self.password_hash
            .as_deref()
            .is_some_and(|stored| check_password(stored, password))
    }
}

/// Counter folded into the entropy so two ids minted in the same
/// nanosecond still differ.
static ID_COUNTER: AtomicU64 = AtomicU64::new(0);

/// One 64-bit draw of entropy: `RandomState` is OS-seeded per process,
/// the nanos/counter/pid make every call distinct.
fn entropy(salt: u64) -> u64 {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let mut h = RandomState::new().build_hasher();
    h.write_u64(salt);
    h.write_u64(ID_COUNTER.fetch_add(1, Ordering::Relaxed));
    h.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    );
    h.write_u32(std::process::id());
    h.finish()
}

/// A UUIDv4-shaped lowercase string — `8-4-4-4-12` hex with the version
/// and variant bits set so it reads as a real UUID to any future store.
pub fn new_id() -> String {
    let (a, b) = (entropy(0xA5A5), entropy(0x5A5A));
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        a as u32,
        (a >> 32) as u16,
        ((a >> 48) as u16 & 0x0fff) | 0x4000, // version 4
        ((b >> 48) as u16 & 0x3fff) | 0x8000, // variant 10xx
        b & 0x0000_ffff_ffff_ffff
    )
}

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
/// Iterations — enough that casual probing feels instant but brute
/// forcing a real password stays silly (this is a screen lock).
const HASH_ROUNDS: u32 = 10_000;

/// Salted iterated FNV-1a of `password`, stored as
/// `v1:<salt_hex>:<hash_hex>` — self-describing so a future version can
/// change the recipe without breaking stored hashes.
pub fn hash_password(password: &str) -> String {
    let salt = entropy(0x5A17);
    format!("v1:{salt:016x}:{:016x}", fnv_rounds(password, salt))
}

/// Recomputes `stored`'s round for `password` and compares.
pub fn check_password(stored: &str, password: &str) -> bool {
    let mut parts = stored.split(':');
    let (Some("v1"), Some(salt_hex), Some(hash_hex), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    let (Ok(salt), Ok(expected)) = (
        u64::from_str_radix(salt_hex, 16),
        u64::from_str_radix(hash_hex, 16),
    ) else {
        return false;
    };
    fnv_rounds(password, salt) == expected
}

fn fnv_rounds(password: &str, salt: u64) -> u64 {
    let mut h = FNV_OFFSET;
    let bytes = password.as_bytes();
    for round in 0..HASH_ROUNDS {
        for &b in bytes {
            h = (h ^ u64::from(b)).wrapping_mul(FNV_PRIME);
        }
        // The salt and the round number feed every lap so equal
        // passwords under different salts diverge immediately.
        h = (h ^ salt.wrapping_add(u64::from(round))).wrapping_mul(FNV_PRIME);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_uuid_shaped_and_unique() {
        let id = new_id();
        let parts: Vec<&str> = id.split('-').collect();
        assert_eq!(parts.len(), 5);
        assert_eq!(parts[0].len(), 8);
        assert_eq!(parts[3].len(), 4);
        assert!(parts[2].starts_with('4'), "version nibble: {id}");
        assert!(
            "89ab".contains(parts[3].chars().next().unwrap()),
            "variant: {id}"
        );
        assert_ne!(id, new_id(), "two draws collided");
    }

    #[test]
    fn password_roundtrip() {
        let stored = hash_password("hunter2");
        assert!(check_password(&stored, "hunter2"));
        assert!(!check_password(&stored, "hunter3"));
        assert!(!check_password(&stored, ""));
        assert!(!check_password("garbage", "hunter2"));
        // Same password → different salts → different stored strings.
        assert_ne!(stored, hash_password("hunter2"));
    }

    #[test]
    fn meta_lock_semantics() {
        let mut meta = CaptureMeta::fresh();
        assert!(!meta.is_protected());
        assert!(!meta.check_password("x"));
        meta.set_password("secret");
        assert!(meta.is_protected());
        assert!(meta.check_password("secret"));
        meta.set_password("");
        assert!(!meta.is_protected());
    }
}
