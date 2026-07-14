//! Synthesis of compiler-internal repository-resource aliases from resolved
//! remote imports (for the runtime executor-job checkout).
//!
//! The compiler owns these aliases: authors write only `imports:` entries, and
//! this pass derives the ADO `resources.repositories` entries needed by later
//! executor jobs. Aliases are stable, valid ADO identifiers, and readable:
//! `import_<sanitized-owner>_<sanitized-repo>_<hash>`. The readable parts replace
//! non-ASCII-identifier characters with `_`; the fixed hash suffix is derived
//! from the original `owner/repo`, so repos that collide under simple
//! sanitization (for example `a-b/c` and `a_b/c`) still get distinct aliases
//! without requiring global import-set context.
//!
//! Repository resources are deduplicated by `(owner, repo, endpoint)`, not by
//! manifest path or SHA: ADO repository-resource refs are branch/tag-level
//! authorization handles, while the executor job will pin the exact commit with
//! `git checkout <sha>` after checkout.
#![allow(dead_code)]

use std::collections::{HashMap, HashSet};

use anyhow::Result;

use super::ResolvedImport;
use crate::compile::types::{CompileTarget, ImportSource, Repository};
use crate::hash::sha256_hex;

const REPOSITORY_RESOURCE_REF: &str = "refs/heads/main";
const HASH_SUFFIX_LEN: usize = 12;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct RepoKey {
    owner: String,
    repo: String,
    endpoint: Option<String>,
}

/// Return the stable compiler-generated repository-resource alias for a remote
/// import source.
///
/// The alias always starts with `import_`, contains only ASCII alphanumeric
/// characters and underscores, and includes a short SHA-256 suffix of the
/// original `owner/repo` to avoid collisions from sanitization alone.
pub fn alias_identifier(owner: &str, repo: &str) -> String {
    let digest = short_hash(&format!("{owner}/{repo}"));
    let owner = sanitize_identifier_part(owner);
    let repo = sanitize_identifier_part(repo);
    format!("import_{owner}_{repo}_{digest}")
}

/// Synthesize ADO repository resources for remote imports.
///
/// Local imports are compile-time only and do not need a runtime checkout, so
/// they are skipped. Remote imports from the same `(owner, repo, endpoint)` are
/// collapsed to one resource even when they point at different manifest paths or
/// SHAs; the resource `ref` is only an ADO authorization ref, not the execution
/// pin.
pub fn synthesize_repo_aliases(imports: &[ResolvedImport]) -> Result<Vec<Repository>> {
    let mut ordered_keys = Vec::new();
    let mut seen = HashSet::new();

    for import in imports {
        let ImportSource::Remote(spec) = &import.source else {
            continue;
        };

        let key = RepoKey {
            owner: spec.owner.clone(),
            repo: spec.repo.clone(),
            endpoint: import.entry.endpoint.clone(),
        };

        if seen.insert(key.clone()) {
            ordered_keys.push(key);
        }
    }

    let mut alias_counts: HashMap<String, usize> = HashMap::new();
    for key in &ordered_keys {
        *alias_counts
            .entry(alias_identifier(&key.owner, &key.repo))
            .or_default() += 1;
    }

    Ok(ordered_keys
        .into_iter()
        .map(|key| {
            let base_alias = alias_identifier(&key.owner, &key.repo);
            let alias = if alias_counts.get(&base_alias).copied().unwrap_or(0) > 1 {
                format!(
                    "{}_{}",
                    base_alias,
                    short_hash(&format!(
                        "{}/{}/{}",
                        key.owner,
                        key.repo,
                        key.endpoint.as_deref().unwrap_or("")
                    ))
                )
            } else {
                base_alias
            };

            Repository {
                repository: alias,
                // MVP inference: imports with an explicit endpoint are backed
                // by a GitHub/GHE service connection; endpoint-less imports are
                // same-org Azure Repos checkouts using System.AccessToken.
                repo_type: if key.endpoint.is_some() {
                    "github".to_string()
                } else {
                    "git".to_string()
                },
                name: format!("{}/{}", key.owner, key.repo),
                // NOTE: ADO repository-resource `ref` does not accept commit
                // SHAs. This branch ref is for resource authorization only;
                // the executor checkout must pin the exact import SHA at
                // runtime with `git checkout <sha>`.
                repo_ref: REPOSITORY_RESOURCE_REF.to_string(),
                endpoint: key.endpoint,
            }
        })
        .collect())
}

fn sanitize_identifier_part(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();

    if sanitized.is_empty() {
        "_".to_string()
    } else {
        sanitized
    }
}

fn short_hash(value: &str) -> String {
    sha256_hex(value.as_bytes())[..HASH_SUFFIX_LEN].to_string()
}

/// Diagnostic (P7): job/stage compile targets are *templates* and cannot emit
/// top-level `resources.repositories`, so the **parent** pipeline must declare
/// and authorize the compiler-generated import repository aliases. Returns a
/// human-readable message listing the alias identifiers the parent must
/// declare, or `None` when the target owns its resources (standalone / 1es) or
/// there are no import aliases.
///
/// The compiler must NOT broaden access automatically — this surfaces the
/// requirement to the pipeline administrator instead.
pub fn import_resource_parent_diagnostic(
    target: CompileTarget,
    aliases: &[String],
) -> Option<String> {
    if aliases.is_empty() {
        return None;
    }
    match target {
        CompileTarget::Job | CompileTarget::Stage => {
            let target_name = match target {
                CompileTarget::Job => "job",
                CompileTarget::Stage => "stage",
                _ => unreachable!(),
            };
            Some(format!(
                "target '{target_name}' is an Azure DevOps template and cannot declare \
                 top-level repository resources; the parent pipeline must define and \
                 authorize these imported component repositories: {}",
                aliases.join(", ")
            ))
        }
        CompileTarget::Standalone | CompileTarget::OneES => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::imports::ImportProvenance;
    use crate::compile::types::{ImportEntry, ParsedImportSpec};
    use crate::secure::CommitSha;

    fn remote_import(
        owner: &str,
        repo: &str,
        path: &str,
        sha: &str,
        endpoint: Option<&str>,
    ) -> ResolvedImport {
        ResolvedImport {
            entry: ImportEntry {
                uses: format!("{owner}/{repo}/{path}@{sha}"),
                with: serde_json::Map::new(),
                endpoint: endpoint.map(str::to_string),
            },
            source: ImportSource::Remote(ParsedImportSpec {
                owner: owner.to_string(),
                repo: repo.to_string(),
                path: path.to_string(),
                sha: CommitSha::parse(sha).expect("test sha should be valid"),
                section: None,
                optional: false,
            }),
            front_matter: serde_yaml::Value::Null,
            body: String::new(),
            provenance: ImportProvenance {
                source: format!("{owner}/{repo}/{path}"),
                sha: Some(sha.to_string()),
                manifest_digest: "digest".to_string(),
            },
        }
    }

    fn local_import(path: &str) -> ResolvedImport {
        ResolvedImport {
            entry: ImportEntry {
                uses: path.to_string(),
                with: serde_json::Map::new(),
                endpoint: None,
            },
            source: ImportSource::Local {
                path: path.to_string(),
                section: None,
                optional: false,
            },
            front_matter: serde_yaml::Value::Null,
            body: String::new(),
            provenance: ImportProvenance {
                source: path.to_string(),
                sha: None,
                manifest_digest: "digest".to_string(),
            },
        }
    }

    fn assert_valid_alias(alias: &str) {
        assert!(!alias.is_empty());
        assert!(!alias.as_bytes()[0].is_ascii_digit());
        assert!(
            alias
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_'),
            "invalid alias: {alias}"
        );
    }

    #[test]
    fn single_remote_import_with_endpoint_synthesizes_github_resource() {
        let imports = vec![remote_import(
            "owner",
            "repo",
            "component.md",
            "0123456789abcdef0123456789abcdef01234567",
            Some("github-service-connection"),
        )];

        let repos = synthesize_repo_aliases(&imports).expect("synthesis should succeed");

        assert_eq!(repos.len(), 1);
        let repo = &repos[0];
        assert_eq!(repo.repo_type, "github");
        assert_eq!(repo.endpoint.as_deref(), Some("github-service-connection"));
        assert_eq!(repo.name, "owner/repo");
        assert_eq!(repo.repo_ref, "refs/heads/main");
        assert_valid_alias(&repo.repository);
    }

    #[test]
    fn remote_import_without_endpoint_synthesizes_git_resource() {
        let imports = vec![remote_import(
            "ado",
            "repo",
            "component.md",
            "1123456789abcdef0123456789abcdef01234567",
            None,
        )];

        let repos = synthesize_repo_aliases(&imports).expect("synthesis should succeed");

        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].repo_type, "git");
        assert_eq!(repos[0].endpoint, None);
        assert_eq!(repos[0].name, "ado/repo");
    }

    #[test]
    fn same_repo_at_different_paths_and_shas_is_deduplicated() {
        let imports = vec![
            remote_import(
                "owner",
                "repo",
                "one.md",
                "2123456789abcdef0123456789abcdef01234567",
                Some("endpoint"),
            ),
            remote_import(
                "owner",
                "repo",
                "two.md",
                "3123456789abcdef0123456789abcdef01234567",
                Some("endpoint"),
            ),
        ];

        let repos = synthesize_repo_aliases(&imports).expect("synthesis should succeed");

        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].name, "owner/repo");
        assert_eq!(repos[0].endpoint.as_deref(), Some("endpoint"));
    }

    #[test]
    fn different_repos_with_naive_alias_collision_get_distinct_aliases() {
        let imports = vec![
            remote_import(
                "a-b",
                "component",
                "tool.md",
                "4123456789abcdef0123456789abcdef01234567",
                None,
            ),
            remote_import(
                "a_b",
                "component",
                "tool.md",
                "5123456789abcdef0123456789abcdef01234567",
                None,
            ),
        ];

        let repos = synthesize_repo_aliases(&imports).expect("synthesis should succeed");

        assert_eq!(repos.len(), 2);
        assert_ne!(repos[0].repository, repos[1].repository);
        assert_valid_alias(&repos[0].repository);
        assert_valid_alias(&repos[1].repository);
    }

    #[test]
    fn local_imports_are_skipped() {
        let imports = vec![local_import("components/local.md")];

        let repos = synthesize_repo_aliases(&imports).expect("synthesis should succeed");

        assert!(repos.is_empty());
    }

    #[test]
    fn alias_identifier_is_stable_and_valid() {
        let alias = alias_identifier("123-owner.with-dots", "repo/name");

        assert_eq!(alias, alias_identifier("123-owner.with-dots", "repo/name"));
        assert_valid_alias(&alias);
        assert!(alias.starts_with("import_"));
    }

    #[test]
    fn parent_diagnostic_emitted_for_template_targets() {
        let aliases = vec!["import_owner_repo_abc".to_string()];
        let job = import_resource_parent_diagnostic(CompileTarget::Job, &aliases)
            .expect("job target should require a parent diagnostic");
        assert!(job.contains("import_owner_repo_abc"));
        assert!(job.contains("parent pipeline"));
        assert!(import_resource_parent_diagnostic(CompileTarget::Stage, &aliases).is_some());
    }

    #[test]
    fn parent_diagnostic_absent_for_owning_targets_or_no_aliases() {
        let aliases = vec!["import_owner_repo_abc".to_string()];
        assert!(import_resource_parent_diagnostic(CompileTarget::Standalone, &aliases).is_none());
        assert!(import_resource_parent_diagnostic(CompileTarget::OneES, &aliases).is_none());
        // No aliases → no diagnostic even for template targets.
        assert!(import_resource_parent_diagnostic(CompileTarget::Job, &[]).is_none());
    }
}
