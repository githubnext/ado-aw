use std::path::PathBuf;
use std::process::Command;

#[test]
fn export_gate_schema_writes_valid_json() {
    let tmp_dir = std::env::var_os("CARGO_TARGET_TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap().join("target"));
    std::fs::create_dir_all(&tmp_dir).expect("failed to create test output dir");
    let tmp = tmp_dir.join("ado-aw-test-gate-schema.json");
    let _ = std::fs::remove_file(&tmp);

    let bin = env!("CARGO_BIN_EXE_ado-aw");
    let status = Command::new(bin)
        .args(["export-gate-schema", "--output"])
        .arg(&tmp)
        .status()
        .expect("failed to spawn ado-aw");
    assert!(status.success(), "ado-aw export-gate-schema failed");

    let content = std::fs::read_to_string(&tmp).expect("schema file missing");
    let parsed: serde_json::Value =
        serde_json::from_str(&content).expect("schema is not valid JSON");
    let stringified = serde_json::to_string(&parsed).unwrap();
    assert!(
        stringified.contains("GateSpec") || stringified.contains("PredicateSpec"),
        "schema does not mention expected type names: {}",
        &stringified[..stringified.len().min(500)]
    );
}
