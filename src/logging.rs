//! Centralized file-based logging for ado-aw
//!
//! All commands log to `$HOME/.ado-aw/logs/` with daily log files.
//! Each session is marked with timestamp, build ID (if in pipeline), and command name.
//! In pipeline environments, these logs are copied to the staging directory for artifact upload.

use anyhow::{Context, Result};
use chrono::{Local, Utc};
use log::LevelFilter;
use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

/// Get the standard log directory path
///
/// Returns `$HOME/.ado-aw/logs/` on Unix/macOS
/// Returns `%USERPROFILE%\.ado-aw\logs\` on Windows
pub fn log_directory() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Could not determine home directory")?;
    Ok(home.join(".ado-aw").join("logs"))
}

/// Get the path for today's log file
pub fn daily_log_path() -> Result<PathBuf> {
    let log_dir = log_directory()?;
    let date = Local::now().format("%Y-%m-%d");
    Ok(log_dir.join(format!("{}.log", date)))
}

/// Ensure the log directory exists
pub fn ensure_log_directory() -> Result<PathBuf> {
    let log_dir = log_directory()?;
    fs::create_dir_all(&log_dir).context("Failed to create log directory")?;
    Ok(log_dir)
}

/// Build the session marker line with context information
fn build_session_marker(command_name: &str) -> String {
    let timestamp = Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ");

    // Collect environment context for correlation
    let build_id = std::env::var("BUILD_BUILDID").ok();
    let build_number = std::env::var("BUILD_BUILDNUMBER").ok();
    let agent_name = std::env::var("AGENT_NAME").ok();
    let definition_name = std::env::var("BUILD_DEFINITIONNAME").ok();

    let mut parts = vec![format!("COMMAND={}", command_name)];

    if let Some(id) = build_id {
        parts.push(format!("BUILD_ID={}", id));
    }
    if let Some(num) = build_number {
        parts.push(format!("BUILD_NUMBER={}", num));
    }
    if let Some(def) = definition_name {
        parts.push(format!("PIPELINE={}", def));
    }
    if let Some(agent) = agent_name {
        parts.push(format!("AGENT={}", agent));
    }

    format!("=== [{}] {} ===", timestamp, parts.join(" "))
}

/// A simple file logger that implements log::Log
struct FileLogger {
    file: Mutex<File>,
    level: LevelFilter,
}

impl log::Log for FileLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= self.level
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            let timestamp = Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ");
            let message = format!(
                "[{}] [{}] [{}] {}\n",
                timestamp,
                record.level(),
                record.target(),
                record.args()
            );

            // Write to file
            if let Ok(mut file) = self.file.lock() {
                let _ = file.write_all(message.as_bytes());
                let _ = file.flush();
            }

            // Also write to stderr for immediate visibility
            eprint!("{}", message);
        }
    }

    fn flush(&self) {
        if let Ok(mut file) = self.file.lock() {
            let _ = file.flush();
        }
    }
}

/// Initialize file-based logging for a command
///
/// Creates/appends to today's log file at `$HOME/.ado-aw/logs/YYYY-MM-DD.log`
/// and writes a session marker with build context for correlation.
///
/// # Arguments
/// * `command_name` - Name of the command (included in session marker)
/// * `level` - Minimum log level to capture
///
/// # Returns
/// Path to the log file, or error if initialization failed
pub fn init_file_logging(command_name: &str, level: LevelFilter) -> Result<PathBuf> {
    ensure_log_directory()?;
    let log_path = daily_log_path()?;

    // Open log file in append mode
    let file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("Failed to open log file: {}", log_path.display()))?;

    // Write session start marker with context
    {
        let mut f = &file;
        let marker = build_session_marker(command_name);
        writeln!(f, "\n{}", marker)?;
    }

    let logger = FileLogger {
        file: Mutex::new(file),
        level,
    };

    log::set_boxed_logger(Box::new(logger))
        .map(|()| log::set_max_level(level))
        .context("Failed to set logger")?;

    Ok(log_path)
}

/// Initialize logging based on CLI flags
///
/// This sets up daily file logging with session markers for correlation.
///
/// # Arguments
/// * `command_name` - Name of the command for the session marker
/// * `debug` - Enable debug level logging
/// * `verbose` - Enable info level logging (ignored if debug is true)
///
/// # Returns
/// Path to the log file if file logging was initialized
pub fn init_logging(command_name: &str, debug: bool, verbose: bool) -> Option<PathBuf> {
    let level = if debug {
        LevelFilter::Debug
    } else if verbose {
        LevelFilter::Info
    } else if std::env::var("RUST_LOG").is_ok() {
        // If RUST_LOG is set, use Info as minimum for file logging
        LevelFilter::Info
    } else {
        // Default: only warnings and errors
        LevelFilter::Warn
    };

    match init_file_logging(command_name, level) {
        Ok(path) => {
            log::debug!("Logging to: {}", path.display());
            Some(path)
        }
        Err(e) => {
            // Fall back to stderr-only logging if file logging fails
            eprintln!("Warning: Could not initialize file logging: {}", e);

            // Use env_logger as fallback
            let mut builder = env_logger::Builder::new();
            if debug {
                builder.filter_level(LevelFilter::Debug);
            } else if verbose {
                builder.filter_level(LevelFilter::Info);
            } else if let Ok(rust_log) = std::env::var("RUST_LOG") {
                builder.parse_filters(&rust_log);
            }
            let _ = builder.try_init();

            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_directory() {
        let dir = log_directory().unwrap();
        assert!(
            dir.ends_with(".ado-aw/logs") || dir.ends_with(".ado-aw\\logs")
        );
    }

    #[test]
    fn test_daily_log_path() {
        let path = daily_log_path().unwrap();
        let filename = path.file_name().unwrap().to_string_lossy();
        // Should be YYYY-MM-DD.log format
        assert!(filename.ends_with(".log"));
        assert!(filename.len() == 14); // "2026-02-05.log"
    }

    #[test]
    fn test_ensure_log_directory() {
        let dir = ensure_log_directory().unwrap();
        assert!(dir.exists());
    }

    #[test]
    fn test_build_session_marker() {
        let marker = build_session_marker("test-command");
        assert!(marker.starts_with("=== ["));
        assert!(marker.contains("COMMAND=test-command"));
        assert!(marker.ends_with(" ==="));
    }
}
