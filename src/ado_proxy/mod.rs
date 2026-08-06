//! Credential-isolated Azure DevOps proxy policy.
//!
//! This module owns the **authoritative** definition of which Azure DevOps
//! operations a Stage 1 agent may perform. It is deliberately data-only: the
//! proxy *runtime* that enforces this policy ships as the
//! `ado-proxy` TypeScript bundle in `scripts/ado-script/`, alongside
//! the other `ado-script` bundles, and is downloaded into the pipeline as part
//! of `ado-script.zip`.
//!
//! # Why the runtime is not here
//!
//! A Rust runtime would need a TLS stack plus certificate minting
//! (`rustls` + `rcgen`), which pulls in `ring` — C and assembly — making a
//! native toolchain a hard build requirement for the whole compiler. ado-aw is
//! otherwise pure-Rust and must stay buildable without one. Node's built-in
//! `tls`/`http`/`net` modules cover the same ground with no new dependency, and
//! match how AWF implements its own credential-isolating sidecars.
//!
//! # Avoiding divergence between compiler and sidecar
//!
//! The compiler *emits* the policy document and the sidecar *consumes* it, so
//! the two must not drift. Rather than maintaining a second copy by hand, every
//! other artefact is generated from the types in [`catalog`]:
//!
//! 1. the JSON Schema, exported by `ado-aw export-ado-proxy-catalog-schema`
//!    and turned into TypeScript types by the `ado-script` `codegen` script, so
//!    the bundle cannot compile against a stale shape;
//! 2. a committed data snapshot, exported by `ado-aw export-ado-proxy-catalog`,
//!    guarded by a drift test that re-runs the exporter and fails on any diff;
//! 3. [`catalog::CATALOG_SCHEMA_VERSION`], embedded in the emitted policy
//!    document and re-checked by the sidecar at startup, so a stale mounted
//!    policy file fails closed instead of under-enforcing.
//!
//! This mirrors the existing `export-gate-schema` / `export-fact-catalog`
//! pattern used by the gate evaluator.

pub mod catalog;
pub mod policy;
