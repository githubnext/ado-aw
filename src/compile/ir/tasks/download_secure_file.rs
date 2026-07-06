//! Typed builder for `DownloadSecureFile@1`.

use crate::compile::ir::step::TaskStep;
use serde::Deserialize;

use super::common::de_opt_str_or_int;

/// Builder for a [`TaskStep`] invoking `DownloadSecureFile@1`.
///
/// Downloads a secure file from the ADO Secure Files library to the agent
/// machine. The file is automatically deleted at the end of the pipeline run.
/// After the task completes the downloaded path is available through the
/// `$(DownloadSecureFile.secureFilePath)` output variable.
///
/// ADO task reference:
/// <https://learn.microsoft.com/en-us/azure/devops/pipelines/tasks/reference/download-secure-file-v1>
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DownloadSecureFile {
    #[serde(rename = "secureFile")]
    secure_file: String,
    #[serde(rename = "retryCount", default, deserialize_with = "de_opt_str_or_int")]
    retry_count: Option<String>,
    #[serde(
        rename = "socketTimeout",
        default,
        deserialize_with = "de_opt_str_or_int"
    )]
    socket_timeout: Option<String>,
    #[serde(skip)]
    display_name: Option<String>,
}

impl DownloadSecureFile {
    /// Required input: `secureFile` — the name or GUID of the secure file in
    /// the ADO Secure Files library.
    pub fn new(secure_file: impl Into<String>) -> Self {
        Self {
            secure_file: secure_file.into(),
            retry_count: None,
            socket_timeout: None,
            display_name: None,
        }
    }

    /// `retryCount` — number of retry attempts when the download fails
    /// (ADO default: `"8"`).
    pub fn retry_count(mut self, value: impl Into<String>) -> Self {
        self.retry_count = Some(value.into());
        self
    }

    /// `socketTimeout` — socket timeout in milliseconds for the download
    /// request. Leave unset to use the ADO default.
    pub fn socket_timeout(mut self, value: impl Into<String>) -> Self {
        self.socket_timeout = Some(value.into());
        self
    }

    /// Override the default `displayName` (`"Download secure file"`).
    pub fn with_display_name(mut self, value: impl Into<String>) -> Self {
        self.display_name = Some(value.into());
        self
    }

    /// Lower into a [`TaskStep`].
    pub fn into_step(self) -> TaskStep {
        let mut t = TaskStep::new(
            "DownloadSecureFile@1",
            self.display_name
                .unwrap_or_else(|| "Download secure file".into()),
        )
        .with_input("secureFile", self.secure_file);
        if let Some(v) = self.retry_count {
            t = t.with_input("retryCount", v);
        }
        if let Some(v) = self.socket_timeout {
            t = t.with_input("socketTimeout", v);
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sets_task_and_required_input() {
        let t = DownloadSecureFile::new("signing-cert.pfx").into_step();
        assert_eq!(t.task, "DownloadSecureFile@1");
        assert_eq!(t.display_name, "Download secure file");
        assert_eq!(
            t.inputs.get("secureFile").map(String::as_str),
            Some("signing-cert.pfx")
        );
        // Optional inputs absent by default.
        assert!(t.inputs.get("retryCount").is_none());
        assert!(t.inputs.get("socketTimeout").is_none());
    }

    #[test]
    fn optional_inputs_emit_only_when_set() {
        let t = DownloadSecureFile::new("my-cert.p12")
            .retry_count("3")
            .socket_timeout("5000")
            .into_step();
        assert_eq!(t.inputs.get("retryCount").map(String::as_str), Some("3"));
        assert_eq!(
            t.inputs.get("socketTimeout").map(String::as_str),
            Some("5000")
        );
    }

    #[test]
    fn display_name_override() {
        let t = DownloadSecureFile::new("build-cert.pfx")
            .with_display_name("Fetch signing certificate")
            .into_step();
        assert_eq!(t.display_name, "Fetch signing certificate");
    }

    #[test]
    fn accepts_guid_as_secure_file_id() {
        let guid = "2a6ca863-f2ce-4f4d-8bcb-15e64608ec4b";
        let t = DownloadSecureFile::new(guid).into_step();
        assert_eq!(t.inputs.get("secureFile").map(String::as_str), Some(guid));
    }
}
