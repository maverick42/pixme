//! Locale completeness gate: `fr.yml` and `es.yml` must define exactly the
//! same keys as `en.yml` (no missing, no extra), with non-empty values.

use std::collections::BTreeMap;
use std::path::Path;

type Locale = BTreeMap<String, String>;

fn load(locale: &str) -> Locale {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("locales")
        .join(format!("{locale}.yml"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let (version, body) = text
        .split_once('\n')
        .unwrap_or_else(|| panic!("{} is empty", path.display()));
    assert_eq!(
        version.trim(),
        "_version: 1",
        "{locale}.yml must start with `_version: 1`"
    );
    serde_yml::from_str(body).unwrap_or_else(|e| panic!("invalid YAML in {}: {e}", path.display()))
}

#[test]
fn fr_and_es_cover_exactly_the_en_keys() {
    let en = load("en");
    assert!(!en.is_empty(), "en.yml is empty");

    for (locale, file) in [load("fr"), load("es")].into_iter().zip(["fr", "es"]) {
        for (key, value) in &locale {
            assert!(en.contains_key(key), "{file}.yml has extra key `{key}`");
            assert!(!value.trim().is_empty(), "{file}.yml: `{key}` is empty");
        }
        for key in en.keys() {
            assert!(
                locale.contains_key(key),
                "{file}.yml is missing key `{key}`"
            );
        }
    }
}
