export * as auth from "./auth.js";
export * as vso from "./vso-logger.js";
export * as envFacts from "./env-facts.js";
export * as policy from "./policy.js";
export * as adoClient from "./ado-client.js";
export * as adoRemote from "./ado-remote.js";
// Promoted from exec-context-pr/ during Stage 0 of the contributor
// build-out so upcoming contributors (`pipeline`, `ci-push`,
// `workitem`, ...) can reuse them without fragmenting the workspace
// with an `exec-context-common/` sibling. See plan.md "Stage 0".
export * as git from "./git.js";
export * as mergeBase from "./merge-base.js";
export * as validate from "./validate.js";
export * as prompt from "./prompt.js";
// Added in Stage 2 of the contributor build-out — see plan.md.
// Build-API helpers shared by `pipeline`, `ci-push`, and
// `pr.checks` contributors.
export * as build from "./build.js";
// Added in Stage 4 of the contributor build-out — see plan.md.
// Work-item REST helpers + the untrusted-prose sentinel wrapper.
// These are kept separate because `wit.ts` is REST and SDK-heavy
// while `untrusted.ts` is pure and very lightweight.
export * as wit from "./wit.js";
export * as untrusted from "./untrusted.js";
