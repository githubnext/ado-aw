//! Validated value-object newtypes ("parse, don't validate").
//!
//! This module provides small newtypes that wrap a `String` and can **only** be
//! constructed by running a structural validator from [`crate::validate`]. Once
//! a value has one of these types, it is guaranteed to have passed the relevant
//! path / identifier / ref security checks — so a safe-output tool that types a
//! field as, say, [`RelativeSafePath`] cannot hold an unvalidated, attacker
//! controlled path.
//!
//! Each newtype:
//!
//! - exposes a fallible [`parse`](RelativeSafePath::parse) constructor plus
//!   `TryFrom<String>` / `TryFrom<&str>` / [`FromStr`](std::str::FromStr);
//! - validates **at deserialization time** via a hand-written `Deserialize`
//!   impl, so invalid JSON/YAML input is rejected before any `validate()` pass;
//! - serializes transparently back to a plain string (`Serialize`) and reports a
//!   plain string JSON schema (`JsonSchema`) so they are drop-in replacements for
//!   `String` fields in MCP `Params` structs;
//! - derefs to `&str` and implements `AsRef<str>` / `AsRef<Path>` / `Display`.
//!
//! ## Choosing a type
//!
//! - [`RelativeSafePath`] — agent-supplied file paths inside the workspace
//!   (allows hidden dotfiles like `.gitignore`; rejects traversal/absolute/`.git`).
//! - [`StrictRelativePath`] — like the above but every component must be a plain
//!   segment (no leading `.`, no `:`); use for attachment/artifact staging.
//! - [`PathSegment`] — a single path segment / alias (e.g. a repo checkout alias).
//! - [`GitRefName`] — a git ref obeying `git check-ref-format`.
//! - [`BranchName`] — a git branch ref with extra length / leading-`-` / space rules.
//! - [`CommitSha`] — a full 40-character hex commit SHA.
//! - [`ArtifactName`] — an ADO artifact / attachment name.
//! - [`Identifier`] — an engine agent/model identifier.
//! - [`HostName`] — a DNS-style hostname.
//! - [`Version`] — a version string (`1.2.3`, `latest`).
//!
//! New safe-output tools that accept paths or identifiers should type those
//! fields with these newtypes instead of raw `String` so the checks are applied
//! automatically and cannot be forgotten.

use std::path::Path;

use crate::validate;

/// Generate a validated string newtype.
///
/// `$validate` is an expression evaluating to a `fn(&str, &str) -> anyhow::Result<()>`
/// (value, label) used to validate the wrapped string.
macro_rules! validated_string {
    (
        $(#[$meta:meta])*
        $name:ident, $label:literal, $validate:expr
    ) => {
        $(#[$meta])*
        #[derive(
            Clone, Debug, PartialEq, Eq, Hash,
            serde::Serialize, schemars::JsonSchema,
        )]
        #[serde(transparent)]
        #[schemars(transparent)]
        pub struct $name(String);

        impl $name {
            /// Parse and validate, returning the newtype on success.
            pub fn parse(value: impl Into<String>) -> anyhow::Result<Self> {
                let value = value.into();
                let validate_fn: fn(&str, &str) -> anyhow::Result<()> = $validate;
                validate_fn(&value, $label)?;
                Ok(Self(value))
            }

            /// Borrow the validated value as a string slice.
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// Consume the newtype, returning the inner `String`.
            pub fn into_inner(self) -> String {
                self.0
            }
        }

        impl std::ops::Deref for $name {
            type Target = str;
            fn deref(&self) -> &str {
                &self.0
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl AsRef<Path> for $name {
            fn as_ref(&self) -> &Path {
                Path::new(&self.0)
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl TryFrom<String> for $name {
            type Error = anyhow::Error;
            fn try_from(value: String) -> anyhow::Result<Self> {
                Self::parse(value)
            }
        }

        impl TryFrom<&str> for $name {
            type Error = anyhow::Error;
            fn try_from(value: &str) -> anyhow::Result<Self> {
                Self::parse(value)
            }
        }

        impl std::str::FromStr for $name {
            type Err = anyhow::Error;
            fn from_str(value: &str) -> anyhow::Result<Self> {
                Self::parse(value)
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let raw = String::deserialize(deserializer)?;
                Self::parse(raw).map_err(serde::de::Error::custom)
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> String {
                value.0
            }
        }
    };
}

validated_string! {
    /// A safe relative file path inside the workspace.
    ///
    /// Allows hidden dotfiles (`.gitignore`); rejects empty, null bytes,
    /// newlines, pipeline commands, absolute / Windows-drive paths, `..`
    /// traversal, and any `.git` component. See
    /// [`crate::validate::validate_relative_safe_path`].
    RelativeSafePath, "path", validate::validate_relative_safe_path
}

validated_string! {
    /// A safe relative path whose every component is a plain segment
    /// (no leading `.`, no `:`). See
    /// [`crate::validate::validate_relative_segment_path`].
    StrictRelativePath, "path", validate::validate_relative_segment_path
}

validated_string! {
    /// A single safe path segment / alias (e.g. a repository checkout alias).
    PathSegment, "segment", |value: &str, label: &str| {
        if validate::is_safe_path_segment(value) {
            Ok(())
        } else {
            anyhow::bail!(
                "{label} '{value}' is not a safe path segment \
                 (no empty, '..', path separators, or leading '.')"
            )
        }
    }
}

validated_string! {
    /// A git ref name obeying `git check-ref-format`.
    GitRefName, "ref", validate::validate_git_ref_name
}

validated_string! {
    /// A git branch ref with additional length / leading-`-` / space rules.
    BranchName, "branch_name", |value: &str, label: &str| {
        if value.is_empty() {
            anyhow::bail!("{label} must not be empty");
        }
        if value.len() > 200 {
            anyhow::bail!("{label} must be at most 200 characters");
        }
        if value.contains('\0') {
            anyhow::bail!("{label} must not contain null bytes");
        }
        if value.starts_with('-') {
            anyhow::bail!("{label} must not start with '-'");
        }
        if value.contains(' ') {
            anyhow::bail!("{label} must not contain spaces");
        }
        validate::validate_git_ref_name(value, label)
    }
}

validated_string! {
    /// A full 40-character hex commit SHA.
    CommitSha, "commit", validate::validate_commit_sha
}

validated_string! {
    /// An Azure DevOps artifact / attachment name.
    ArtifactName, "artifact_name", |value: &str, label: &str| {
        if value.len() > 100 {
            anyhow::bail!("{label} must be at most 100 characters");
        }
        if value.starts_with('.') {
            anyhow::bail!("{label} must not start with '.'");
        }
        if validate::is_valid_artifact_name(value) {
            Ok(())
        } else {
            anyhow::bail!(
                "{label} must be non-empty and contain only alphanumeric characters, '-', '_' or '.'"
            )
        }
    }
}

validated_string! {
    /// An engine agent / model identifier.
    Identifier, "identifier", |value: &str, label: &str| {
        if validate::is_valid_identifier(value) {
            Ok(())
        } else {
            anyhow::bail!(
                "{label} '{value}' must be non-empty and contain only [A-Za-z0-9._:-]"
            )
        }
    }
}

validated_string! {
    /// A DNS-style hostname.
    HostName, "hostname", |value: &str, label: &str| {
        if validate::is_valid_hostname(value) {
            Ok(())
        } else {
            anyhow::bail!("{label} '{value}' must be non-empty and contain only [A-Za-z0-9.-]")
        }
    }
}

validated_string! {
    /// A version string (e.g. `1.2.3`, `latest`).
    Version, "version", |value: &str, label: &str| {
        if validate::is_valid_version(value) {
            Ok(())
        } else {
            anyhow::bail!("{label} '{value}' must be non-empty and contain only [A-Za-z0-9._-]")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_safe_path_parses_and_rejects() {
        assert!(RelativeSafePath::parse("src/main.rs").is_ok());
        assert!(RelativeSafePath::parse(".gitignore").is_ok());
        assert!(RelativeSafePath::parse("../escape").is_err());
        assert!(RelativeSafePath::parse(".git/config").is_err());
        assert_eq!(
            RelativeSafePath::parse("a/b.txt").unwrap().as_str(),
            "a/b.txt"
        );
    }

    #[test]
    fn strict_relative_path_is_stricter() {
        assert!(StrictRelativePath::parse("a/b.txt").is_ok());
        assert!(StrictRelativePath::parse(".hidden").is_err());
        assert!(StrictRelativePath::parse("a:b").is_err());
    }

    #[test]
    fn path_segment_rejects_separators() {
        assert!(PathSegment::parse("my-repo").is_ok());
        assert!(PathSegment::parse("a/b").is_err());
        assert!(PathSegment::parse("..").is_err());
        assert!(PathSegment::parse(".hidden").is_err());
    }

    #[test]
    fn branch_name_rules() {
        assert!(BranchName::parse("feature/x").is_ok());
        assert!(BranchName::parse("-bad").is_err());
        assert!(BranchName::parse("has space").is_err());
        assert!(BranchName::parse("a..b").is_err());
        assert!(BranchName::parse("x".repeat(201).as_str()).is_err());
    }

    #[test]
    fn commit_sha_rules() {
        assert!(CommitSha::parse("0123456789abcdef0123456789abcdef01234567").is_ok());
        assert!(CommitSha::parse("short").is_err());
    }

    #[test]
    fn artifact_name_rules() {
        assert!(ArtifactName::parse("build-logs_1.0").is_ok());
        assert!(ArtifactName::parse(".hidden").is_err());
        assert!(ArtifactName::parse("has space").is_err());
        assert!(ArtifactName::parse("a".repeat(101).as_str()).is_err());
    }

    #[test]
    fn deserialize_validates() {
        // Valid value round-trips.
        let ok: RelativeSafePath = serde_json::from_str("\"src/lib.rs\"").unwrap();
        assert_eq!(ok.as_str(), "src/lib.rs");
        // Invalid value fails at deserialize time.
        let err = serde_json::from_str::<RelativeSafePath>("\"../escape\"");
        assert!(err.is_err());
    }

    #[test]
    fn serialize_is_transparent() {
        let p = RelativeSafePath::parse("a/b.txt").unwrap();
        assert_eq!(serde_json::to_string(&p).unwrap(), "\"a/b.txt\"");
    }

    #[test]
    fn json_schema_is_string() {
        let schema = schemars::schema_for!(RelativeSafePath);
        let json = serde_json::to_value(&schema).unwrap();
        assert_eq!(json.get("type").and_then(|t| t.as_str()), Some("string"));
    }

    #[test]
    fn usable_as_path() {
        let p = RelativeSafePath::parse("a/b.txt").unwrap();
        let joined = Path::new("/base").join(&p);
        assert_eq!(joined, Path::new("/base/a/b.txt"));
    }
}
