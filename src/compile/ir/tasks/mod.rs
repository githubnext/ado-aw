//! Typed builder structs for ADO built-in pipeline tasks.
//!
//! Each ADO task is modeled as a **builder struct** with `new(<required>)`, one
//! typed chained setter per optional input, and `into_step(self) -> TaskStep`.
//! Only inputs that were explicitly set are emitted, so generated YAML stays
//! minimal and matches the task's own defaults. Constrained input values are
//! typed enums (each with `as_ado_str()`); bool-string inputs are `Option<bool>`.
//!
//! Command/mode-dispatch tasks (`Docker@2`, `DotNetCoreCLI@2`, `NuGetCommand@2`,
//! `PowerShell@2`) use a command enum whose variants carry the per-command
//! optional inputs, so applying an input to the wrong command is
//! unrepresentable. [`docker::Docker`] is the canonical template for new such
//! tasks.
//!
//! Each task lives in its own submodule; reference a builder by its module path,
//! e.g. `tasks::copy_files::CopyFiles`. Call sites wrap the result in
//! [`crate::compile::ir::step::Step::Task`], e.g.
//! `Step::Task(copy_files::CopyFiles::new(contents, dst).into_step())`.

mod common;

pub mod archive_files;
pub mod azure_key_vault;
pub mod azure_powershell;
pub mod cargo_authenticate;
pub mod cmd_line;
pub mod copy_files;
pub mod delete_files;
pub mod docker;
pub mod docker_installer;
pub mod dotnet_core_cli;
pub mod download_package;
pub mod download_pipeline_artifact;
pub mod extract_files;
pub mod go_tool;
pub mod java_tool_installer;
pub mod npm;
pub mod nuget_authenticate;
pub mod nuget_command;
pub mod powershell;
pub mod python_script;
pub mod publish_build_artifacts;
pub mod publish_code_coverage_results;
pub mod publish_pipeline_artifact;
pub mod publish_test_results;
pub mod vstest;
