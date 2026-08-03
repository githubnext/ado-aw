//! Versioned, deny-by-default Azure DevOps read-operation catalog.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const CATALOG_SCHEMA_VERSION: &str = "ado-aw/ado-proxy-catalog/v1";

/// Whether authors can actually reach this catalog through a compiled pipeline.
///
/// This gates the *author-facing* path, not the existence of the runtime: the
/// `ado-proxy` bundle is implemented and tested, but nothing emits its sidecar,
/// policy document, or credential lifecycle yet. It flips only once
/// `compiler-proxy-wiring` lands against a pinned AWF release whose agent image
/// supports the managed policy proxy and CA.
pub const RUNTIME_AVAILABLE: bool = false;

/// Canonical Azure DevOps Services REST host for the current organization.
pub const ORGANIZATION_HOST: &str = "dev.azure.com";

/// Hardcoded deployment-level SPS host used for resource-area fallback
/// discovery. It is organization-agnostic, so its routes carry no `{org}`
/// segment and are scoped by allowed resource-area id instead.
pub const SPS_FALLBACK_HOST: &str = "app.vssps.visualstudio.com";

const JSON_LIMIT: u64 = 10 * 1024 * 1024;
const NO_QUERY: &[&str] = &[];

/// Marker used by every operation that negotiates a normal REST API version.
/// The concrete accepted range lives in [`API_VERSION_MIN`] /
/// [`API_VERSION_MAX`] so the catalog string and the enforcement code cannot
/// drift.
pub const API_VERSION_RANGE: &str = "5.0..=7.2; preview allowed";

/// Marker used by discovery `OPTIONS` operations, which must be sent without
/// any API version at all (in the query string or the `Accept` header).
pub const API_VERSION_ABSENT: &str = "absent";

/// Inclusive lower bound of the accepted `major.minor` API version.
pub const API_VERSION_MIN: (u32, u32) = (5, 0);

/// Inclusive upper bound of the accepted `major.minor` API version.
pub const API_VERSION_MAX: (u32, u32) = (7, 2);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    Get,
    Options,
}

impl HttpMethod {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Options => "OPTIONS",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Capability {
    Discovery,
    Core,
    Repos,
    Pipelines,
    Boards,
}

impl Capability {
    /// Every capability the catalog defines.
    ///
    /// Exhaustive by construction: the `match` below fails to compile if a
    /// variant is added without being listed here, which in turn drives the
    /// front-matter coverage guard in `crate::compile::types`.
    ///
    /// Only the front-matter guard consumes this today; the compiler wiring
    /// that emits the policy document will be its second caller.
    #[allow(dead_code)]
    pub const ALL: &'static [Self] = &[
        Self::Discovery,
        Self::Core,
        Self::Repos,
        Self::Pipelines,
        Self::Boards,
    ];

    /// Whether the proxy enables this capability regardless of author opt-in.
    ///
    /// `discovery` is always on: `az` and the REST SDKs call `resourceareas`
    /// and `connectiondata` before anything else, so a policy without it would
    /// produce a proxy no supported client can actually use. It exposes only
    /// service-topology metadata, never repository, pipeline, or work-item
    /// content, so it is not a meaningful widening.
    #[allow(dead_code)]
    pub const fn is_always_on(self) -> bool {
        match self {
            Self::Discovery => true,
            Self::Core | Self::Repos | Self::Pipelines | Self::Boards => false,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Discovery => "discovery",
            Self::Core => "core",
            Self::Repos => "repos",
            Self::Pipelines => "pipelines",
            Self::Boards => "boards",
        }
    }

    /// The `az` command group this capability makes usable, if any.
    ///
    /// The `az` wrapper's allow-list is derived from this rather than
    /// maintained by hand, so a capability the policy does not grant cannot be
    /// advertised to the agent. Without it the two drift: `az artifacts` was
    /// briefly permitted by the wrapper while no catalogued operation backed
    /// it, so the call passed the wrapper and was refused by the engine.
    ///
    /// `Discovery` maps to nothing: it is the service-topology lookup every
    /// client performs before its first real call, not a command group an
    /// agent invokes.
    pub const fn az_command_group(self) -> Option<&'static str> {
        match self {
            Self::Discovery => None,
            Self::Core => Some("devops"),
            Self::Repos => Some("repos"),
            Self::Pipelines => Some("pipelines"),
            Self::Boards => Some("boards"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum HostPolicy {
    CurrentOrganization,
    SpsFallback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ScopePolicy {
    CurrentOrganization,
    AllowedResourceArea,
    CurrentProjectPath,
    CurrentRepositoryPath,
    FilterProjectsToCurrent,
    FilterResourceAreas,
    ResponseCurrentProject,
    ResponseCurrentRepository,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ResponsePolicy {
    Json,
    FilterProjects,
    FilterResourceAreas,
    ValidateProject,
    ValidateProjectAndRepository,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Operation {
    pub id: &'static str,
    pub capability: Capability,
    pub method: HttpMethod,
    pub host: HostPolicy,
    pub route: &'static str,
    pub api_version: &'static str,
    pub scope: ScopePolicy,
    pub response: ResponsePolicy,
    pub allowed_query: &'static [&'static str],
    pub denied_query: &'static [&'static str],
    pub max_response_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Catalog {
    pub schema_version: &'static str,
    pub runtime_available: bool,
    pub protected_hosts: &'static [&'static str],
    pub operations: Vec<Operation>,
    pub denied_route_families: &'static [&'static str],
    /// Inclusive `[major, minor]` lower bound of the accepted REST API version.
    pub api_version_min: [u32; 2],
    /// Inclusive `[major, minor]` upper bound of the accepted REST API version.
    pub api_version_max: [u32; 2],
}

/// Generate the JSON Schema for the `ado-proxy` catalog.
///
/// This schema is the formal contract between the Rust compiler (which emits
/// the policy document) and the `ado-proxy` TypeScript bundle (which enforces
/// it). `npm run codegen` in `scripts/ado-script` turns it into
/// `src/shared/ado-proxy-catalog.types.gen.ts`, so the bundle cannot compile
/// against a stale shape.
///
/// Mirrors [`crate::compile::filter_ir::generate_gate_spec_schema`].
pub fn generate_catalog_schema() -> String {
    let schema = schemars::schema_for!(Catalog);
    serde_json::to_string_pretty(&schema).expect("catalog schema serialization")
}

/// Generate the catalog **data** as JSON.
///
/// Emitted to a committed `catalog.gen.json` by `npm run codegen` and
/// drift-checked in CI, so any Rust-side change to an operation, scope,
/// response policy, or denial family forces a regeneration rather than
/// silently diverging from what the sidecar enforces.
///
/// Mirrors [`crate::compile::filter_ir::generate_fact_catalog`].
pub fn generate_catalog_json() -> String {
    serde_json::to_string_pretty(&catalog()).expect("catalog serialization")
}

macro_rules! get {
    ($id:literal, $cap:ident, $host:ident, $route:literal, $scope:ident, $response:ident, $query:expr) => {
        Operation {
            id: $id,
            capability: Capability::$cap,
            method: HttpMethod::Get,
            host: HostPolicy::$host,
            route: $route,
            api_version: API_VERSION_RANGE,
            scope: ScopePolicy::$scope,
            response: ResponsePolicy::$response,
            allowed_query: $query,
            denied_query: NO_QUERY,
            max_response_bytes: JSON_LIMIT,
        }
    };
}

pub fn catalog() -> Catalog {
    let (min_major, min_minor) = API_VERSION_MIN;
    let (max_major, max_minor) = API_VERSION_MAX;
    Catalog {
        schema_version: CATALOG_SCHEMA_VERSION,
        runtime_available: RUNTIME_AVAILABLE,
        protected_hosts: &[ORGANIZATION_HOST, SPS_FALLBACK_HOST],
        operations: operations(),
        denied_route_families: DENIED_ROUTE_FAMILIES,
        api_version_min: [min_major, min_minor],
        api_version_max: [max_major, max_minor],
    }
}

pub fn operations() -> Vec<Operation> {
    vec![
        Operation {
            id: "discovery.host-options",
            capability: Capability::Discovery,
            method: HttpMethod::Options,
            host: HostPolicy::CurrentOrganization,
            route: "/{org}/_apis",
            api_version: API_VERSION_ABSENT,
            scope: ScopePolicy::CurrentOrganization,
            response: ResponsePolicy::Json,
            allowed_query: &["allHostTypes"],
            denied_query: NO_QUERY,
            max_response_bytes: JSON_LIMIT,
        },
        Operation {
            id: "discovery.sps-host-options",
            capability: Capability::Discovery,
            method: HttpMethod::Options,
            host: HostPolicy::SpsFallback,
            // No `{org}` segment: SPS is a deployment-level service, so this
            // route is organization-agnostic.
            //
            // Verified against the real Azure CLI: `az repos show` issues
            // `OPTIONS https://app.vssps.visualstudio.com/_apis` before its
            // first data call and fails outright without it. It returns the
            // same resource-location document as the organization-host variant
            // — service topology only, never repository, pipeline, or work-item
            // content — so allowing it does not widen data access.
            route: "/_apis",
            api_version: API_VERSION_ABSENT,
            scope: ScopePolicy::CurrentOrganization,
            response: ResponsePolicy::Json,
            allowed_query: &["allHostTypes"],
            denied_query: NO_QUERY,
            max_response_bytes: JSON_LIMIT,
        },
        Operation {
            id: "discovery.area-options",
            capability: Capability::Discovery,
            method: HttpMethod::Options,
            host: HostPolicy::CurrentOrganization,
            route: "/{org}/_apis/{area}",
            api_version: API_VERSION_ABSENT,
            scope: ScopePolicy::CurrentOrganization,
            response: ResponsePolicy::Json,
            allowed_query: NO_QUERY,
            denied_query: NO_QUERY,
            max_response_bytes: JSON_LIMIT,
        },
        get!(
            "discovery.resource-areas",
            Discovery,
            CurrentOrganization,
            "/{org}/_apis/resourceareas",
            FilterResourceAreas,
            FilterResourceAreas,
            NO_QUERY
        ),
        get!(
            "discovery.sps-resource-area",
            Discovery,
            SpsFallback,
            "/_apis/resourceareas/{areaId}",
            AllowedResourceArea,
            Json,
            NO_QUERY
        ),
        get!(
            "discovery.connection-data",
            Discovery,
            CurrentOrganization,
            "/{org}/_apis/connectiondata",
            CurrentOrganization,
            Json,
            &["connectOptions", "lastChangeId", "lastChangeId64"]
        ),
        get!(
            "core.project-get",
            Core,
            CurrentOrganization,
            "/{org}/_apis/projects/{project}",
            CurrentProjectPath,
            Json,
            &["includeCapabilities", "includeHistory"]
        ),
        get!(
            "core.project-validation-probe",
            Core,
            CurrentOrganization,
            "/{org}/_apis/projects",
            FilterProjectsToCurrent,
            FilterProjects,
            &["stateFilter", "$top", "$skip"]
        ),
        get!(
            "repos.repository-get",
            Repos,
            CurrentOrganization,
            "/{org}/{project}/_apis/git/repositories/{repository}",
            CurrentRepositoryPath,
            Json,
            &["includeParent"]
        ),
        get!(
            "repos.refs-list",
            Repos,
            CurrentOrganization,
            "/{org}/{project}/_apis/git/repositories/{repository}/refs",
            CurrentRepositoryPath,
            Json,
            &[
                "filter",
                "filterContains",
                "includeLinks",
                "includeStatuses",
                "includeMyBranches",
                "latestStatusesOnly",
                "peelTags",
                "$top",
                "continuationToken",
            ]
        ),
        Operation {
            denied_query: &["download", "$format", "zipForUnix"],
            ..get!(
                "repos.items-list",
                Repos,
                CurrentOrganization,
                "/{org}/{project}/_apis/git/repositories/{repository}/items",
                CurrentRepositoryPath,
                Json,
                &[
                    "path",
                    "scopePath",
                    "recursionLevel",
                    "includeContentMetadata",
                    "latestProcessedChange",
                    "includeLinks",
                    "versionDescriptor.version",
                    "versionDescriptor.versionType",
                    "versionDescriptor.versionOptions",
                ]
            )
        },
        get!(
            "repos.commits-list",
            Repos,
            CurrentOrganization,
            "/{org}/{project}/_apis/git/repositories/{repository}/commits",
            CurrentRepositoryPath,
            Json,
            &["$top", "$skip", "searchCriteria"]
        ),
        get!(
            "repos.commit-get",
            Repos,
            CurrentOrganization,
            "/{org}/{project}/_apis/git/repositories/{repository}/commits/{commitId}",
            CurrentRepositoryPath,
            Json,
            &["changeCount"]
        ),
        get!(
            "repos.commit-changes",
            Repos,
            CurrentOrganization,
            "/{org}/{project}/_apis/git/repositories/{repository}/commits/{commitId}/changes",
            CurrentRepositoryPath,
            Json,
            &["top", "skip"]
        ),
        get!(
            "repos.pull-requests-list",
            Repos,
            CurrentOrganization,
            "/{org}/{project}/_apis/git/repositories/{repository}/pullrequests",
            CurrentRepositoryPath,
            Json,
            &["searchCriteria", "$top", "$skip", "maxCommentLength"]
        ),
        get!(
            "repos.pull-request-get",
            Repos,
            CurrentOrganization,
            "/{org}/{project}/_apis/git/repositories/{repository}/pullrequests/{pullRequestId}",
            CurrentRepositoryPath,
            Json,
            &[
                "maxCommentLength",
                "$top",
                "$skip",
                "includeCommits",
                "includeWorkItemRefs",
            ]
        ),
        get!(
            "repos.pull-request-get-by-id",
            Repos,
            CurrentOrganization,
            "/{org}/_apis/git/pullrequests/{pullRequestId}",
            ResponseCurrentRepository,
            ValidateProjectAndRepository,
            &["maxCommentLength", "includeCommits", "includeWorkItemRefs"]
        ),
        get!(
            "repos.pull-request-threads",
            Repos,
            CurrentOrganization,
            "/{org}/{project}/_apis/git/repositories/{repository}/pullrequests/{pullRequestId}/threads",
            CurrentRepositoryPath,
            Json,
            &["$top", "$skip", "iteration", "baseIteration"]
        ),
        get!(
            "repos.pull-request-iterations",
            Repos,
            CurrentOrganization,
            "/{org}/{project}/_apis/git/repositories/{repository}/pullrequests/{pullRequestId}/iterations",
            CurrentRepositoryPath,
            Json,
            &["includeCommits"]
        ),
        get!(
            "repos.pull-request-reviewers",
            Repos,
            CurrentOrganization,
            "/{org}/{project}/_apis/git/repositories/{repository}/pullrequests/{pullRequestId}/reviewers",
            CurrentRepositoryPath,
            Json,
            NO_QUERY
        ),
        get!(
            "repos.pull-request-work-items",
            Repos,
            CurrentOrganization,
            "/{org}/{project}/_apis/git/repositories/{repository}/pullrequests/{pullRequestId}/workitems",
            CurrentRepositoryPath,
            Json,
            NO_QUERY
        ),
        get!(
            "pipelines.definitions-list",
            Pipelines,
            CurrentOrganization,
            "/{org}/{project}/_apis/build/definitions",
            CurrentProjectPath,
            Json,
            &[
                "name",
                "repositoryId",
                "repositoryType",
                "$top",
                "continuationToken",
                "path"
            ]
        ),
        get!(
            "pipelines.definition-get",
            Pipelines,
            CurrentOrganization,
            "/{org}/{project}/_apis/build/definitions/{definitionId}",
            CurrentProjectPath,
            Json,
            &["revision", "propertyFilters", "includeLatestBuilds"]
        ),
        get!(
            "pipelines.builds-list",
            Pipelines,
            CurrentOrganization,
            "/{org}/{project}/_apis/build/builds",
            CurrentProjectPath,
            Json,
            &[
                "definitions",
                "buildNumber",
                "minTime",
                "maxTime",
                "reasonFilter",
                "statusFilter",
                "resultFilter",
                "$top",
                "continuationToken",
                "queryOrder",
                "branchName",
                "buildIds",
                "repositoryId",
                "repositoryType",
            ]
        ),
        get!(
            "pipelines.build-get",
            Pipelines,
            CurrentOrganization,
            "/{org}/{project}/_apis/build/builds/{buildId}",
            CurrentProjectPath,
            Json,
            &["propertyFilters"]
        ),
        get!(
            "pipelines.timeline-get",
            Pipelines,
            CurrentOrganization,
            "/{org}/{project}/_apis/build/builds/{buildId}/timeline",
            CurrentProjectPath,
            Json,
            &["changeId", "planId"]
        ),
        get!(
            "pipelines.pipeline-list",
            Pipelines,
            CurrentOrganization,
            "/{org}/{project}/_apis/pipelines",
            CurrentProjectPath,
            Json,
            &["orderBy", "$top", "continuationToken"]
        ),
        get!(
            "pipelines.pipeline-get",
            Pipelines,
            CurrentOrganization,
            "/{org}/{project}/_apis/pipelines/{pipelineId}",
            CurrentProjectPath,
            Json,
            &["pipelineVersion"]
        ),
        get!(
            "pipelines.runs-list",
            Pipelines,
            CurrentOrganization,
            "/{org}/{project}/_apis/pipelines/{pipelineId}/runs",
            CurrentProjectPath,
            Json,
            NO_QUERY
        ),
        get!(
            "pipelines.run-get",
            Pipelines,
            CurrentOrganization,
            "/{org}/{project}/_apis/pipelines/{pipelineId}/runs/{runId}",
            CurrentProjectPath,
            Json,
            NO_QUERY
        ),
        get!(
            "boards.work-item-get",
            Boards,
            CurrentOrganization,
            "/{org}/{project}/_apis/wit/workitems/{id}",
            CurrentProjectPath,
            Json,
            &["fields", "asOf", "$expand"]
        ),
        get!(
            "boards.work-item-get-by-id",
            Boards,
            CurrentOrganization,
            "/{org}/_apis/wit/workitems/{id}",
            ResponseCurrentProject,
            ValidateProject,
            &["fields", "asOf", "$expand"]
        ),
        get!(
            "boards.work-item-comments",
            Boards,
            CurrentOrganization,
            "/{org}/{project}/_apis/wit/workitems/{id}/comments",
            CurrentProjectPath,
            Json,
            &[
                "$top",
                "continuationToken",
                "includeDeleted",
                "expand",
                "order"
            ]
        ),
        get!(
            "boards.work-item-updates",
            Boards,
            CurrentOrganization,
            "/{org}/{project}/_apis/wit/workitems/{id}/updates",
            CurrentProjectPath,
            Json,
            &["$top", "$skip"]
        ),
        get!(
            "boards.work-item-revisions",
            Boards,
            CurrentOrganization,
            "/{org}/{project}/_apis/wit/workitems/{id}/revisions",
            CurrentProjectPath,
            Json,
            &["$top", "$skip", "$expand"]
        ),
    ]
}

pub const DENIED_ROUTE_FAMILIES: &[&str] = &[
    "/_apis/accesscontrollists",
    "/_apis/accesscontrolentries",
    "/_apis/securitynamespaces",
    "/_apis/permissions",
    "/_apis/tokens",
    "/_apis/tokenadmin",
    "/_apis/delegatedauth",
    "/_apis/oauth2",
    "/_apis/serviceendpoint",
    "/_apis/distributedtask/variablegroups",
    "/_apis/distributedtask/securefiles",
    "/_apis/build/builds/{buildId}/oauthtoken",
    "/_apis/build/builds/{buildId}/artifacts",
    "/_apis/build/builds/{buildId}/logs",
    "/_apis/build/builds/{buildId}/attachments",
    "/_apis/wit/wiql",
    "/_apis/wit/workitemsbatch",
    "/_apis/wit/workitems?ids=",
    "/_apis/git/repositories/{repository}/itemsbatch",
    "/_apis/git/repositories/{repository}/commitsbatch",
    "/_apis/git/repositories/{repository}/blobs",
    "/_apis/git/repositories/{repository}/trees",
    "/_git/",
    "/_odata/",
    "/_apis/search/",
    "/_apis/customerintelligence/events",
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn catalog_is_versioned_and_runtime_stays_disabled() {
        let catalog = catalog();
        assert_eq!(catalog.schema_version, CATALOG_SCHEMA_VERSION);
        assert!(!catalog.runtime_available);
        assert!(!catalog.operations.is_empty());
    }

    #[test]
    fn operation_ids_are_unique_and_routes_are_normalized() {
        let mut ids = HashSet::new();
        for operation in operations() {
            assert!(ids.insert(operation.id), "duplicate id {}", operation.id);
            assert!(operation.route.starts_with('/'));
            let mut in_parameter = false;
            for character in operation.route.chars() {
                match character {
                    '{' => in_parameter = true,
                    '}' => in_parameter = false,
                    _ if !in_parameter => {
                        assert!(
                            !character.is_ascii_uppercase(),
                            "fixed route segments must be lowercase: {}",
                            operation.route
                        );
                    }
                    _ => {}
                }
            }
            assert!(matches!(
                operation.method,
                HttpMethod::Get | HttpMethod::Options
            ));
        }
    }

    #[test]
    fn discovery_and_response_scoped_operations_are_explicit() {
        let entries = operations();
        assert!(entries.iter().any(|entry| {
            entry.id == "discovery.host-options"
                && entry.method == HttpMethod::Options
                && entry.api_version == API_VERSION_ABSENT
        }));
        assert!(entries.iter().any(|entry| {
            entry.id == "repos.pull-request-get-by-id"
                && entry.response == ResponsePolicy::ValidateProjectAndRepository
        }));
        assert!(entries.iter().any(|entry| {
            entry.id == "boards.work-item-get-by-id"
                && entry.response == ResponsePolicy::ValidateProject
        }));
    }

    #[test]
    fn protected_hosts_exclude_package_and_token_services() {
        let hosts = catalog().protected_hosts;
        for denied in [
            "pkgs.dev.azure.com",
            "artifacts.dev.azure.com",
            "vstoken.dev.azure.com",
            "vssps.dev.azure.com",
        ] {
            assert!(!hosts.contains(&denied));
        }
    }

    #[test]
    fn known_sensitive_families_are_default_denied() {
        for required in [
            "/_apis/serviceendpoint",
            "/_apis/distributedtask/variablegroups",
            "/_apis/distributedtask/securefiles",
            "/_apis/build/builds/{buildId}/oauthtoken",
            "/_git/",
        ] {
            assert!(DENIED_ROUTE_FAMILIES.contains(&required));
        }
    }

    /// The human-readable `API_VERSION_RANGE` marker and the machine-readable
    /// bounds are both exported to the sidecar. If they ever disagree, the
    /// bundle would enforce a different window than the catalog advertises.
    #[test]
    fn api_version_marker_matches_the_exported_bounds() {
        let (min_major, min_minor) = API_VERSION_MIN;
        let (max_major, max_minor) = API_VERSION_MAX;
        assert!(
            API_VERSION_RANGE.starts_with(&format!("{min_major}.{min_minor}")),
            "range marker must start at API_VERSION_MIN: {API_VERSION_RANGE}"
        );
        assert!(
            API_VERSION_RANGE.contains(&format!("{max_major}.{max_minor}")),
            "range marker must name API_VERSION_MAX: {API_VERSION_RANGE}"
        );
        assert!(
            (min_major, min_minor) < (max_major, max_minor),
            "API version bounds must be ordered"
        );
    }
}
