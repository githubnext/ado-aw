use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_yaml::{Mapping, Value};

use super::alias::{import_resource_parent_diagnostic, synthesize_repo_aliases};
use super::merge::merge_resolved;
use super::{ImportProvenance, ManifestFetcher, ResolvedImport, resolve_imports};
use crate::compile::types::{
    CompileTarget, ImportEndpoint, ImportEntry, ImportSource, ParsedImportSpec,
};
use crate::secure::CommitSha;

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

struct PanicFetcher;

#[async_trait::async_trait]
impl ManifestFetcher for PanicFetcher {
    async fn fetch(&self, _spec: &ParsedImportSpec) -> Result<Vec<u8>> {
        panic!("integration tests must not fetch remote imports")
    }
}

fn temp_repo() -> tempfile::TempDir {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("imports-integration-tmp");
    fs::create_dir_all(&root).expect("create integration temp root");
    tempfile::Builder::new()
        .prefix("repo-")
        .tempdir_in(root)
        .expect("create temp repo")
}

fn key(name: &str) -> Value {
    Value::String(name.to_string())
}

fn ymap(yaml: &str) -> Mapping {
    match serde_yaml::from_str::<Value>(yaml).expect("valid YAML") {
        Value::Mapping(mapping) => mapping,
        other => panic!("expected mapping, got {other:?}"),
    }
}

fn map_get<'a>(mapping: &'a Mapping, name: &str) -> &'a Value {
    mapping
        .get(key(name))
        .unwrap_or_else(|| panic!("expected mapping key `{name}`"))
}

fn map_get_mapping<'a>(mapping: &'a Mapping, name: &str) -> &'a Mapping {
    value_as_mapping(map_get(mapping, name))
}

fn value_as_mapping(value: &Value) -> &Mapping {
    match value {
        Value::Mapping(mapping) => mapping,
        other => panic!("expected mapping value, got {other:?}"),
    }
}

fn import_entry(uses: &str) -> ImportEntry {
    ImportEntry {
        uses: uses.to_string(),
        with: serde_json::Map::new(),
        endpoint: None,
    }
}

fn parse_workflow(path: &Path) -> (Mapping, String, Vec<ImportEntry>) {
    let content = fs::read_to_string(path).expect("read workflow");
    let parts = crate::compile::common::split_markdown_front_matter(&content, true)
        .expect("split workflow front matter");
    let front_matter = match serde_yaml::from_str::<Value>(
        parts.yaml_raw.as_deref().expect("front matter exists"),
    )
    .expect("parse workflow front matter")
    {
        Value::Mapping(mapping) => mapping,
        other => panic!("expected workflow mapping, got {other:?}"),
    };
    let imports = front_matter
        .get(key("imports"))
        .map(|value| {
            serde_yaml::from_value::<Vec<ImportEntry>>(value.clone()).expect("deserialize imports")
        })
        .unwrap_or_default();

    (front_matter, parts.markdown_body, imports)
}

fn write_component(dir: &Path, name: &str, content: &str) {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create component parent");
    }
    fs::write(path, content).expect("write component");
}

async fn resolve_local(entries: &[ImportEntry], base_dir: &Path) -> Vec<ResolvedImport> {
    resolve_imports(entries, base_dir, &PanicFetcher)
        .await
        .expect("resolve local imports")
}

#[tokio::test]
async fn imports_integration_local_resolve_then_merge_consumer_wins_and_unions() {
    let repo = temp_repo();
    let workflow_dir = repo.path().join("workflows");
    fs::create_dir_all(&workflow_dir).expect("create workflow dir");
    write_component(
        &workflow_dir,
        "components/notify.md",
        r#"---
target: 1es
tools:
  edit: {}
safe-outputs:
  imported-notify:
    run: node scripts/notify.js
    inputs:
      message:
        type: string
---
Imported guidance.
"#,
    );
    let workflow_path = workflow_dir.join("agent.md");
    fs::write(
        &workflow_path,
        r#"---
name: consumer
description: consumer workflow
target: standalone
imports:
  - components/notify.md
tools:
  bash: {}
---
Consumer guidance.
"#,
    )
    .expect("write workflow");

    let (mut consumer_fm, consumer_body, entries) = parse_workflow(&workflow_path);
    let resolved = resolve_local(&entries, &workflow_dir).await;
    let merged_body =
        merge_resolved(&mut consumer_fm, &consumer_body, &resolved).expect("merge imports");

    assert_eq!(
        map_get(&consumer_fm, "target"),
        &Value::String("standalone".into())
    );
    assert!(
        !consumer_fm.contains_key(key("imports")),
        "imports key should be consumed"
    );
    let tools = map_get_mapping(&consumer_fm, "tools");
    assert!(tools.contains_key(key("edit")), "imported tool missing");
    assert!(tools.contains_key(key("bash")), "consumer tool missing");
    let safe_outputs = map_get_mapping(&consumer_fm, "safe-outputs");
    assert!(safe_outputs.contains_key(key("imported-notify")));
    assert_eq!(merged_body, "Imported guidance.\n\nConsumer guidance.");
}

#[tokio::test]
async fn imports_integration_schema_inputs_are_substituted_before_merge() {
    let repo = temp_repo();
    let workflow_dir = repo.path().join("workflows");
    fs::create_dir_all(&workflow_dir).expect("create workflow dir");
    write_component(
        &workflow_dir,
        "components/deploy.md",
        r#"---
import-schema:
  destination:
    type: string
    required: true
safe-outputs:
  deploy:
    run: "deploy --to ${{ ado.aw.import-inputs.destination }}"
    env:
      DESTINATION: "${{ ado.aw.import-inputs.destination }}"
---
Deploy to ${{ ado.aw.import-inputs.destination }}.
"#,
    );
    let workflow_path = workflow_dir.join("agent.md");
    fs::write(
        &workflow_path,
        r#"---
name: consumer
description: consumer workflow
imports:
  - uses: components/deploy.md
    with:
      destination: prod-west
---
Consumer body.
"#,
    )
    .expect("write workflow");

    let (mut consumer_fm, consumer_body, entries) = parse_workflow(&workflow_path);
    let resolved = resolve_local(&entries, &workflow_dir).await;
    let merged_body =
        merge_resolved(&mut consumer_fm, &consumer_body, &resolved).expect("merge imports");

    assert!(!consumer_fm.contains_key(key("import-schema")));
    let safe_outputs = map_get_mapping(&consumer_fm, "safe-outputs");
    let deploy = value_as_mapping(
        safe_outputs
            .get(key("deploy"))
            .expect("deploy safe-output present"),
    );
    assert_eq!(
        map_get(deploy, "run"),
        &Value::String("deploy --to prod-west".into())
    );
    let env = map_get_mapping(deploy, "env");
    assert_eq!(
        map_get(env, "DESTINATION"),
        &Value::String("prod-west".into())
    );
    assert_eq!(merged_body, "Deploy to prod-west.\n\nConsumer body.");
}

#[test]
fn imports_integration_remote_specs_must_be_sha_pinned() {
    let err = ImportEntry {
        uses: "o/r/p.md@main".to_string(),
        with: serde_json::Map::new(),
        endpoint: None,
    }
    .parse_source()
    .expect_err("branch refs must be rejected");

    assert!(
        err.to_string().contains("full 40-character commit SHA"),
        "{err}"
    );
}

#[tokio::test]
async fn imports_integration_merge_conflicts_and_safe_output_configuration() {
    let repo = temp_repo();
    let workflow_dir = repo.path().join("workflows");
    fs::create_dir_all(&workflow_dir).expect("create workflow dir");
    write_component(
        &workflow_dir,
        "tool-one.md",
        "---\ntools:\n  edit: {}\n---\none\n",
    );
    write_component(
        &workflow_dir,
        "tool-two.md",
        "---\ntools:\n  edit: {}\n---\ntwo\n",
    );
    let duplicate_tools = resolve_local(
        &[import_entry("tool-one.md"), import_entry("tool-two.md")],
        &workflow_dir,
    )
    .await;
    let err = merge_resolved(&mut ymap("name: consumer"), "", &duplicate_tools)
        .expect_err("duplicate imported tools should fail");
    assert!(err.to_string().contains("tools.edit"), "{err}");

    write_component(
        &workflow_dir,
        "notify.md",
        "---\nsafe-outputs:\n  notify:\n    run: notify.js\n---\nnotify\n",
    );
    let notify = resolve_local(&[import_entry("notify.md")], &workflow_dir).await;
    let err = merge_resolved(
        &mut ymap("safe-outputs:\n  notify:\n    run: consumer.js"),
        "",
        &notify,
    )
    .expect_err("consumer executor redefinition should fail");
    assert!(err.to_string().contains("executor"), "{err}");

    let mut consumer = ymap("safe-outputs:\n  notify:\n    require-approval: true");
    merge_resolved(&mut consumer, "", &notify).expect("configuration overlay should succeed");
    let safe_outputs = map_get_mapping(&consumer, "safe-outputs");
    let notify_cfg = value_as_mapping(
        safe_outputs
            .get(key("notify"))
            .expect("notify safe-output present"),
    );
    assert_eq!(
        map_get(notify_cfg, "run"),
        &Value::String("notify.js".into())
    );
    assert_eq!(map_get(notify_cfg, "require-approval"), &Value::Bool(true));
}

#[tokio::test]
async fn imports_integration_resolve_enforces_import_count_limit() {
    let repo = temp_repo();
    let entries: Vec<ImportEntry> = (0..21)
        .map(|idx| import_entry(&format!("missing-{idx}.md?")))
        .collect();

    let err = resolve_imports(&entries, repo.path(), &PanicFetcher)
        .await
        .expect_err("more than 20 imports should fail before resolution");

    assert!(
        err.to_string()
            .contains("imports per workflow must be <= 20"),
        "{err}"
    );
}

#[test]
fn imports_integration_remote_alias_synthesis_and_template_diagnostic() {
    let import = ResolvedImport {
        entry: ImportEntry {
            uses: format!("octo/components/deploy.md@{SHA}"),
            with: serde_json::Map::new(),
            endpoint: Some(ImportEndpoint::GitHub {
                name: "github-service-connection".to_string(),
            }),
        },
        source: ImportSource::Remote(ParsedImportSpec {
            owner: "octo".to_string(),
            repo: "components".to_string(),
            path: "deploy.md".to_string(),
            sha: CommitSha::parse(SHA).expect("valid sha"),
            section: None,
            optional: false,
            endpoint: Some(ImportEndpoint::GitHub {
                name: "github-service-connection".to_string(),
            }),
        }),
        front_matter: Value::Null,
        body: String::new(),
        provenance: ImportProvenance {
            source: "octo/components/deploy.md".to_string(),
            sha: Some(SHA.to_string()),
            manifest_digest: "digest".to_string(),
        },
    };

    let repos = synthesize_repo_aliases(&[import]).expect("synthesize aliases");

    assert_eq!(repos.len(), 1);
    assert_eq!(repos[0].repo_type, "github");
    assert_eq!(
        repos[0].endpoint.as_deref(),
        Some("github-service-connection")
    );
    assert_eq!(repos[0].name, "octo/components");
    let aliases = repos
        .iter()
        .map(|repo| repo.repository.clone())
        .collect::<Vec<_>>();
    let diagnostic = import_resource_parent_diagnostic(CompileTarget::Job, &aliases)
        .expect("job template target should report parent resource diagnostic");
    assert!(diagnostic.contains(&aliases[0]), "{diagnostic}");
    assert!(diagnostic.contains("parent pipeline"), "{diagnostic}");
}
