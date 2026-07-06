//! Registry of ado-script Node bundles and their compile-time env contracts.
//!
//! Every compiler-emitted pipeline step that runs an ado-script bundle
//! (`/tmp/ado-aw-scripts/ado-script/<name>.js`) has an implicit environment
//! contract: which `process.env` keys the bundle reads. Historically each
//! call site hand-wrote that step's `env:` block, which is how the Conclusion
//! step drifted and shipped without an ADO bearer token (issue #1307 — the
//! shared `getWebApi()` auth helper had no `SYSTEM_ACCESSTOKEN`).
//!
//! This module makes the **auth** half of that contract explicit and
//! enforceable:
//!
//! * [`Bundle`] enumerates every bundle, with its on-disk [`Bundle::path`] and
//!   its [`Bundle::auth`] requirement.
//! * [`apply_bundle_auth`] is the single chokepoint that projects
//!   `SYSTEM_ACCESSTOKEN` into a step for every bearer-requiring bundle, so no
//!   call site can forget it again.
//! * [`token_source_for`] unifies the `System.AccessToken` vs `SC_WRITE_TOKEN`
//!   selection that was previously duplicated between the Conclusion job and
//!   the Stage 3 executor.
//!
//! ## What is *not* modelled here
//!
//! The ADO collection URI is read from the **auto-injected**
//! `SYSTEM_COLLECTIONURI` (see `scripts/ado-script/src/shared/auth.ts` and
//! #1307), so it is not part of any step's env contract. Likewise every ADO
//! predefined variable (`System.*`, `Build.*`) is auto-injected into every
//! script step's env under its SCREAMING_SNAKE name, so bundle steps must not
//! re-project them — [`is_redundant_ado_mirror`] identifies such redundant
//! mirrors and the contract test asserts migrated steps do not emit them.
//! `SYSTEM_ACCESSTOKEN` is the one documented exception: ADO maps it only when
//! a step explicitly references it, which is exactly what [`apply_bundle_auth`]
//! does.

use crate::compile::extensions::ado_script as paths;
use crate::compile::ir::env::EnvValue;
use crate::compile::ir::step::BashStep;

/// An ado-script Node bundle shipped in `ado-script.zip` and unpacked to
/// `/tmp/ado-aw-scripts/ado-script/<name>.js` at pipeline runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bundle {
    Gate,
    Import,
    ExecContextPr,
    ExecContextPrSynth,
    ExecContextPrChecks,
    ExecContextPipeline,
    ExecContextCiPush,
    ExecContextWorkitem,
    ExecContextSchedule,
    ExecContextManual,
    ExecContextRepo,
    ApprovalSummary,
    Conclusion,
    /// GitHub App installation-token minter/revoker (issue #1316). Runs before
    /// the Copilot invocation in the Agent and Detection jobs. It authenticates
    /// to the **GitHub** API (not ADO REST), so it needs no ADO bearer.
    GithubAppToken,
}

/// The auth contract a bundle requires from the step that invokes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BundleAuth {
    /// The bundle reads `SYSTEM_ACCESSTOKEN` from its env — for ADO REST auth
    /// (via `getWebApi()` in `scripts/ado-script/src/shared/auth.ts`), for git
    /// credential/bearer auth (via `shared/git.ts::bearerEnv`), or both — so
    /// its step must project the token. `SYSTEM_ACCESSTOKEN` is the one ADO
    /// predefined variable that is *not* auto-injected (ADO maps it only on
    /// explicit reference), which is why it must be projected. The collection
    /// URI, by contrast, comes from the auto-injected `SYSTEM_COLLECTIONURI`
    /// and is therefore *not* part of this contract (see #1307).
    Bearer,
    /// The bundle needs no bearer (pure filesystem / git-without-auth / argv).
    None,
}

/// Which pipeline secret variable supplies the ADO bearer token for a step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenSource {
    /// The pipeline's built-in OAuth token (`$(System.AccessToken)`), scoped
    /// by the pipeline's job-authorization settings.
    SystemAccessToken,
    /// A write-capable ADO token minted from an ARM service connection into
    /// the `SC_WRITE_TOKEN` pipeline variable (Stage 3 executor / Conclusion).
    WriteServiceConnection,
}

impl TokenSource {
    /// The bare pipeline-variable name backing this token source. Lowering
    /// wraps it as `$(<name>)`, so `SystemAccessToken` produces the same wire
    /// form (`$(System.AccessToken)`) that the hand-written call sites emitted
    /// before this module existed — no YAML churn on the token line.
    pub fn variable(self) -> &'static str {
        match self {
            TokenSource::SystemAccessToken => "System.AccessToken",
            TokenSource::WriteServiceConnection => "SC_WRITE_TOKEN",
        }
    }
}

/// Select the token source for a step given the optional write service
/// connection. When a write SC is configured the compiler mints
/// `SC_WRITE_TOKEN` upstream; otherwise the built-in `$(System.AccessToken)`
/// is used. Shared by the Conclusion job and the Stage 3 executor so the two
/// paths cannot disagree.
pub fn token_source_for(write_service_connection: Option<&str>) -> TokenSource {
    if write_service_connection.is_some() {
        TokenSource::WriteServiceConnection
    } else {
        TokenSource::SystemAccessToken
    }
}

impl Bundle {
    /// Every bundle, for registry-wide iteration. Consumed by the contract
    /// tests in this module. `ALL` is compiled in all builds (not `cfg(test)`)
    /// so that every `Bundle` variant is constructed here — that keeps the
    /// `None`-auth variants, which production only ever *matches* on (never
    /// constructs), from tripping `dead_code`. `ALL` itself is unused outside
    /// tests, hence `#[allow(dead_code)]`.
    #[allow(dead_code)]
    pub const ALL: &'static [Bundle] = &[
        Bundle::Gate,
        Bundle::Import,
        Bundle::ExecContextPr,
        Bundle::ExecContextPrSynth,
        Bundle::ExecContextPrChecks,
        Bundle::ExecContextPipeline,
        Bundle::ExecContextCiPush,
        Bundle::ExecContextWorkitem,
        Bundle::ExecContextSchedule,
        Bundle::ExecContextManual,
        Bundle::ExecContextRepo,
        Bundle::ApprovalSummary,
        Bundle::Conclusion,
        Bundle::GithubAppToken,
    ];

    /// The bundle's unpacked on-disk path inside the runtime VM. The Conclusion
    /// bundle is referenced inline by `agentic_pipeline` (not via a shared
    /// const), so its literal path is repeated here as the single registry
    /// source of truth. Consumed by the contract tests in this module
    /// (production references the path constants directly); `#[allow(dead_code)]`
    /// rather than `#[cfg(test)]` so `paths` stays a normal import.
    #[allow(dead_code)]
    pub fn path(self) -> &'static str {
        match self {
            Bundle::Gate => paths::GATE_EVAL_PATH,
            Bundle::Import => paths::IMPORT_EVAL_PATH,
            Bundle::ExecContextPr => paths::EXEC_CONTEXT_PR_PATH,
            Bundle::ExecContextPrSynth => paths::EXEC_CONTEXT_PR_SYNTH_PATH,
            Bundle::ExecContextPrChecks => paths::EXEC_CONTEXT_PR_CHECKS_PATH,
            Bundle::ExecContextPipeline => paths::EXEC_CONTEXT_PIPELINE_PATH,
            Bundle::ExecContextCiPush => paths::EXEC_CONTEXT_CI_PUSH_PATH,
            Bundle::ExecContextWorkitem => paths::EXEC_CONTEXT_WORKITEM_PATH,
            Bundle::ExecContextSchedule => paths::EXEC_CONTEXT_SCHEDULE_PATH,
            Bundle::ExecContextManual => paths::EXEC_CONTEXT_MANUAL_PATH,
            Bundle::ExecContextRepo => paths::EXEC_CONTEXT_REPO_PATH,
            Bundle::ApprovalSummary => paths::APPROVAL_SUMMARY_PATH,
            Bundle::Conclusion => paths::CONCLUSION_PATH,
            Bundle::GithubAppToken => paths::GITHUB_APP_TOKEN_PATH,
        }
    }

    /// The auth contract this bundle requires from its invoking step.
    pub fn auth(self) -> BundleAuth {
        match self {
            // These bundles read SYSTEM_ACCESSTOKEN at runtime — for ADO REST
            // (via getWebApi()) and/or git bearer auth (via bearerEnv).
            Bundle::Gate
            | Bundle::ExecContextPr
            | Bundle::ExecContextPrSynth
            | Bundle::ExecContextPrChecks
            | Bundle::ExecContextPipeline
            | Bundle::ExecContextCiPush
            | Bundle::ExecContextWorkitem
            | Bundle::ExecContextSchedule
            | Bundle::Conclusion => BundleAuth::Bearer,
            // Pure filesystem / git-without-auth / argv — no bearer.
            Bundle::Import
            | Bundle::ExecContextManual
            | Bundle::ExecContextRepo
            | Bundle::ApprovalSummary
            // Authenticates to the GitHub API with its own App JWT / minted
            // token, not the ADO bearer.
            | Bundle::GithubAppToken => BundleAuth::None,
        }
    }
}

/// Project the bundle's auth env contract onto a step.
///
/// For [`BundleAuth::Bearer`] bundles this maps `SYSTEM_ACCESSTOKEN` from the
/// chosen [`TokenSource`]. For [`BundleAuth::None`] it is a no-op — the `token`
/// argument is ignored (a `None`-auth bundle needs no bearer, and today no
/// caller routes such a bundle through this function). This is the single
/// guarantee that every bearer-requiring bundle step carries a token — the
/// structural fix for the class of bug behind #1307.
pub fn apply_bundle_auth(step: BashStep, bundle: Bundle, token: TokenSource) -> BashStep {
    match bundle.auth() {
        BundleAuth::Bearer => {
            step.with_env("SYSTEM_ACCESSTOKEN", EnvValue::secret(token.variable()))
        }
        BundleAuth::None => step,
    }
}

/// True iff `(key, value)` is a redundant re-projection of an ADO predefined
/// variable that the runtime already auto-injects into every script step's env
/// under the SCREAMING_SNAKE form of its dotted name
/// (e.g. `SYSTEM_TEAMPROJECT: $(System.TeamProject)`).
///
/// The auth token is deliberately projected as [`EnvValue::Secret`] (not
/// [`EnvValue::AdoMacro`]) by [`apply_bundle_auth`], so it is never flagged
/// here even though `System.AccessToken` shares the `SYSTEM_ACCESSTOKEN` name —
/// that projection is intentional (ADO maps the token only on explicit
/// reference).
///
/// Note: some auto-injected vars are still projected deliberately even though
/// this function *would* flag them — e.g. the manual contributor's
/// `BUILD_REQUESTEDFOR` / `BUILD_REQUESTEDFOREMAIL` requestor-identity vars,
/// which are retained to keep the email-hygiene opt-in visible at the call
/// site. Such exemptions live in an explicit allowlist at the (test) call site
/// rather than being special-cased here, so this predicate stays a pure
/// "does the key mirror the macro" check.
///
/// Consumed by the contract tests in this module; the compiled-YAML churn
/// guard in `tests/compiler_tests.rs` re-implements the same rule at the
/// integration level. Test-only, so compiled only under `cfg(test)`.
#[cfg(test)]
pub fn is_redundant_ado_mirror(key: &str, value: &EnvValue) -> bool {
    match value {
        EnvValue::AdoMacro(name) => key == name.replace('.', "_").to_uppercase(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bundle_path_is_under_the_unpack_dir() {
        for b in Bundle::ALL {
            assert!(
                b.path().starts_with("/tmp/ado-aw-scripts/ado-script/"),
                "{b:?} path must live under the unzip destination"
            );
            assert!(b.path().ends_with(".js"), "{b:?} path must be a .js bundle");
        }
    }

    #[test]
    fn apply_bundle_auth_projects_token_for_bearer_bundles_only() {
        for b in Bundle::ALL {
            let step = BashStep::new("t", "node x\n");
            let out = apply_bundle_auth(step, *b, TokenSource::SystemAccessToken);
            let has_token = out.env.contains_key("SYSTEM_ACCESSTOKEN");
            match b.auth() {
                BundleAuth::Bearer => assert!(
                    has_token,
                    "{b:?} is Bearer and must carry SYSTEM_ACCESSTOKEN"
                ),
                BundleAuth::None => assert!(
                    !has_token,
                    "{b:?} is None and must not carry SYSTEM_ACCESSTOKEN"
                ),
            }
        }
    }

    #[test]
    fn token_source_selection_matches_write_service_connection() {
        assert_eq!(token_source_for(None), TokenSource::SystemAccessToken);
        assert_eq!(token_source_for(None).variable(), "System.AccessToken");
        assert_eq!(
            token_source_for(Some("my-sc")),
            TokenSource::WriteServiceConnection
        );
        assert_eq!(token_source_for(Some("my-sc")).variable(), "SC_WRITE_TOKEN");
    }

    #[test]
    fn projected_auth_token_is_not_flagged_as_a_mirror() {
        // Secret-sourced token must never be mistaken for a redundant mirror,
        // even though System.AccessToken -> SYSTEM_ACCESSTOKEN name-matches.
        let token = EnvValue::secret(TokenSource::SystemAccessToken.variable());
        assert!(!is_redundant_ado_mirror("SYSTEM_ACCESSTOKEN", &token));
    }

    #[test]
    fn detects_redundant_and_genuine_env() {
        // Redundant mirror of an auto-injected predefined variable.
        assert!(is_redundant_ado_mirror(
            "SYSTEM_TEAMPROJECT",
            &EnvValue::ado_macro("System.TeamProject").unwrap()
        ));
        assert!(is_redundant_ado_mirror(
            "BUILD_SOURCESDIRECTORY",
            &EnvValue::ado_macro("Build.SourcesDirectory").unwrap()
        ));
        // Genuine computed input (literal) is never a mirror.
        assert!(!is_redundant_ado_mirror(
            "GATE_SPEC",
            &EnvValue::literal("base64")
        ));
        // A pipeline-var override (synth PR id) is never a mirror.
        assert!(!is_redundant_ado_mirror(
            "SYSTEM_PULLREQUEST_PULLREQUESTID",
            &EnvValue::pipeline_var("AW_PR_ID")
        ));
        // A macro projected under a *different* key is not a self-mirror.
        assert!(!is_redundant_ado_mirror(
            "AW_PR_ID",
            &EnvValue::ado_macro("System.PullRequest.PullRequestId").unwrap()
        ));
    }
}
