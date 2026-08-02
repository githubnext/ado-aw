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

/// Placeholder substituted with the organization name at step time.
pub const ORGANIZATION_PLACEHOLDER: &str = "${ADO_PROXY_ORGANIZATION}";

/// Placeholder substituted with the project name at step time.
pub const PROJECT_PLACEHOLDER: &str = "${ADO_PROXY_PROJECT}";

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
    pub capabilities: Vec<&'static str>,
    pub protected_hosts: Vec<&'static str>,
}

impl PolicyDocument {
    /// Build the document for a set of author-requested capabilities.
    ///
    /// Always-on capabilities are added regardless of what the author asked
    /// for, and the result is emitted in [`Capability::ALL`] order so the
    /// document is stable no matter how the front matter was written — an
    /// author reordering their `capabilities:` list must not produce a
    /// different pipeline.
    pub fn new(requested: &[Capability]) -> Self {
        let capabilities = Capability::ALL
            .iter()
            .filter(|capability| {
                capability.is_always_on() || requested.contains(capability)
            })
            .map(|capability| capability.as_str())
            .collect();

        Self {
            catalog_version: CATALOG_SCHEMA_VERSION,
            organization: ORGANIZATION_PLACEHOLDER.to_string(),
            project: PROJECT_PLACEHOLDER.to_string(),
            project_id: None,
            repository: None,
            repository_id: None,
            capabilities,
            // Every catalogued host must appear: one the bundle policed but
            // the document omitted would be byte-tunnelled to Squid instead,
            // which is the single failure mode the proxy cannot tolerate.
            protected_hosts: vec![ORGANIZATION_HOST, SPS_FALLBACK_HOST],
        }
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

    #[test]
    fn discovery_is_present_even_when_unrequested() {
        let document = PolicyDocument::new(&[Capability::Repos]);
        assert!(
            document.capabilities.contains(&"discovery"),
            "discovery is always on; without it no supported client can \
             complete its initial resource-area lookup: {:?}",
            document.capabilities
        );
    }

    #[test]
    fn capability_order_is_independent_of_request_order() {
        let one = PolicyDocument::new(&[Capability::Boards, Capability::Core]);
        let two = PolicyDocument::new(&[Capability::Core, Capability::Boards]);
        assert_eq!(
            one.capabilities, two.capabilities,
            "author-visible ordering must not change the compiled pipeline"
        );
    }

    #[test]
    fn unrequested_capabilities_are_absent() {
        let document = PolicyDocument::new(&[]);
        for capability in Capability::ALL {
            if capability.is_always_on() {
                continue;
            }
            assert!(
                !document.capabilities.contains(&capability.as_str()),
                "{} was never requested and must not be granted",
                capability.as_str()
            );
        }
    }

    #[test]
    fn every_catalogued_protected_host_is_declared() {
        let document = PolicyDocument::new(&[]);
        for host in super::super::catalog::catalog().protected_hosts {
            assert!(
                document.protected_hosts.contains(host),
                "{host} is catalogued as protected but absent from the policy; \
                 the bundle would byte-tunnel it to Squid unpoliced"
            );
        }
    }

    #[test]
    fn catalog_version_matches_the_catalog() {
        let document = PolicyDocument::new(&[]);
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
    fn scope_is_left_as_placeholders_for_step_time_substitution() {
        let document = PolicyDocument::new(&[]);
        assert_eq!(document.organization, ORGANIZATION_PLACEHOLDER);
        assert_eq!(document.project, PROJECT_PLACEHOLDER);
    }

    #[test]
    fn json_omits_unset_optional_scope_fields() {
        // The bundle rejects unknown keys, and treats a present-but-null
        // narrowing field differently from an absent one. Emitting `null`
        // would be a startup failure.
        let json = PolicyDocument::new(&[]).to_json();
        assert!(!json.contains("null"), "unset scope fields must be omitted: {json}");
        assert!(!json.contains("project_id"));
        assert!(!json.contains("repository"));
    }
}
