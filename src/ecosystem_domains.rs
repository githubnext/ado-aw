//! Ecosystem domain allowlists for network isolation.
//!
//! This module loads ecosystem-specific domain lists from an embedded JSON file
//! sourced from [gh-aw](https://github.com/github/gh-aw). The JSON maps ecosystem
//! identifiers (e.g., `"python"`, `"rust"`, `"node"`) to arrays of domains that
//! those ecosystems require for package management, registry access, etc.
//!
//! Users reference these identifiers in the `network.allowed` front matter field
//! instead of listing individual domains:
//!
//! ```yaml
//! network:
//!   allowed:
//!     - python
//!     - rust
//!     - "api.custom.com"
//! ```

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

/// Embedded ecosystem domains JSON, sourced from gh-aw.
static ECOSYSTEM_JSON: &str = include_str!("data/ecosystem_domains.json");

/// Parsed ecosystem domain map, loaded once at first access.
static ECOSYSTEM_DOMAINS: LazyLock<HashMap<String, Vec<String>>> = LazyLock::new(|| {
    serde_json::from_str(ECOSYSTEM_JSON).expect("embedded ecosystem_domains.json is invalid")
});

/// Compound ecosystems that expand to the union of multiple component ecosystems.
/// Mirrors gh-aw's `compoundEcosystems` mapping.
static COMPOUND_ECOSYSTEMS: LazyLock<HashMap<&'static str, Vec<&'static str>>> =
    LazyLock::new(|| {
        HashMap::from([(
            "default-safe-outputs",
            vec!["defaults", "dev-tools", "github", "local"],
        )])
    });

/// Returns the domains for a given ecosystem identifier.
///
/// Supports both direct ecosystem names (e.g., `"python"`) and compound
/// identifiers (e.g., `"default-safe-outputs"` which expands to
/// `defaults + dev-tools + github + local`).
///
/// Returns an empty `Vec` if the identifier is unknown.
pub fn get_ecosystem_domains(identifier: &str) -> Vec<String> {
    // Check compound ecosystems first
    if let Some(components) = COMPOUND_ECOSYSTEMS.get(identifier) {
        let mut domains: HashSet<String> = HashSet::new();
        for component in components {
            for d in get_ecosystem_domains(component) {
                domains.insert(d);
            }
        }
        let mut result: Vec<String> = domains.into_iter().collect();
        result.sort();
        return result;
    }

    ECOSYSTEM_DOMAINS
        .get(identifier)
        .cloned()
        .unwrap_or_default()
}

/// Returns `true` if the identifier is a known ecosystem name
/// (either a direct key in the JSON or a compound identifier).
pub fn is_known_ecosystem(identifier: &str) -> bool {
    ECOSYSTEM_DOMAINS.contains_key(identifier) || COMPOUND_ECOSYSTEMS.contains_key(identifier)
}

/// Returns the sorted list of all known ecosystem names
/// (both direct and compound).
#[cfg(test)]
pub fn known_ecosystem_names() -> Vec<String> {
    let mut names: Vec<String> = ECOSYSTEM_DOMAINS
        .keys()
        .cloned()
        .chain(COMPOUND_ECOSYSTEMS.keys().map(|k| k.to_string()))
        .collect();
    names.sort();
    names.dedup();
    names
}

/// Heuristic: ecosystem identifiers are composed of lowercase ASCII letters,
/// digits, and hyphens (e.g., `"python"`, `"linux-distros"`, `"default-safe-outputs"`).
/// Domain names contain dots (e.g., `"pypi.org"`, `"*.example.com"`).
/// Strings with spaces, special characters, or other unexpected content are
/// treated as neither — they fall through to domain validation which will reject them.
pub fn is_ecosystem_identifier(value: &str) -> bool {
    !value.is_empty()
        && !value.contains('.')
        && value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_known_ecosystems_loaded() {
        let names = known_ecosystem_names();
        assert!(names.contains(&"python".to_string()));
        assert!(names.contains(&"rust".to_string()));
        assert!(names.contains(&"node".to_string()));
        assert!(names.contains(&"go".to_string()));
        assert!(names.contains(&"defaults".to_string()));
        assert!(names.len() > 20, "expected 20+ ecosystems, got {}", names.len());
    }

    #[test]
    fn test_get_python_domains() {
        let domains = get_ecosystem_domains("python");
        assert!(domains.contains(&"pypi.org".to_string()));
        assert!(domains.contains(&"pip.pypa.io".to_string()));
        assert!(!domains.is_empty());
    }

    #[test]
    fn test_get_rust_domains() {
        let domains = get_ecosystem_domains("rust");
        assert!(domains.contains(&"crates.io".to_string()));
        assert!(domains.contains(&"static.rust-lang.org".to_string()));
    }

    #[test]
    fn test_get_node_domains() {
        let domains = get_ecosystem_domains("node");
        assert!(domains.contains(&"registry.npmjs.org".to_string()));
        assert!(domains.contains(&"nodejs.org".to_string()));
    }

    #[test]
    fn test_unknown_ecosystem_returns_empty() {
        let domains = get_ecosystem_domains("nonexistent-ecosystem");
        assert!(domains.is_empty());
    }

    #[test]
    fn test_is_known_ecosystem() {
        assert!(is_known_ecosystem("python"));
        assert!(is_known_ecosystem("rust"));
        assert!(is_known_ecosystem("default-safe-outputs"));
        assert!(!is_known_ecosystem("nonexistent"));
    }

    #[test]
    fn test_compound_ecosystem() {
        let domains = get_ecosystem_domains("default-safe-outputs");
        assert!(!domains.is_empty());
        // Should include domains from defaults, dev-tools, github, local
        assert!(domains.contains(&"github.com".to_string()), "should include github domains");
        assert!(domains.contains(&"localhost".to_string()), "should include local domains");
    }

    #[test]
    fn test_is_ecosystem_identifier_heuristic() {
        // Ecosystem identifiers (lowercase + hyphens)
        assert!(is_ecosystem_identifier("python"));
        assert!(is_ecosystem_identifier("rust"));
        assert!(is_ecosystem_identifier("node"));
        assert!(is_ecosystem_identifier("default-safe-outputs"));
        assert!(is_ecosystem_identifier("linux-distros"));

        // Domain names (have dots)
        assert!(!is_ecosystem_identifier("pypi.org"));
        assert!(!is_ecosystem_identifier("*.example.com"));
        assert!(!is_ecosystem_identifier("api.github.com"));

        // Invalid strings (special chars, spaces, uppercase)
        assert!(!is_ecosystem_identifier(""));
        assert!(!is_ecosystem_identifier("bad host!"));
        assert!(!is_ecosystem_identifier("PYTHON"));
        assert!(!is_ecosystem_identifier("hello world"));
    }

    #[test]
    fn test_defaults_ecosystem_has_expected_entries() {
        let domains = get_ecosystem_domains("defaults");
        // Certificate infrastructure
        assert!(domains.contains(&"ocsp.digicert.com".to_string()));
        // Ubuntu
        assert!(domains.contains(&"archive.ubuntu.com".to_string()));
    }
}
