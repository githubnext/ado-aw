use std::path::PathBuf;

// The `run` subcommand previously had a description "Run agent locally" when
// it was an internal developer tool. It now queues ADO builds ("Queue a build
// for every ADO definition…"). This test guards against that old description
// being reinstated.
#[test]
fn test_run_agent_locally_description_absent() {
    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .arg("--help")
        .output()
        .expect("Failed to run ado-aw --help");

    assert!(output.status.success(), "--help should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("Run agent locally"),
        "Help output should not contain the old 'Run agent locally' description; \
         the run subcommand now queues ADO builds. Got:\n{stdout}"
    );
}

/// Guard that lifecycle subcommands (`list`, `disable`, `remove`, `status`) each
/// appear as a named entry in the top-level Commands section of `ado-aw --help`.
///
/// Checking only within the Commands section (between "Commands:" and "Options:")
/// prevents false-positives from description prose that incidentally contains
/// one of these common words.
///
/// This replaces four per-file one-word `stdout.contains()` tests that
/// unnecessarily spawned `ado-aw --help` four times and made no attempt to
/// isolate the Commands section.
#[test]
fn test_lifecycle_subcommands_appear_in_top_level_help_commands_section() {
    let binary_path = PathBuf::from(env!("CARGO_BIN_EXE_ado-aw"));
    let output = std::process::Command::new(&binary_path)
        .arg("--help")
        .output()
        .expect("Failed to run ado-aw --help");

    assert!(output.status.success(), "--help should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lower = stdout.to_lowercase();

    let commands_start = lower
        .find("commands:")
        .expect("top-level --help should contain a 'Commands:' section");
    let options_start = lower
        .find("options:")
        .expect("top-level --help should contain an 'Options:' section");
    let commands_block = &lower[commands_start..options_start];

    for cmd in ["list", "disable", "remove", "status"] {
        // Each lifecycle subcommand must appear as a command-line entry
        // (indented two spaces before the name) in the Commands section.
        assert!(
            commands_block.contains(&format!("  {cmd}")),
            "Expected '{cmd}' to be listed in the Commands section of `ado-aw --help`; \
             commands block was:\n{commands_block}"
        );
    }
}
