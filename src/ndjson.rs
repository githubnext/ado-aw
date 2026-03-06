//! NDJSON (Newline Delimited JSON) parsing utilities for safe outputs

use anyhow::{Context, Result};
use log::debug;
use serde_json::{Deserializer, Value};
use std::path::Path;
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;

use crate::tools::ToolResult;

/// The standard filename for safe outputs
pub const SAFE_OUTPUT_FILENAME: &str = "safe_outputs.ndjson";

/// Parse NDJSON content into a vector of JSON values
pub fn parse_ndjson(content: &str) -> Result<Vec<Value>> {
    if content.trim().is_empty() {
        return Ok(vec![]);
    }

    Deserializer::from_str(content)
        .into_iter::<Value>()
        .collect::<Result<Vec<_>, _>>()
        .context("Failed to parse NDJSON content")
}

/// Read and parse an NDJSON file
pub async fn read_ndjson_file(path: &Path) -> Result<Vec<Value>> {
    debug!("Reading NDJSON file: {}", path.display());
    let contents = tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("Failed to read NDJSON file: {}", path.display()))?;
    debug!("NDJSON file size: {} bytes", contents.len());

    let result = parse_ndjson(&contents)?;
    debug!("Parsed {} entries from NDJSON", result.len());
    Ok(result)
}

/// Append a tool result to an NDJSON file
pub async fn append_to_ndjson_file<T: ToolResult>(path: &Path, value: &T) -> Result<()> {
    debug!("Appending {} to NDJSON: {}", T::NAME, path.display());
    let line = serde_json::to_string(&value)? + "\n";
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await
        .with_context(|| format!("Failed to open NDJSON file for append: {}", path.display()))?;

    file.write_all(line.as_bytes())
        .await
        .with_context(|| format!("Failed to write to NDJSON file: {}", path.display()))?;
    debug!("Appended {} bytes to NDJSON", line.len());
    Ok(())
}

/// Initialize an empty NDJSON file
pub async fn init_ndjson_file(path: &Path) -> Result<()> {
    if path.exists() {
        Ok(())
    } else {
        tokio::fs::write(path, "")
            .await
            .with_context(|| format!("Failed to initialize NDJSON file: {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_ndjson_empty() {
        let result = parse_ndjson("").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_ndjson_whitespace_only() {
        let result = parse_ndjson("   \n  \n  ").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_ndjson_single_line() {
        let result = parse_ndjson(r#"{"name":"test","value":123}"#).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["name"], "test");
        assert_eq!(result[0]["value"], 123);
    }

    #[test]
    fn test_parse_ndjson_multiple_lines() {
        let content = r#"{"name":"first","id":1}
{"name":"second","id":2}
{"name":"third","id":3}
"#;
        let result = parse_ndjson(content).unwrap();
        assert_eq!(result.len(), 3);
        assert_eq!(result[0]["name"], "first");
        assert_eq!(result[1]["name"], "second");
        assert_eq!(result[2]["name"], "third");
    }

    #[test]
    fn test_parse_ndjson_invalid_json() {
        let result = parse_ndjson(r#"{"invalid": json}"#);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_init_and_read_ndjson_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("test.ndjson");

        init_ndjson_file(&path).await.unwrap();
        let contents = read_ndjson_file(&path).await.unwrap();
        assert!(contents.is_empty());
    }
}
