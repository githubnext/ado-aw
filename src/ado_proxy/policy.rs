//! Policy document emitted by the compiler and consumed by the `ado-proxy`
//! bundle at startup.
//!
//! The bundle refuses to start on a document it does not fully understand: an
//! unknown key, a `catalog_version` that is not this bundle's, or a catalogued
//! protected host that the document omits are all fatal. That is deliberate —
//! each of those would silently *under*-enforce. Emitting the document from
//! the same Rust module that owns the catalog is what keeps the two in step.
//!
//! Scope values are left as placeholders rather than baked in at compile time.
//! The organization and project are properties of the *run*, not of the
//! workflow source, and a compiled pipeline is routinely queued against a
//! different project than the one it was compiled in. Substituting them at
//! step time (the same `sed` pattern MCPG already uses) keeps the emitted YAML
//! portable and prevents a stale scope from silently widening access.

use serde::Serialize;

use super::catalog::{
    CATALOG_SCHEMA_VERSION, Capability, ORGANIZATION_HOST, SPS_FALLBACK_HOST,
};
use crate::compile::types::FrontMatter;

/// Resolve the capabilities the policy engine should enable.
///
/// An author who names `capabilities:` gets exactly those, plus the always-on
/// ones. Omitting the key selects the whole catalog — deliberately broad
/// *within* a narrow boundary: every catalogued operation is a `GET` or
/// `OPTIONS`, and the always-denied route families exclude ACLs, tokens,
/// service endpoints, variable groups and secure files. Starting narrower
/// would leave the Azure DevOps MCP unable to answer most questions, which
/// pushes authors back towards handing agents raw credentials — the outcome
/// this design exists to prevent.
///
/// The result is always in [`Capability::ALL`] order, so reordering a
/// `capabilities:` list cannot change the compiled pipeline.
pub fn ado_proxy_capabilities(front_matter: &FrontMatter) -> Vec<Capability> {
    let requested: Option<Vec<Capability>> = front_matter
        .permissions
        .as_ref()
        .and_then(|permissions| permissions.read.as_ref())
        .and_then(crate::compile::types::ReadPermissionConfig::options)
        .filter(|options| !options.capabilities.is_empty())
        .map(|options| {
            options
                .capabilities
                .iter()
                .map(|capability| capability.to_catalog())
                .collect()
        });

    Capability::ALL
        .iter()
        .copied()
        .filter(|capability| match &requested {
            // `discovery` is always on: every client resolves resource areas
            // before its first real call, so a policy without it produces a
            // proxy no supported client can actually use.
            Some(selected) => capability.is_always_on() || selected.contains(capability),
            None => true,
        })
        .collect()
}

/// Placeholder substituted with the organization name at step time.
pub const ORGANIZATION_PLACEHOLDER: &str = "${ADO_PROXY_ORGANIZATION}";

/// Placeholder substituted with the project name at step time.
pub const PROJECT_PLACEHOLDER: &str = "${ADO_PROXY_PROJECT}";

/// Placeholder substituted with the project GUID at step time.
///
/// Clients address the current project by name in some calls and by GUID in
/// others — `az` substitutes whichever it cached — so both forms must be
/// present or a GUID-addressed request is denied.
pub const PROJECT_ID_PLACEHOLDER: &str = "${ADO_PROXY_PROJECT_ID}";

/// Placeholder substituted with the current repository name at step time.
pub const REPOSITORY_PLACEHOLDER: &str = "${ADO_PROXY_REPOSITORY}";

/// Placeholder substituted with the current repository GUID at step time.
pub const REPOSITORY_ID_PLACEHOLDER: &str = "${ADO_PROXY_REPOSITORY_ID}";

/// The policy document handed to the `ado-proxy` bundle via `--policy-file`.
///
/// Field names and shape are a contract with `parsePolicy` in
/// `scripts/ado-script/src/ado-proxy/config.ts`; the bundle rejects any key it
/// does not recognize, so adding a field here without adding it there is a
/// startup failure rather than a silent mismatch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PolicyDocument {
    pub catalog_version: &'static str,
    pub organization: String,
    pub project: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository_id: Option<String>,
    /// Scopes beyond the pipeline's own organization/project/repository.
    ///
    /// Empty is emitted rather than omitted so the compiler makes an explicit
    /// statement that there are no additions. The bundle defaults an absent
    /// value to empty for compatibility with older policies.
    pub additional_scopes: Vec<PolicyOrganizationScope>,
    pub capabilities: Vec<&'static str>,
    pub protected_hosts: Vec<&'static str>,
}

/// One explicitly allowed organization and its projects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PolicyOrganizationScope {
    pub organization: String,
    pub projects: Vec<PolicyProjectScope>,
}

/// A project grant within one organization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PolicyProjectScope {
    pub project: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// True because `permissions.read.allow` names this project deliberately.
    ///
    /// A later `repos:`-derived grant uses false so declaring a repository does
    /// not unlock the work items, builds and pipelines beside it.
    pub project_scoped: bool,
    pub repositories: Vec<String>,
}

impl PolicyDocument {
    /// Build the document from the compiler's own configuration.
    ///
    /// Taking [`FrontMatter`] rather than a capability slice is deliberate. A
    /// constructor narrower than the configuration cannot express it, so every
    /// input it cannot see becomes a silent default — and because the bundle
    /// treats an absent field as "match nothing", each of those defaults is an
    /// invisible *denial* rather than a loud error. That is how the current
    /// repository came to be unreachable: `repository` was hard-coded `None`,
    /// omitted from the JSON, and twelve catalogued operations denied
    /// unconditionally without a single test noticing.
    ///
    /// Adding a field to this struct should therefore force a decision about
    /// which piece of configuration populates it.
    ///
    /// Always-on capabilities are added regardless of what the author asked
    /// for, and the result is emitted in [`Capability::ALL`] order so the
    /// document is stable no matter how the front matter was written — an
    /// author reordering their `capabilities:` list must not produce a
    /// different pipeline.
    pub fn new(front_matter: &FrontMatter) -> Self {
        let requested = ado_proxy_capabilities(front_matter);
        let capabilities = requested
            .iter()
            .map(|capability| capability.as_str())
            .collect();

        Self {
            catalog_version: CATALOG_SCHEMA_VERSION,
            organization: ORGANIZATION_PLACEHOLDER.to_string(),
            project: PROJECT_PLACEHOLDER.to_string(),
            // Substituted at step time like the organization and project. A
            // compiled pipeline is routinely queued against a different
            // project than the one it was compiled in, so baking these in
            // would make a lock file wrong the moment it moved.
            project_id: Some(PROJECT_ID_PLACEHOLDER.to_string()),
            repository: Some(REPOSITORY_PLACEHOLDER.to_string()),
            repository_id: Some(REPOSITORY_ID_PLACEHOLDER.to_string()),
            additional_scopes: Self::explicit_additional_scopes(front_matter),
            capabilities,
            // Every catalogued host must appear: one the bundle policed but
            // the document omitted would be byte-tunnelled to Squid instead,
            // which is the single failure mode the proxy cannot tolerate.
            protected_hosts: vec![ORGANIZATION_HOST, SPS_FALLBACK_HOST],
        }
    }

    /// Lower `permissions.read.allow` into the exact organization-relative tree
    /// the bundle validates.
    ///
    /// The nesting is preserved rather than flattened: a project granted in
    /// organization A must never match the same project name in organization B.
    fn explicit_additional_scopes(front_matter: &FrontMatter) -> Vec<PolicyOrganizationScope> {
        let options = front_matter
            .permissions
            .as_ref()
            .and_then(|permissions| permissions.read.as_ref())
            .and_then(crate::compile::types::ReadPermissionConfig::options);

        options
            .into_iter()
            .flat_map(|options| &options.allow)
            .map(|scope| PolicyOrganizationScope {
                organization: scope.organization.as_str().to_string(),
                projects: scope
                    .projects
                    .iter()
                    .map(|project| PolicyProjectScope {
                        project: project.project.as_str().to_string(),
                        project_id: project
                            .project_id
                            .as_ref()
                            .map(|value| value.as_str().to_string()),
                        project_scoped: true,
                        repositories: project
                            .repositories
                            .iter()
                            .map(|repository| repository.as_str().to_string())
                            .collect(),
                    })
                    .collect(),
            })
            .collect()
    }

    /// Render as the JSON the bundle reads from `--policy-file`.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self)
            .expect("PolicyDocument is a plain serializable struct")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Front matter with no explicit read policy — the common case.
    fn plain() -> FrontMatter {
        crate::compile::parse_markdown("---\nname: t\ndescription: x\n---\n")
            .unwrap()
            .0
    }

    /// Front matter naming an explicit capability set.
    fn with_capabilities(list: &str) -> FrontMatter {
        crate::compile::parse_markdown(&format!(
            "---\nname: t\ndescription: x\npermissions:\n  read:\n    \
             service-connection: my-read-sc\n    capabilities: [{list}]\n---\n"
        ))
        .unwrap()
        .0
    }

    fn with_additional_scope() -> FrontMatter {
        crate::compile::parse_markdown(
            r#"---
name: t
description: x
permissions:
  read:
    service-connection: my-read-sc
    capabilities: [core, repos]
    allow:
      - organization: fabrikam
        projects:
          - project: Shared
            project-id: 33333333-3333-3333-3333-333333333333
            repositories: [shared-api]
---
"#,
        )
        .unwrap()
        .0
    }

    #[test]
    fn discovery_is_present_even_when_unrequested() {
        let document = PolicyDocument::new(&with_capabilities("repos"));
        assert!(
            document.capabilities.contains(&"discovery"),
            "discovery is always on; without it no supported client can \
             complete its initial resource-area lookup: {:?}",
            document.capabilities
        );
    }

    #[test]
    fn capability_order_is_independent_of_request_order() {
        let one = PolicyDocument::new(&with_capabilities("boards, core"));
        let two = PolicyDocument::new(&with_capabilities("core, boards"));
        assert_eq!(
            one.capabilities, two.capabilities,
            "author-visible ordering must not change the compiled pipeline"
        );
    }

    #[test]
    fn unrequested_capabilities_are_absent() {
        let document = PolicyDocument::new(&with_capabilities("repos"));
        for capability in Capability::ALL {
            if capability.is_always_on() || *capability == Capability::Repos {
                continue;
            }
            assert!(
                !document.capabilities.contains(&capability.as_str()),
                "{} was never requested and must not be granted: {:?}",
                capability.as_str(),
                document.capabilities
            );
        }
    }

    #[test]
    fn omitting_capabilities_selects_the_whole_catalog() {
        // Deliberately broad within a narrow boundary: every catalogued
        // operation is a GET or OPTIONS, and secret-bearing route families are
        // denied outright. Starting narrower would leave the MCP unable to
        // answer most questions.
        let document = PolicyDocument::new(&plain());
        for capability in Capability::ALL {
            assert!(
                document.capabilities.contains(&capability.as_str()),
                "{} must be granted when the author names none",
                capability.as_str()
            );
        }
    }

    #[test]
    fn every_catalogued_protected_host_is_declared() {
        let document = PolicyDocument::new(&plain());
        for host in super::super::catalog::catalog().protected_hosts {
            assert!(
                document.protected_hosts.contains(host),
                "{host} is catalogued as protected but absent from the policy; \
                 the bundle would byte-tunnel it to Squid unpoliced"
            );
        }
    }

    #[test]
    fn explicit_allow_scopes_preserve_the_organization_project_tree() {
        let document = PolicyDocument::new(&with_additional_scope());

        assert_eq!(
            document.additional_scopes,
            vec![PolicyOrganizationScope {
                organization: "fabrikam".to_string(),
                projects: vec![PolicyProjectScope {
                    project: "Shared".to_string(),
                    project_id: Some(
                        "33333333-3333-3333-3333-333333333333".to_string()
                    ),
                    project_scoped: true,
                    repositories: vec!["shared-api".to_string()],
                }],
            }]
        );
    }

    #[test]
    fn additional_scopes_are_emitted_even_when_empty() {
        // Explicit `[]` is the compiler stating there are no additions. The
        // bundle also accepts absent for compatibility with old policies, but
        // the current compiler should never leave this field undecided.
        let json = PolicyDocument::new(&plain()).to_json();
        assert!(
            json.contains("\"additional_scopes\": []"),
            "additional_scopes must be explicit: {json}"
        );
    }

    #[test]
    fn catalog_version_matches_the_catalog() {
        let document = PolicyDocument::new(&plain());
        assert_eq!(
            document.catalog_version,
            catalog_version_from_catalog(),
            "a policy naming a different catalog version is refused at startup"
        );
    }

    fn catalog_version_from_catalog() -> &'static str {
        super::super::catalog::catalog().schema_version
    }

    #[test]
    fn every_current_scope_identifier_is_emitted() {
        // The regression that motivated taking the config: `repository` was
        // hard-coded `None` and omitted from the JSON, so the bundle — which
        // treats absent as "match nothing" — denied all twelve catalogued
        // repository operations without a single test noticing.
        let json = PolicyDocument::new(&plain()).to_json();
        for field in [
            "organization",
            "project",
            "project_id",
            "repository",
            "repository_id",
        ] {
            assert!(
                json.contains(&format!("\"{field}\"")),
                "{field} is absent, which the bundle reads as match-nothing: {json}"
            );
        }
        assert!(
            !json.contains("null"),
            "a present-but-null field is not the same as an absent one: {json}"
        );
    }

    #[test]
    fn scope_is_left_as_placeholders_for_step_time_substitution() {
        // A compiled pipeline is routinely queued against a different project
        // than it was compiled in, so baking any of these in would make a lock
        // file wrong the moment it moved.
        let document = PolicyDocument::new(&plain());
        assert_eq!(document.organization, ORGANIZATION_PLACEHOLDER);
        assert_eq!(document.project, PROJECT_PLACEHOLDER);
        assert_eq!(document.project_id.as_deref(), Some(PROJECT_ID_PLACEHOLDER));
        assert_eq!(document.repository.as_deref(), Some(REPOSITORY_PLACEHOLDER));
        assert_eq!(
            document.repository_id.as_deref(),
            Some(REPOSITORY_ID_PLACEHOLDER)
        );
    }
}
