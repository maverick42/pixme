//! Language selection and runtime locale switching.
//!
//! Translations live in `locales/{en,fr,es}.yml` (embedded at compile time by
//! `rust-i18n`, English is the fallback). This module maps the persisted
//! [`Language`] preference to a locale and applies it via
//! [`rust_i18n::set_locale`].

use serde::{Deserialize, Serialize};

/// The user's language preference (`language = "auto" | "en" | "fr" | "es"`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    /// Follow the OS locale, falling back to English when unsupported.
    #[default]
    Auto,
    /// English.
    En,
    /// French.
    Fr,
    /// Spanish.
    Es,
}

impl Language {
    /// Every supported explicit language (used by the settings picker).
    pub const ALL: [Self; 3] = [Self::En, Self::Fr, Self::Es];

    /// The locale this preference resolves to. `Auto` queries the OS.
    pub fn locale(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::Fr => "fr",
            Self::Es => "es",
            Self::Auto => system_locale(),
        }
    }

    /// The name shown in the language picker: each language names itself
    /// (not translated, per platform convention).
    pub fn native_name(self) -> &'static str {
        match self {
            Self::En => "English",
            Self::Fr => "Français",
            Self::Es => "Español",
            Self::Auto => unreachable!("Auto is labelled via i18n, not natively"),
        }
    }
}

/// Applies the preference globally: subsequent `t!` calls return strings in
/// this language.
pub fn apply(language: Language) {
    rust_i18n::set_locale(language.locale());
}

/// Maps the OS locale (`"fr-FR"`, `"es_419"`, …) to a supported locale,
/// defaulting to English.
fn system_locale() -> &'static str {
    sys_locale::get_locale().map_or("en", |locale| {
        let tag = locale.to_lowercase();
        if tag.starts_with("fr") {
            "fr"
        } else if tag.starts_with("es") {
            "es"
        } else {
            "en"
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_languages_map_to_their_locale() {
        assert_eq!(Language::En.locale(), "en");
        assert_eq!(Language::Fr.locale(), "fr");
        assert_eq!(Language::Es.locale(), "es");
    }

    #[test]
    fn auto_resolves_to_a_supported_locale() {
        assert!(["en", "fr", "es"].contains(&Language::Auto.locale()));
    }
}
