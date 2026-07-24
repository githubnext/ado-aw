//! Compiler-internal repository-resource identifiers for imported custom
//! safe-output components (for the runtime executor-job checkout), plus the
//! typed-endpoint → ADO repo-type/service-connection mapping and the P7
//! parent-resource diagnostic for template targets.
//!
//! The compiler owns these aliases: authors write only `imports:` entries, and
//! the per-component provenance stamped during the merge pass carries the
//! `owner/repo` that [`alias_identifier`] turns into a stable, valid ADO
//! identifier: `import_<sanitized-owner>_<sanitized-repo>_<hash>`. The readable
//! parts replace non-ASCII-identifier characters with `_`; the fixed hash suffix
//! is derived from the original `owner/repo`, so repos that collide under simple
//! sanitization (for example `a-b/c` and `a_b/c`) still get distinct aliases.

use crate::compile::types::{CompileTarget, ImportEndpoint};
use crate::hash::sha256_hex;

const HASH_SUFFIX_LEN: usize = 12;

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

/// Map a typed import [`ImportEndpoint`] to the ADO repository-resource `type`
/// and the backing service-connection name (`None` for same-org Azure Repos).
///
/// Single source of truth for the compile-time component-provenance stamping in
/// [`crate::compile::imports::merge`], so the runtime component checkout uses
/// the correct repo type + service connection for GitHub / GitHub Enterprise /
/// cross-org Azure Repos components (not a hardcoded `git` / no-endpoint).
pub(crate) fn endpoint_repo_type_and_connection(
    endpoint: Option<&ImportEndpoint>,
) -> (&'static str, Option<String>) {
    match endpoint {
        None => ("git", None),
        Some(ImportEndpoint::AzureReposCrossOrg { name, .. }) => ("git", Some(name.clone())),
        Some(ImportEndpoint::GitHub { name }) => ("github", Some(name.clone())),
        Some(ImportEndpoint::GitHubEnterprise { name, .. }) => {
            ("githubenterprise", Some(name.clone()))
        }
    }
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
