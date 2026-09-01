/**
 * Scope resolution: which organizations, projects and repositories the agent
 * may read.
 *
 * The policy carries a *current* scope — the organization, project and
 * repository the pipeline itself runs in, substituted at step time — plus any
 * additional scopes the author declared. Rather than testing "current OR
 * additional" at each of the five call sites that need it, both are folded
 * into one lookup here at startup. Two code paths would drift; one cannot.
 *
 * ## Resolution is organization-relative
 *
 * The trap this module exists to avoid: asking "is this project in *any*
 * allowed list" would let a request addressed to organization B name a project
 * that is only allowed in organization A. Every lookup therefore resolves the
 * organization first and tests the project *within that organization's* entry,
 * and likewise for repositories within a project.
 *
 * ## Project scope is not implied by repository scope
 *
 * A scope derived from a `repos:` declaration grants the repository without
 * granting the project: an author who declared a repository asked for a
 * repository, not for the work items, pipelines and builds that happen to live
 * beside it. `projectScoped` records that distinction, mirroring the rule the
 * front matter already has, where a project entry with no `repositories:`
 * grants project-scoped reads without any repository-scoped read.
 */
import type { ProxyPolicy, PolicyProjectScope } from "./config.js";

/** Case-insensitive identifier comparison, as Azure DevOps treats names. */
function normalize(value: string | undefined): string | undefined {
  return value === undefined || value.trim() === "" ? undefined : value.trim().toLowerCase();
}

/** One project's grant within a single organization. */
interface ProjectGrant {
  /**
   * Whether project-addressed reads (work items, pipelines, builds) are
   * allowed. False for scopes derived from `repos:`, which grant only the
   * repositories they name.
   */
  readonly projectScoped: boolean;
  /** Repository names and ids, lowercased. */
  readonly repositories: ReadonlySet<string>;
}

/** Resolved, organization-relative view of everything the policy permits. */
export class ScopeIndex {
  /** organization → (project name or id) → grant. */
  private readonly byOrganization: Map<string, Map<string, ProjectGrant>>;

  private constructor(byOrganization: Map<string, Map<string, ProjectGrant>>) {
    this.byOrganization = byOrganization;
  }

  /**
   * Fold the current scope and every additional scope into one index.
   *
   * The current scope is seeded first so it cannot be omitted by a malformed
   * `additional_scopes`, and a later entry naming the same project can only
   * widen it — never revoke `projectScoped`.
   */
  static from(policy: ProxyPolicy): ScopeIndex {
    const index = new Map<string, Map<string, ProjectGrant>>();

    const add = (
      organization: string | undefined,
      project: string | undefined,
      projectId: string | undefined,
      projectScoped: boolean,
      repositories: readonly string[],
    ): void => {
      const organizationKey = normalize(organization);
      if (organizationKey === undefined) return;
      const projects = index.get(organizationKey) ?? new Map<string, ProjectGrant>();
      index.set(organizationKey, projects);

      const repositoryKeys = new Set(
        repositories.map(normalize).filter((value): value is string => value !== undefined),
      );

      // A project may be addressed by name or by GUID; both keys point at the
      // same grant so a client using either form resolves identically.
      for (const key of [normalize(project), normalize(projectId)]) {
        if (key === undefined) continue;
        const existing = projects.get(key);
        projects.set(key, {
          projectScoped: projectScoped || (existing?.projectScoped ?? false),
          repositories: new Set([...(existing?.repositories ?? []), ...repositoryKeys]),
        });
      }
    };

    // The pipeline's own organization, project and repository. Always granted:
    // the agent is already running there, with the repository checked out.
    add(
      policy.organization,
      policy.project,
      policy.project_id,
      true,
      [policy.repository, policy.repository_id].filter(
        (value): value is string => value !== undefined,
      ),
    );

    for (const scope of policy.additional_scopes ?? []) {
      for (const project of scope.projects) {
        add(
          scope.organization,
          project.project,
          project.project_id,
          project.project_scoped ?? true,
          project.repositories ?? [],
        );
      }
    }

    return new ScopeIndex(index);
  }

  /** Whether any scope exists in this organization. */
  hasOrganization(organization: string | undefined): boolean {
    const key = normalize(organization);
    return key !== undefined && this.byOrganization.has(key);
  }

  /**
   * Whether project-addressed reads are allowed for this organization/project
   * pair.
   *
   * Organization-relative by construction: the project is looked up inside the
   * organization's own map, so a project allowed elsewhere does not match here.
   */
  allowsProject(organization: string | undefined, project: string | undefined): boolean {
    return this.grant(organization, project)?.projectScoped === true;
  }

  /** Whether a repository-addressed read is allowed within this project. */
  allowsRepository(
    organization: string | undefined,
    project: string | undefined,
    repository: string | undefined,
  ): boolean {
    const key = normalize(repository);
    if (key === undefined) return false;
    return this.grant(organization, project)?.repositories.has(key) === true;
  }

  private grant(
    organization: string | undefined,
    project: string | undefined,
  ): ProjectGrant | undefined {
    const organizationKey = normalize(organization);
    const projectKey = normalize(project);
    if (organizationKey === undefined || projectKey === undefined) return undefined;
    return this.byOrganization.get(organizationKey)?.get(projectKey);
  }
}

/** Normalize a policy project entry, applying the documented defaults. */
export function projectScopeDefaults(scope: PolicyProjectScope): PolicyProjectScope {
  return {
    project: scope.project,
    project_id: scope.project_id,
    // Absent means the author named a project deliberately, which grants
    // project reads. Only a `repos:`-derived scope sets it false.
    project_scoped: scope.project_scoped ?? true,
    repositories: scope.repositories ?? [],
  };
}
