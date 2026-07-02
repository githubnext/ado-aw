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
//!   `SYSTEM_ACCESSTOKEN` into a step for every REST-calling bundle, so no
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
}

/// The auth contract a bundle requires from the step that invokes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BundleAuth {
    /// The bundle authenticates to the ADO REST API via `getWebApi()` in
    /// `scripts/ado-script/src/shared/auth.ts`, so its step must map
    /// `SYSTEM_ACCESSTOKEN`. The collection URI comes from the auto-injected
    /// `SYSTEM_COLLECTIONURI` and is therefore *not* part of this contract
    /// (see #1307).
    AdoRest,
    /// The bundle needs no ADO REST auth (pure filesystem / git / argv).
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
    /// tests in this module (the bin artifact itself iterates a fixed set of
    /// call sites, so this is `dead_code` outside `cfg(test)`).
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
    ];

    /// The bundle's unpacked on-disk path inside the runtime VM. The Conclusion
    /// bundle is referenced inline by `agentic_pipeline` (not via a shared
    /// const), so its literal path is repeated here as the single registry
    /// source of truth. Consumed by the contract tests in this module.
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
            Bundle::Conclusion => "/tmp/ado-aw-scripts/ado-script/conclusion.js",
        }
    }

    /// The auth contract this bundle requires from its invoking step.
    pub fn auth(self) -> BundleAuth {
        match self {
            // These bundles call `getWebApi()` (ADO REST) at runtime.
            Bundle::Gate
            | Bundle::ExecContextPr
            | Bundle::ExecContextPrSynth
            | Bundle::ExecContextPrChecks
            | Bundle::ExecContextPipeline
            | Bundle::ExecContextCiPush
            | Bundle::ExecContextWorkitem
            | Bundle::ExecContextSchedule
            | Bundle::Conclusion => BundleAuth::AdoRest,
            // Pure filesystem / git / argv — no ADO REST auth.
            Bundle::Import
            | Bundle::ExecContextManual
            | Bundle::ExecContextRepo
            | Bundle::ApprovalSummary => BundleAuth::None,
        }
    }
}

/// Project the bundle's auth env contract onto a step.
///
/// For [`BundleAuth::AdoRest`] bundles this maps `SYSTEM_ACCESSTOKEN` from the
/// chosen [`TokenSource`]; for [`BundleAuth::None`] it is a no-op. This is the
/// single guarantee that every REST-calling bundle step carries a bearer token
/// — the structural fix for the class of bug behind #1307.
pub fn apply_bundle_auth(step: BashStep, bundle: Bundle, token: TokenSource) -> BashStep {
    match bundle.auth() {
        BundleAuth::AdoRest => {
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
/// Consumed by the contract tests in this module; the compiled-YAML churn
/// guard in `tests/compiler_tests.rs` re-implements the same rule at the
/// integration level.
#[allow(dead_code)]
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
                b.path()
                    .starts_with("/tmp/ado-aw-scripts/ado-script/"),
                "{b:?} path must live under the unzip destination"
            );
            assert!(b.path().ends_with(".js"), "{b:?} path must be a .js bundle");
        }
    }

    #[test]
    fn apply_bundle_auth_projects_token_for_ado_rest_only() {
        for b in Bundle::ALL {
            let step = BashStep::new("t", "node x\n");
            let out = apply_bundle_auth(step, *b, TokenSource::SystemAccessToken);
            let has_token = out.env.contains_key("SYSTEM_ACCESSTOKEN");
            match b.auth() {
                BundleAuth::AdoRest => assert!(
                    has_token,
                    "{b:?} is AdoRest and must carry SYSTEM_ACCESSTOKEN"
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
        assert_eq!(
            token_source_for(Some("my-sc")).variable(),
            "SC_WRITE_TOKEN"
        );
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
