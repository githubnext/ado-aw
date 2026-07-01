//! Typed builder for `AzureKeyVault@2`.

use super::common::{bool_input, de_opt_bool_flex};
use crate::compile::ir::step::TaskStep;
use serde::Deserialize;

/// Builder for a [`TaskStep`] invoking `AzureKeyVault@2`.
///
/// Downloads secrets from an Azure Key Vault into pipeline variables so
/// subsequent steps can reference them via `$(secret-name)`. The task
/// authenticates using the Azure Resource Manager service connection
/// `connected_service_name`.
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/azure-key-vault-v2-task>
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AzureKeyVault {
    #[serde(rename = "ConnectedServiceName")]
    connected_service_name: String,
    #[serde(rename = "KeyVaultName")]
    key_vault_name: String,
    #[serde(rename = "SecretsFilter", default)]
    secrets_filter: Option<String>,
    #[serde(rename = "RunAsPreJob", default, deserialize_with = "de_opt_bool_flex")]
    run_as_pre_job: Option<bool>,
    #[serde(skip)]
    display_name: Option<String>,
}

impl AzureKeyVault {
    /// Required inputs: `ConnectedServiceName` (Azure RM service connection)
    /// and `KeyVaultName` (the vault to read from).
    pub fn new(
        connected_service_name: impl Into<String>,
        key_vault_name: impl Into<String>,
    ) -> Self {
        Self {
            connected_service_name: connected_service_name.into(),
            key_vault_name: key_vault_name.into(),
            secrets_filter: None,
            run_as_pre_job: None,
            display_name: None,
        }
    }

    /// `SecretsFilter` — comma-separated list of secret names to download, or
    /// `*` (the ADO default) to download all secrets.
    pub fn secrets_filter(mut self, value: impl Into<String>) -> Self {
        self.secrets_filter = Some(value.into());
        self
    }

    /// `RunAsPreJob` — when `true` the task runs before the job even starts,
    /// making secrets available to every step without an explicit dependency.
    /// ADO default is `false`.
    pub fn run_as_pre_job(mut self, value: bool) -> Self {
        self.run_as_pre_job = Some(value);
        self
    }

    /// Override the default `displayName` (`"Azure Key Vault"`).
    pub fn with_display_name(mut self, value: impl Into<String>) -> Self {
        self.display_name = Some(value.into());
        self
    }

    /// Lower into a [`TaskStep`].
    pub fn into_step(self) -> TaskStep {
        let mut t = TaskStep::new(
            "AzureKeyVault@2",
            self.display_name.unwrap_or_else(|| "Azure Key Vault".into()),
        )
        .with_input("ConnectedServiceName", self.connected_service_name)
        .with_input("KeyVaultName", self.key_vault_name);
        if let Some(v) = self.secrets_filter {
            t = t.with_input("SecretsFilter", v);
        }
        if let Some(v) = self.run_as_pre_job {
            t = t.with_input("RunAsPreJob", bool_input(v));
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sets_task_and_required_inputs() {
        let t = AzureKeyVault::new("my-arm-connection", "my-key-vault").into_step();
        assert_eq!(t.task, "AzureKeyVault@2");
        assert_eq!(t.display_name, "Azure Key Vault");
        assert_eq!(
            t.inputs.get("ConnectedServiceName").map(String::as_str),
            Some("my-arm-connection")
        );
        assert_eq!(
            t.inputs.get("KeyVaultName").map(String::as_str),
            Some("my-key-vault")
        );
        assert!(t.inputs.get("SecretsFilter").is_none());
        assert!(t.inputs.get("RunAsPreJob").is_none());
    }

    #[test]
    fn optional_inputs_emit_only_when_set() {
        let t = AzureKeyVault::new("svc-conn", "prod-vault")
            .secrets_filter("MY_SECRET,ANOTHER_SECRET")
            .run_as_pre_job(true)
            .into_step();
        assert_eq!(
            t.inputs.get("SecretsFilter").map(String::as_str),
            Some("MY_SECRET,ANOTHER_SECRET")
        );
        assert_eq!(t.inputs.get("RunAsPreJob").map(String::as_str), Some("true"));
    }

    #[test]
    fn wildcard_secrets_filter() {
        let t = AzureKeyVault::new("svc-conn", "dev-vault")
            .secrets_filter("*")
            .into_step();
        assert_eq!(t.inputs.get("SecretsFilter").map(String::as_str), Some("*"));
    }

    #[test]
    fn run_as_pre_job_false() {
        let t = AzureKeyVault::new("svc-conn", "vault").run_as_pre_job(false).into_step();
        assert_eq!(t.inputs.get("RunAsPreJob").map(String::as_str), Some("false"));
    }

    #[test]
    fn display_name_override() {
        let t = AzureKeyVault::new("svc-conn", "vault")
            .with_display_name("Download deployment secrets")
            .into_step();
        assert_eq!(t.display_name, "Download deployment secrets");
    }
}
