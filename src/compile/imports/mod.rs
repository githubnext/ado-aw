//! Compile-time resolution for `imports:` entries.
//!
//! This module deliberately stops at resolution: it fetches/loads the imported
//! markdown manifest, parses its front matter and body, and records provenance.
//! Merging imported content into the consumer workflow is a later compile pass.
#![allow(dead_code)]

pub mod alias;
#[cfg(test)]
mod integration_tests;
pub mod merge;
pub mod schema;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::Deserialize;

use crate::compile::types::{ImportEntry, ImportSource, ParsedImportSpec};
use crate::hash::sha256_hex;

const MAX_IMPORTS_PER_WORKFLOW: usize = 20;
const MAX_MANIFEST_BYTES: usize = 256 * 1024;
const IMPORT_GITATTRIBUTES: &str = "# Mark all cached import files as generated\n\
* linguist-generated=true\n\
# Keep local cached versions on merge\n\
* merge=ours\n";

/// Fetches a single SHA-pinned component manifest.
pub trait ManifestFetcher {
    fn fetch(&self, spec: &ParsedImportSpec) -> Result<Vec<u8>>;
}

/// GitHub Contents API-backed manifest fetcher using the author's `gh` auth.
pub struct GhCliFetcher;

impl ManifestFetcher for GhCliFetcher {
    fn fetch(&self, spec: &ParsedImportSpec) -> Result<Vec<u8>> {
        // TODO: Azure Repos manifest fetch (follow-up).
        let route = format!(
            "repos/{}/{}/contents/{}?ref={}",
            spec.owner,
            spec.repo,
            spec.path,
            spec.sha.as_str()
        );
        let output = Command::new("gh")
            .args(["api", &route])
            .output()
            .with_context(|| format!("failed to run `gh api {route}` for import manifest"))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!(
                "`gh api {}` failed with status {}: {}",
                route,
                output.status,
                stderr.trim()
            );
        }

        #[derive(Deserialize)]
        struct ContentsResponse {
            content: String,
            #[serde(default)]
            encoding: Option<String>,
        }

        let response: ContentsResponse = serde_json::from_slice(&output.stdout)
            .with_context(|| format!("failed to parse GitHub Contents API response for {route}"))?;
        if response.encoding.as_deref().unwrap_or("base64") != "base64" {
            anyhow::bail!(
                "GitHub Contents API response for {} used unsupported encoding {:?}",
                route,
                response.encoding
            );
        }

        let compact_content: String = response
            .content
            .chars()
            .filter(|ch| !ch.is_whitespace())
            .collect();
        STANDARD
            .decode(compact_content.as_bytes())
            .with_context(|| {
                format!("failed to base64-decode GitHub Contents API response for {route}")
            })
    }
}

/// A resolved import manifest plus source provenance.
#[derive(Debug, Clone)]
pub struct ResolvedImport {
    pub entry: ImportEntry,
    pub source: ImportSource,
    pub front_matter: serde_yaml::Value,
    pub body: String,
    pub provenance: ImportProvenance,
}

/// Audit provenance for a resolved import manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportProvenance {
    pub source: String,
    pub sha: Option<String>,
    pub manifest_digest: String,
}

/// Resolve top-level imports using `base_dir` for local paths and cache root.
///
/// Prefer [`resolve_imports_with_repo_root`] when the workflow directory is not
/// the repository root. This function is kept as the simple public entry point
/// for callers that compile from the repo root.
pub fn resolve_imports(
    entries: &[ImportEntry],
    base_dir: &Path,
    fetcher: &dyn ManifestFetcher,
) -> Result<Vec<ResolvedImport>> {
    resolve_imports_with_repo_root(entries, base_dir, base_dir, fetcher)
}

/// Resolve top-level imports using an explicit repo root for the committed
/// `.ado-aw/imports` cache.
///
/// TODO: nested imports (depth<=3). This pass intentionally resolves only the
/// workflow's declared top-level `imports:` list; transitive resolution will
/// layer on this entry point in a later merge pass.
pub fn resolve_imports_with_repo_root(
    entries: &[ImportEntry],
    base_dir: &Path,
    repo_root: &Path,
    fetcher: &dyn ManifestFetcher,
) -> Result<Vec<ResolvedImport>> {
    if entries.len() > MAX_IMPORTS_PER_WORKFLOW {
        anyhow::bail!(
            "imports per workflow must be <= {}, got {}",
            MAX_IMPORTS_PER_WORKFLOW,
            entries.len()
        );
    }

    let mut resolved = Vec::new();
    for entry in entries {
        if let Some(import) = resolve_one(entry, base_dir, repo_root, fetcher)
            .with_context(|| format!("failed to resolve import `{}`", entry.uses))?
        {
            resolved.push(import);
        }
    }
    Ok(resolved)
}

fn resolve_one(
    entry: &ImportEntry,
    base_dir: &Path,
    repo_root: &Path,
    fetcher: &dyn ManifestFetcher,
) -> Result<Option<ResolvedImport>> {
    let source = entry.parse_source()?;
    match &source {
        ImportSource::Local {
            path,
            section,
            optional,
        } => {
            let local_path = resolve_local_path(base_dir, path)?;
            let bytes = match fs::read(&local_path) {
                Ok(bytes) => bytes,
                Err(err) if *optional && err.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(None);
                }
                Err(err) => {
                    return Err(err).with_context(|| {
                        format!("failed to read local import {}", local_path.display())
                    });
                }
            };
            let digest = sha256_hex(&bytes);
            let (front_matter, body) = parse_manifest_bytes(&bytes, section.as_deref())?;
            Ok(Some(ResolvedImport {
                entry: entry.clone(),
                source: source.clone(),
                front_matter,
                body,
                provenance: ImportProvenance {
                    source: path.clone(),
                    sha: None,
                    manifest_digest: digest,
                },
            }))
        }
        ImportSource::Remote(spec) => {
            let bytes = read_remote_manifest(repo_root, spec, fetcher)?;
            let digest = sha256_hex(&bytes);
            let (front_matter, body) = parse_manifest_bytes(&bytes, spec.section.as_deref())?;
            Ok(Some(ResolvedImport {
                entry: entry.clone(),
                source: source.clone(),
                front_matter,
                body,
                provenance: ImportProvenance {
                    source: format!("{}/{}/{}", spec.owner, spec.repo, spec.path),
                    sha: Some(spec.sha.as_str().to_string()),
                    manifest_digest: digest,
                },
            }))
        }
    }
}

fn resolve_local_path(base_dir: &Path, import_path: &str) -> Result<PathBuf> {
    let path = Path::new(import_path);
    if path.is_absolute() {
        anyhow::bail!("local import path must be relative, got `{}`", import_path);
    }
    Ok(base_dir.join(path))
}

fn read_remote_manifest(
    repo_root: &Path,
    spec: &ParsedImportSpec,
    fetcher: &dyn ManifestFetcher,
) -> Result<Vec<u8>> {
    let cache_path = cache_path(repo_root, spec)?;
    if cache_path.exists() {
        let bytes = fs::read(&cache_path)
            .with_context(|| format!("failed to read cached import {}", cache_path.display()))?;
        enforce_manifest_size(bytes.len(), &cache_path.display().to_string())?;
        return Ok(bytes);
    }

    let bytes = fetcher.fetch(spec)?;
    enforce_manifest_size(
        bytes.len(),
        &format!("{}/{}/{}", spec.owner, spec.repo, spec.path),
    )?;

    let parent = cache_path
        .parent()
        .context("import cache path unexpectedly has no parent")?;
    fs::create_dir_all(parent).with_context(|| {
        format!(
            "failed to create import cache directory {}",
            parent.display()
        )
    })?;
    ensure_import_gitattributes(repo_root)?;
    fs::write(&cache_path, &bytes)
        .with_context(|| format!("failed to write cached import {}", cache_path.display()))?;
    Ok(bytes)
}

fn enforce_manifest_size(size: usize, source: &str) -> Result<()> {
    if size > MAX_MANIFEST_BYTES {
        anyhow::bail!(
            "import manifest {} is {} bytes, exceeding the {} byte limit",
            source,
            size,
            MAX_MANIFEST_BYTES
        );
    }
    Ok(())
}

fn cache_path(repo_root: &Path, spec: &ParsedImportSpec) -> Result<PathBuf> {
    validate_cache_segment("owner", &spec.owner)?;
    validate_cache_segment("repo", &spec.repo)?;
    let flat_path = flatten_import_path(&spec.path)?;
    Ok(repo_root
        .join(".ado-aw")
        .join("imports")
        .join(&spec.owner)
        .join(&spec.repo)
        .join(spec.sha.as_str())
        .join(flat_path))
}

fn validate_cache_segment(label: &str, value: &str) -> Result<()> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.contains('\\')
        || value.contains('/')
    {
        anyhow::bail!(
            "remote import {} contains an invalid path segment: `{}`",
            label,
            value
        );
    }
    Ok(())
}

fn flatten_import_path(path: &str) -> Result<String> {
    if path.is_empty()
        || path.contains('\\')
        || path
            .split('/')
            .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        anyhow::bail!("remote import path contains an invalid segment: `{}`", path);
    }
    Ok(path.replace('/', "_"))
}

fn ensure_import_gitattributes(repo_root: &Path) -> Result<()> {
    let imports_dir = repo_root.join(".ado-aw").join("imports");
    fs::create_dir_all(&imports_dir).with_context(|| {
        format!(
            "failed to create import cache attributes directory {}",
            imports_dir.display()
        )
    })?;
    let attributes_path = imports_dir.join(".gitattributes");
    if !attributes_path.exists() {
        fs::write(&attributes_path, IMPORT_GITATTRIBUTES).with_context(|| {
            format!(
                "failed to write import cache attributes {}",
                attributes_path.display()
            )
        })?;
    }
    Ok(())
}

fn parse_manifest_bytes(
    bytes: &[u8],
    section: Option<&str>,
) -> Result<(serde_yaml::Value, String)> {
    enforce_manifest_size(bytes.len(), "resolved import manifest")?;
    let content =
        std::str::from_utf8(bytes).context("import manifest must be valid UTF-8 markdown")?;
    let parts = super::common::split_markdown_front_matter(content, false)?;
    let front_matter = match parts.yaml_raw {
        Some(yaml) => {
            let value: serde_yaml::Value =
                serde_yaml::from_str(&yaml).context("failed to parse import YAML front matter")?;
            match value {
                serde_yaml::Value::Mapping(_) | serde_yaml::Value::Null => value,
                other => {
                    anyhow::bail!(
                        "import YAML front matter must be a mapping/object, got {}",
                        yaml_value_kind(&other)
                    );
                }
            }
        }
        None => serde_yaml::Value::Null,
    };

    let body = match section {
        Some(name) => extract_markdown_section(&parts.markdown_body, name)?,
        None => parts.markdown_body,
    };
    Ok((front_matter, body))
}

fn yaml_value_kind(value: &serde_yaml::Value) -> &'static str {
    match value {
        serde_yaml::Value::Null => "null",
        serde_yaml::Value::Bool(_) => "boolean",
        serde_yaml::Value::Number(_) => "number",
        serde_yaml::Value::String(_) => "string",
        serde_yaml::Value::Sequence(_) => "sequence/array",
        serde_yaml::Value::Mapping(_) => "mapping/object",
        serde_yaml::Value::Tagged(_) => "tagged value",
    }
}

/// Extract a markdown `# Name` / `## Name` section, including its heading.
fn extract_markdown_section(body: &str, section: &str) -> Result<String> {
    let lines: Vec<&str> = body.lines().collect();
    let start = lines
        .iter()
        .position(|line| markdown_heading(line).is_some_and(|(_, name)| name == section))
        .ok_or_else(|| anyhow::anyhow!("import section `{}` was not found", section))?;
    let start_level = markdown_heading(lines[start])
        .map(|(level, _)| level)
        .expect("line matched heading above");

    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find_map(|(idx, line)| match markdown_heading(line) {
            Some((level, _)) if level <= start_level => Some(idx),
            _ => None,
        })
        .unwrap_or(lines.len());

    Ok(lines[start..end].join("\n").trim().to_string())
}

fn markdown_heading(line: &str) -> Option<(usize, &str)> {
    let trimmed = line.trim_start();
    let level = trimmed.bytes().take_while(|byte| *byte == b'#').count();
    if !(level == 1 || level == 2) {
        return None;
    }
    let rest = &trimmed[level..];
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let name = rest
        .trim()
        .trim_end_matches('#')
        .trim_end()
        .trim()
        .trim_end_matches('\r');
    if name.is_empty() {
        return None;
    }
    Some((level, name))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    struct StaticFetcher {
        bytes: Vec<u8>,
    }

    impl ManifestFetcher for StaticFetcher {
        fn fetch(&self, _spec: &ParsedImportSpec) -> Result<Vec<u8>> {
            Ok(self.bytes.clone())
        }
    }

    struct PanicFetcher;

    impl ManifestFetcher for PanicFetcher {
        fn fetch(&self, _spec: &ParsedImportSpec) -> Result<Vec<u8>> {
            panic!("fetcher must not be called on cache hit")
        }
    }

    fn import_entry(uses: &str) -> ImportEntry {
        ImportEntry {
            uses: uses.to_string(),
            with: serde_json::Map::new(),
            endpoint: None,
        }
    }

    fn manifest() -> &'static [u8] {
        b"---\nimport-schema:\n  region:\n    type: string\n---\n# Imported\nBody\n"
    }

    #[test]
    fn local_import_resolves_front_matter_body_and_provenance() {
        let repo = tempfile::tempdir().unwrap();
        let workflow_dir = repo.path().join("workflows");
        fs::create_dir_all(&workflow_dir).unwrap();
        let import_path = workflow_dir.join("component.md");
        fs::write(&import_path, manifest()).unwrap();

        let resolved = resolve_imports_with_repo_root(
            &[import_entry("component.md")],
            &workflow_dir,
            repo.path(),
            &PanicFetcher,
        )
        .unwrap();

        assert_eq!(resolved.len(), 1);
        assert!(resolved[0].front_matter["import-schema"].is_mapping());
        assert_eq!(resolved[0].body, "# Imported\nBody");
        assert_eq!(resolved[0].provenance.source, "component.md");
        assert_eq!(resolved[0].provenance.sha, None);
        assert_eq!(
            resolved[0].provenance.manifest_digest,
            sha256_hex(manifest())
        );
    }

    #[test]
    fn remote_import_fetches_writes_cache_attributes_and_records_digest() {
        let repo = tempfile::tempdir().unwrap();
        let entry = import_entry(&format!("acme/shared/components/deploy.md@{SHA}"));
        let fetcher = StaticFetcher {
            bytes: manifest().to_vec(),
        };

        let resolved =
            resolve_imports_with_repo_root(&[entry], repo.path(), repo.path(), &fetcher).unwrap();

        let cache_file = repo
            .path()
            .join(".ado-aw")
            .join("imports")
            .join("acme")
            .join("shared")
            .join(SHA)
            .join("components_deploy.md");
        assert!(cache_file.exists());
        assert_eq!(fs::read(&cache_file).unwrap(), manifest());
        let attributes = fs::read_to_string(
            repo.path()
                .join(".ado-aw")
                .join("imports")
                .join(".gitattributes"),
        )
        .unwrap();
        assert_eq!(attributes, IMPORT_GITATTRIBUTES);
        assert_eq!(
            resolved[0].provenance.source,
            "acme/shared/components/deploy.md"
        );
        assert_eq!(resolved[0].provenance.sha.as_deref(), Some(SHA));
        assert_eq!(
            resolved[0].provenance.manifest_digest,
            sha256_hex(manifest())
        );
    }

    #[test]
    fn remote_import_uses_cache_before_fetching() {
        let repo = tempfile::tempdir().unwrap();
        let entry = import_entry(&format!("acme/shared/components/deploy.md@{SHA}"));
        let cache_dir = repo
            .path()
            .join(".ado-aw")
            .join("imports")
            .join("acme")
            .join("shared")
            .join(SHA);
        fs::create_dir_all(&cache_dir).unwrap();
        fs::write(cache_dir.join("components_deploy.md"), manifest()).unwrap();

        let resolved =
            resolve_imports_with_repo_root(&[entry], repo.path(), repo.path(), &PanicFetcher)
                .unwrap();

        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].body, "# Imported\nBody");
    }

    #[test]
    fn size_cap_rejects_large_manifest() {
        let repo = tempfile::tempdir().unwrap();
        let entry = import_entry(&format!("acme/shared/component.md@{SHA}"));
        let fetcher = StaticFetcher {
            bytes: vec![b'x'; MAX_MANIFEST_BYTES + 1],
        };

        let err = resolve_imports_with_repo_root(&[entry], repo.path(), repo.path(), &fetcher)
            .unwrap_err();

        assert!(format!("{err:#}").contains("exceeding the 262144 byte limit"));
    }

    #[test]
    fn section_selector_extracts_only_that_section() {
        let repo = tempfile::tempdir().unwrap();
        let workflow_dir = repo.path();
        fs::write(
            workflow_dir.join("component.md"),
            b"---\n{}\n---\n# One\none\n## Two\ntwo\n### Detail\nkeep\n## Three\nthree\n",
        )
        .unwrap();

        let resolved = resolve_imports_with_repo_root(
            &[import_entry("component.md#Two")],
            workflow_dir,
            repo.path(),
            &PanicFetcher,
        )
        .unwrap();

        assert_eq!(resolved[0].body, "## Two\ntwo\n### Detail\nkeep");
    }

    #[test]
    fn optional_missing_local_import_is_skipped_and_required_missing_errors() {
        let repo = tempfile::tempdir().unwrap();

        let optional = resolve_imports_with_repo_root(
            &[import_entry("missing.md?")],
            repo.path(),
            repo.path(),
            &PanicFetcher,
        )
        .unwrap();
        assert!(optional.is_empty());

        let err = resolve_imports_with_repo_root(
            &[import_entry("missing.md")],
            repo.path(),
            repo.path(),
            &PanicFetcher,
        )
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("failed to resolve import `missing.md`")
        );
    }

    #[test]
    fn imports_per_workflow_limit_is_enforced() {
        let repo = tempfile::tempdir().unwrap();
        let entries: Vec<ImportEntry> = (0..=MAX_IMPORTS_PER_WORKFLOW)
            .map(|idx| import_entry(&format!("component-{idx}.md?")))
            .collect();

        let err = resolve_imports_with_repo_root(&entries, repo.path(), repo.path(), &PanicFetcher)
            .unwrap_err();

        assert!(
            err.to_string()
                .contains("imports per workflow must be <= 20")
        );
    }
}
