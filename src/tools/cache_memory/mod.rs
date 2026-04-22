//! Cache memory first-class tool.
//!
//! Compile-time: injects pipeline steps (download/restore previous memory)
//! and a prompt supplement informing the agent about its memory directory.
//!
//! Stage 3 runtime: validates and copies sanitized memory files to the
//! final safe_outputs artifact for pickup by the next run.

pub mod execute;
pub mod extension;

pub use execute::{MemoryConfig, process_agent_memory};
pub use extension::CacheMemoryExtension;
