/**
 * Glob matching helpers for branch refs and changed-file paths.
 *
 * Both branches and paths in front-matter use the same ADO/git-style
 * glob syntax: `*` matches one path segment, `**` matches any number,
 * `?` matches a single char. We normalise refs by stripping the
 * `refs/heads/` prefix so authors can write either `main` or
 * `refs/heads/main` and both match.
 *
 * Built on picomatch for correctness; picomatch is added as a direct
 * dependency so ncc bundles it deterministically.
 */
import picomatch from "picomatch";

const REFS_HEADS_PREFIX = "refs/heads/";

/** Strip a leading `refs/heads/` prefix if present. */
export function normaliseBranchRef(ref: string): string {
  return ref.startsWith(REFS_HEADS_PREFIX) ? ref.slice(REFS_HEADS_PREFIX.length) : ref;
}

/** Strip a leading `/` from a path if present (iteration-API paths start with `/`). */
export function normalisePath(p: string): string {
  return p.startsWith("/") ? p.slice(1) : p;
}

/**
 * Apply include/exclude semantics: include must match (or be empty),
 * and exclude must not match.
 *
 * - Empty `includes` → include-all (treat as "no positive filter").
 * - Non-empty `includes` → at least one must match.
 * - Non-empty `excludes` → none must match.
 *
 * Mirrors ADO's `branches:` / `paths:` semantics in PR triggers.
 */
export function matchesIncludeExclude(
  value: string,
  includes: string[],
  excludes: string[],
): boolean {
  const normalised = normaliseBranchRef(value);
  const normIncludes = includes.map(normaliseBranchRef);
  const normExcludes = excludes.map(normaliseBranchRef);
  const includeMatches =
    normIncludes.length === 0 ||
    normIncludes.some((g) => picomatch(g, { dot: true })(normalised));
  if (!includeMatches) return false;
  if (
    normExcludes.length > 0 &&
    normExcludes.some((g) => picomatch(g, { dot: true })(normalised))
  ) {
    return false;
  }
  return true;
}

/** Path variant: normalises leading `/` instead of `refs/heads/`. */
export function pathMatchesIncludeExclude(
  pathValue: string,
  includes: string[],
  excludes: string[],
): boolean {
  const normalised = normalisePath(pathValue);
  const normIncludes = includes.map(normalisePath);
  const normExcludes = excludes.map(normalisePath);
  const includeMatches =
    normIncludes.length === 0 ||
    normIncludes.some((g) => picomatch(g, { dot: true })(normalised));
  if (!includeMatches) return false;
  if (
    normExcludes.length > 0 &&
    normExcludes.some((g) => picomatch(g, { dot: true })(normalised))
  ) {
    return false;
  }
  return true;
}
