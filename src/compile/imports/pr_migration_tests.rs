use super::*;
use crate::compile::{self, common, types::FrontMatter};
use serde_json::json;
use serde_yaml::{Mapping, Value};

fn workflow(imports: &str, outputs: &str) -> String {
    format!(
        "---\nname: imported-pr\ndescription: Imported PR policy\n{imports}safe-outputs:\n{outputs}---\n\nConsumer body.\n  Unchanged bytes. \n"
    )
}

fn local_workflow(repo: &Path, component: &str, outputs: &str) -> PathBuf {
    write(&repo.join("component.md"), component);
    let source = repo.join("agent.md");
    write(
        &source,
        &workflow("imports:\n  - ./component.md\n", outputs),
    );
    source
}

async fn compile_source(path: &Path) -> Result<bool> {
    compile::compile_pipeline_with_registry(
        &path.to_string_lossy(),
        None,
        false,
        false,
        compile::codemods::CODEMODS,
    )
    .await
}

#[tokio::test]
async fn imported_old_comment_and_update_pr_match_root_capabilities_without_rewrites() {
    let repo = temp_repo();
    let outputs = "  add-pr-comment:\n    max: 2\n  update-pr:\n    max: 3\n    allowed-operations: [add-labels, add-reviewers]\n";
    let component = format!("---\nsafe-outputs:\n{outputs}---\nImported old update-pr prompt.\n");
    let source = local_workflow(repo.path(), &component, "  noop:\n");
    let before = fs::read(&source).unwrap();
    let (imported, _) = compile::build_pipeline_ir(&source).await.unwrap();
    let root = common::parse_markdown_detailed(&workflow("", &format!("{outputs}  noop:\n")))
        .unwrap()
        .front_matter;
    assert_eq!(imported.safe_outputs, root.safe_outputs);
    assert!(!compile_source(&source).await.unwrap());
    compile::check_pipeline(&source.with_extension("lock.yml").to_string_lossy())
        .await
        .unwrap();
    assert_eq!(fs::read(&source).unwrap(), before);
    assert_eq!(
        fs::read_to_string(repo.path().join("component.md")).unwrap(),
        component
    );
}

#[tokio::test]
async fn aliases_obey_consumer_precedence_in_both_directions() {
    for (imported, consumer) in [
        ("add-pr-comment", "add-pull-request-comment"),
        ("add-pull-request-comment", "add-pr-comment"),
    ] {
        let repo = temp_repo();
        let component = format!("---\nsafe-outputs:\n  {imported}:\n    max: 5\n---\nComponent");
        let source = local_workflow(
            repo.path(),
            &component,
            &format!("  {consumer}:\n    max: 1\n"),
        );
        let (fm, _) = compile::build_pipeline_ir(&source).await.unwrap();
        assert_eq!(fm.safe_outputs["add-pull-request-comment"]["max"], 1);
        assert!(!fm.safe_outputs.contains_key("add-pr-comment"));
        compile_source(&source).await.unwrap();
        let (again, _) = compile::build_pipeline_ir(&source).await.unwrap();
        assert_eq!(again.safe_outputs, fm.safe_outputs);
        assert_eq!(
            fs::read_to_string(repo.path().join("component.md")).unwrap(),
            component
        );
    }
}

#[tokio::test]
async fn nested_local_schema_policy_uses_shared_read_only_preparation() {
    let repo = temp_repo();
    let child = "---\nimport-schema:\n  operation:\n    type: string\n    required: true\nsafe-outputs:\n  update-pr:\n    allowed-operations: ['{{ inputs.operation }}']\n---\nChild {{ inputs.operation }}.\n";
    write(&repo.path().join("nested").join("child.md"), child);
    let parent = "---\nimports:\n  - uses: ./nested/child.md\n    with:\n      operation: add-reviewers\n---\nParent.\n";
    let source = local_workflow(repo.path(), parent, "  noop:\n");
    let before = fs::read(&source).unwrap();
    let effective = compile::prepare_source_front_matter(&source).await.unwrap();
    let (inspected, _) = compile::build_pipeline_ir(&source).await.unwrap();
    assert_eq!(effective.safe_outputs, inspected.safe_outputs);
    assert!(
        effective
            .safe_outputs
            .contains_key("add-pull-request-reviewers")
    );
    assert!(
        !effective
            .safe_outputs
            .contains_key("add-pull-request-labels")
    );
    crate::inspect::build_lint(&source).await.unwrap();
    assert!(!source.with_extension("lock.yml").exists());
    assert_eq!(fs::read(&source).unwrap(), before);
    assert_eq!(
        fs::read_to_string(repo.path().join("nested").join("child.md")).unwrap(),
        child
    );
    assert_eq!(
        fs::read_to_string(repo.path().join("component.md")).unwrap(),
        parent
    );
}

#[tokio::test]
async fn duplicate_alias_imports_fail_even_with_consumer_override() {
    let repo = temp_repo();
    write(
        &repo.path().join("a.md"),
        "---\nsafe-outputs:\n  add-pr-comment:\n---\nA",
    );
    write(
        &repo.path().join("b.md"),
        "---\nsafe-outputs:\n  add-pull-request-comment:\n---\nB",
    );
    let source = repo.path().join("agent.md");
    let content = workflow(
        "imports:\n  - ./a.md\n  - ./b.md\n",
        "  add-pr-comment:\n    max: 1\n",
    );
    write(&source, &content);
    let error = format!("{:#}", compile_source(&source).await.unwrap_err());
    assert!(error.contains("import conflict"), "{error}");
    assert!(
        error.contains("add-pull-request-comment")
            && error.contains("a.md")
            && error.contains("b.md"),
        "{error}"
    );
    assert_eq!(fs::read_to_string(source).unwrap(), content);
}

#[tokio::test]
async fn same_source_alias_conflicts_fail_atomically_for_root_and_import() {
    let conflict = "  add-pr-comment:\n  add-pull-request-comment:\n";
    for root_conflict in [false, true] {
        let repo = temp_repo();
        let component = format!(
            "---\nsafe-outputs:\n{}---\nComponent",
            if root_conflict { "  noop:\n" } else { conflict }
        );
        let source = local_workflow(
            repo.path(),
            &component,
            if root_conflict { conflict } else { "  noop:\n" },
        );
        let original = fs::read(&source).unwrap();
        let error = format!("{:#}", compile_source(&source).await.unwrap_err());
        assert!(
            error.contains("both add-pr-comment and add-pull-request-comment"),
            "{error}"
        );
        assert_eq!(fs::read(&source).unwrap(), original);
        assert_eq!(
            fs::read_to_string(repo.path().join("component.md")).unwrap(),
            component
        );
        assert!(!source.with_extension("lock.yml").exists());
    }
}

#[tokio::test]
async fn invalid_winning_import_policy_reports_origin_without_rewriting_files() {
    let repo = temp_repo();
    let component = "---\nsafe-outputs:\n  update-pr:\n    allowed-operations: [vote]\n    allowed-votes: [comment]\n---\nImported prompt";
    let source = local_workflow(repo.path(), component, "  add-pr-comment:\n");
    let original = fs::read(&source).unwrap();
    let error = format!("{:#}", compile_source(&source).await.unwrap_err());
    assert!(
        error.contains("component.md") && error.contains("allowed-votes"),
        "{error}"
    );
    assert_eq!(fs::read(source).unwrap(), original);
    assert_eq!(
        fs::read_to_string(repo.path().join("component.md")).unwrap(),
        component
    );
}

#[tokio::test]
async fn whole_family_override_survives_root_rewrite_and_second_compile() {
    let repo = temp_repo();
    // The invalid vote default is irrelevant: the consumer replaces this whole declaration.
    let component = "---\nsafe-outputs:\n  update-pr:\n    allowed-operations: [add-reviewers, add-labels, vote]\n    allowed-votes: [comment]\n    max: 5\n---\nImported prompt.\n";
    let source = local_workflow(
        repo.path(),
        component,
        "  update-pr:\n    max: 1\n    allowed-operations: [add-reviewers]\n    allowed-reviewers: [alice, bob]\n",
    );
    let original = fs::read_to_string(&source).unwrap();
    let body = common::split_markdown_front_matter(&original, true)
        .unwrap()
        .body_raw;
    let (before, _) = compile::build_pipeline_ir(&source).await.unwrap();
    assert!(
        before
            .safe_outputs
            .contains_key("add-pull-request-reviewers")
    );
    assert!(!before.safe_outputs.contains_key("add-pull-request-labels"));
    assert!(
        !before
            .safe_outputs
            .contains_key("submit-pull-request-review")
    );
    assert_eq!(
        before.safe_outputs["budget-groups"]["update-pr"],
        json!({"max": 1, "tools": ["add-pull-request-reviewers"]})
    );

    assert!(compile_source(&source).await.unwrap());
    let rewritten = fs::read_to_string(&source).unwrap();
    assert_eq!(
        common::split_markdown_front_matter(&rewritten, true)
            .unwrap()
            .body_raw,
        body
    );
    assert!(!rewritten.contains("allowed-votes"));
    let (after, _) = compile::build_pipeline_ir(&source).await.unwrap();
    assert_eq!(before.safe_outputs, after.safe_outputs);
    assert!(!compile_source(&source).await.unwrap());
    compile::check_pipeline(&source.with_extension("lock.yml").to_string_lossy())
        .await
        .unwrap();
    assert_eq!(fs::read_to_string(&source).unwrap(), rewritten);
    assert_eq!(
        fs::read_to_string(repo.path().join("component.md")).unwrap(),
        component
    );

    // Narrowing a migrated child must not be overwritten by its retained legacy metadata.
    let mut parsed = common::parse_markdown_detailed(&rewritten).unwrap();
    parsed.front_matter_mapping["safe-outputs"]["add-pull-request-reviewers"]["max"] =
        Value::from(0);
    let narrowed = common::reconstruct_source(
        &parsed.leading_whitespace,
        &parsed.front_matter_mapping,
        &parsed.body_raw,
    )
    .unwrap();
    write(&source, &narrowed);
    let effective = compile::prepare_source_front_matter(&source).await.unwrap();
    assert_eq!(
        effective.safe_outputs["add-pull-request-reviewers"]["max"],
        0
    );
    assert_eq!(
        effective.safe_outputs["add-pull-request-reviewers"]["legacy-update-pr"]["max"],
        1
    );
    assert!(
        !effective
            .safe_outputs
            .contains_key("add-pull-request-labels")
    );
}

#[tokio::test]
async fn migrated_import_family_replacement_retains_independent_caps() {
    let repo = temp_repo();
    let imported = common::parse_markdown_detailed(
        &workflow("", "  update-pr:\n    max: 5\n    allowed-operations: [add-reviewers, add-labels]\n  set-pull-request-auto-complete:\n    max: 8\n  budget-groups:\n    independent:\n      max: 2\n      tools: [set-pull-request-auto-complete]\n"),
    ).unwrap();
    let component =
        common::reconstruct_source("", &imported.front_matter_mapping, &imported.body_raw).unwrap();
    let source = local_workflow(
        repo.path(),
        &component,
        "  update-pr:\n    max: 1\n    allowed-operations: [add-reviewers]\n",
    );
    let (first, _) = compile::build_pipeline_ir(&source).await.unwrap();
    assert!(!first.safe_outputs.contains_key("add-pull-request-labels"));
    assert_eq!(
        first.safe_outputs["budget-groups"]["independent"],
        json!({"max": 2, "tools": ["set-pull-request-auto-complete"]})
    );
    assert_eq!(
        first.safe_outputs["budget-groups"]["update-pr"],
        json!({"max": 1, "tools": ["add-pull-request-reviewers"]})
    );
    assert!(compile_source(&source).await.unwrap());
    let (second, _) = compile::build_pipeline_ir(&source).await.unwrap();
    assert_eq!(first.safe_outputs, second.safe_outputs);
    assert!(!compile_source(&source).await.unwrap());
    assert_eq!(
        fs::read_to_string(repo.path().join("component.md")).unwrap(),
        component
    );
}

#[tokio::test]
async fn unrelated_imported_and_root_budget_groups_are_preserved() {
    let repo = temp_repo();
    write(
        &repo.path().join("a.md"),
        "---\nsafe-outputs:\n  add-pr-labels:\n    max: 4\n  budget-groups:\n    labels:\n      max: 2\n      tools: [add-pr-labels]\n---\nA",
    );
    write(
        &repo.path().join("b.md"),
        "---\nsafe-outputs:\n  set-pr-auto-complete:\n    max: 4\n  budget-groups:\n    auto:\n      max: 3\n      tools: [set-pr-auto-complete]\n---\nB",
    );
    let source = repo.path().join("agent.md");
    write(
        &source,
        &workflow(
            "imports:\n  - ./a.md\n  - ./b.md\n",
            "  update-pr:\n    allowed-operations: [add-reviewers]\n    max: 1\n",
        ),
    );
    let (first, _) = compile::build_pipeline_ir(&source).await.unwrap();
    let groups = &first.safe_outputs["budget-groups"];
    assert_eq!(groups.as_object().unwrap().len(), 3);
    assert_eq!(
        groups["labels"],
        json!({"max": 2, "tools": ["add-pull-request-labels"]})
    );
    assert_eq!(
        groups["auto"],
        json!({"max": 3, "tools": ["set-pull-request-auto-complete"]})
    );
    assert_eq!(
        groups["update-pr"],
        json!({"max": 1, "tools": ["add-pull-request-reviewers"]})
    );
    compile_source(&source).await.unwrap();
    let (second, _) = compile::build_pipeline_ir(&source).await.unwrap();
    assert_eq!(first.safe_outputs, second.safe_outputs);
}

#[tokio::test]
async fn duplicate_legacy_families_conflict_across_raw_and_migrated_imports() {
    let repo = temp_repo();
    let raw = "---\nname: x\ndescription: x\nsafe-outputs:\n  update-pr:\n    allowed-operations: [add-reviewers]\n---\nBody";
    let parsed = common::parse_markdown_detailed(raw).unwrap();
    write(&repo.path().join("a.md"), raw);
    let migrated =
        common::reconstruct_source("", &parsed.front_matter_mapping, &parsed.body_raw).unwrap();
    write(&repo.path().join("b.md"), &migrated);
    let source = repo.path().join("agent.md");
    write(
        &source,
        &workflow("imports:\n  - ./a.md\n  - ./b.md\n", "  noop:\n"),
    );
    let error = format!("{:#}", compile_source(&source).await.unwrap_err());
    assert!(
        error.contains("import conflict") && error.contains("legacy family"),
        "{error}"
    );
}

#[tokio::test]
async fn duplicate_budget_group_names_report_both_component_origins() {
    let repo = temp_repo();
    write(
        &repo.path().join("a.md"),
        "---\nsafe-outputs:\n  add-pr-labels:\n  budget-groups:\n    shared:\n      max: 1\n      tools: [add-pr-labels]\n---\nA",
    );
    write(
        &repo.path().join("b.md"),
        "---\nsafe-outputs:\n  add-pr-reviewers:\n  budget-groups:\n    shared:\n      max: 2\n      tools: [add-pr-reviewers]\n---\nB",
    );
    let source = repo.path().join("agent.md");
    write(
        &source,
        &workflow("imports:\n  - ./a.md\n  - ./b.md\n", "  noop:\n"),
    );
    let error = format!("{:#}", compile_source(&source).await.unwrap_err());
    assert!(
        error.contains("safe-outputs.budget-groups.shared"),
        "{error}"
    );
    assert!(error.contains("a.md") && error.contains("b.md"), "{error}");
}

#[tokio::test]
async fn ambiguous_migrated_family_metadata_is_rejected_without_rewrite() {
    for tools in ["[add-pull-request-labels]", "[]"] {
        let repo = temp_repo();
        let source = local_workflow(
            repo.path(),
            "---\n{}\n---\nComponent",
            &format!(
                "  add-pull-request-reviewers:\n    legacy-update-pr: {{allowed-operations: [add-reviewers]}}\n  budget-groups:\n    update-pr:\n      max: 1\n      tools: {tools}\n"
            ),
        );
        let original = fs::read(&source).unwrap();
        let error = format!("{:#}", compile_source(&source).await.unwrap_err());
        assert!(
            error.contains("ambiguous legacy update-pr family"),
            "{error}"
        );
        assert_eq!(fs::read(source).unwrap(), original);
    }
}

#[tokio::test]
async fn custom_legacy_job_names_and_consumer_policies_are_not_migrated() {
    for name in ["update-pr", "add-pr-comment"] {
        let repo = temp_repo();
        let component = format!(
            "---\nsafe-outputs:\n  jobs:\n    {name}:\n      description: Custom operation\n      steps:\n        - bash: echo custom\n  {name}:\n    max: 5\n---\nComponent\n"
        );
        let source = local_workflow(repo.path(), &component, &format!("  {name}:\n    max: 1\n"));
        let original = fs::read(&source).unwrap();
        let (fm, _) = compile::build_pipeline_ir(&source).await.unwrap();
        assert_eq!(fm.custom_safe_output_tool_names(), vec![name.to_string()]);
        assert_eq!(fm.safe_outputs[name], json!({"max": 1}));
        assert!(fm.safe_outputs["jobs"].get(name).is_some());
        assert!(!fm.safe_outputs.contains_key("budget-groups"));
        assert!(!compile_source(&source).await.unwrap());
        assert_eq!(fs::read(source).unwrap(), original);
        assert_eq!(
            fs::read_to_string(repo.path().join("component.md")).unwrap(),
            component
        );
    }
}

#[tokio::test]
async fn mixed_custom_ownership_survives_unrelated_builtin_root_rewrite() {
    let repo = temp_repo();
    let component = "---\nsafe-outputs:\n  jobs:\n    update-pr:\n      description: Custom update\n      steps:\n        - bash: echo update-pr\n---\nImported custom job";
    let source = local_workflow(
        repo.path(),
        component,
        "  jobs:\n    add-pr-labels:\n      description: Custom labels\n      steps:\n        - bash: echo add-pr-labels\n  add-pr-labels:\n    max: 3\n  update-pr:\n    max: 2\n  add-pr-comment:\n    max: 1\n",
    );
    let (before, _) = compile::build_pipeline_ir(&source).await.unwrap();
    assert!(compile_source(&source).await.unwrap());
    let (after, _) = compile::build_pipeline_ir(&source).await.unwrap();
    assert_eq!(before.safe_outputs, after.safe_outputs);
    assert_eq!(after.safe_outputs["update-pr"], json!({"max": 2}));
    assert_eq!(after.safe_outputs["add-pr-labels"], json!({"max": 3}));
    assert_eq!(
        after.safe_outputs["add-pull-request-comment"],
        json!({"max": 1})
    );
    let rewritten = fs::read_to_string(&source).unwrap();
    assert!(rewritten.contains("echo add-pr-labels"));
    assert!(!rewritten.contains("add-pull-request-labels"));
    assert!(!compile_source(&source).await.unwrap());
}

#[tokio::test]
async fn import_free_custom_legacy_names_keep_their_definitions_and_policies() {
    let repo = temp_repo();
    let source = repo.path().join("agent.md");
    let content = workflow(
        "",
        "  jobs:\n    update-pr:\n      description: Custom update\n      steps:\n        - bash: echo custom\n    add-pr-comment:\n      description: Custom comment\n      steps:\n        - bash: echo comment\n  update-pr:\n    max: 2\n  add-pr-comment:\n    max: 1\n",
    );
    write(&source, &content);
    assert!(!compile_source(&source).await.unwrap());
    assert_eq!(fs::read_to_string(&source).unwrap(), content);
    let fm = compile::prepare_source_front_matter(&source).await.unwrap();
    assert_eq!(fm.safe_outputs["update-pr"], json!({"max": 2}));
    assert_eq!(fm.safe_outputs["add-pr-comment"], json!({"max": 1}));
}

#[tokio::test]
async fn canonical_custom_job_collisions_remain_explicit_errors() {
    let repo = temp_repo();
    let component = "---\nsafe-outputs:\n  jobs:\n    add-pull-request-comment:\n      steps:\n        - bash: echo custom\n---\nComponent";
    let source = local_workflow(repo.path(), component, "  add-pr-comment:\n");
    let original = fs::read(&source).unwrap();
    let error = format!("{:#}", compile_source(&source).await.unwrap_err());
    assert!(
        error.contains("collides") && error.contains("add-pull-request-comment"),
        "{error}"
    );
    assert_eq!(fs::read(source).unwrap(), original);
}

#[tokio::test]
async fn nested_schema_inputs_migrate_after_substitution_with_unchanged_offline_cache() {
    let repo = temp_repo();
    let parent = "---\nimports:\n  - uses: ./child.md\n    with:\n      limit: 3\n      operation: add-labels\n---\nParent prompt.\n";
    let child = "---\nimport-schema:\n  limit:\n    type: number\n    required: true\n  operation:\n    type: string\n    required: true\nsafe-outputs:\n  add-pr-comment:\n    max: 3\n  update-pr:\n    max: 3\n    allowed-operations: ['{{ inputs.operation }}']\n---\nChild {{ inputs.operation }} prompt, limit {{ inputs.limit }}.\n";
    let fetcher = FakeFetcher::default()
        .with_manifest("components/parent.md", parent)
        .with_manifest("components/child.md", child);
    let entries = vec![remote_entry(
        &format!("components/parent.md@{SHA}"),
        "project/shared",
    )];
    let first = resolve_imports_with_repo_root(&entries, repo.path(), repo.path(), &fetcher)
        .await
        .unwrap();
    let snapshot = cache_snapshot(repo.path());
    let mut consumer: Mapping = serde_yaml::from_str("name: test\ndescription: test").unwrap();
    let body = merge_resolved(&mut consumer, "Consumer prompt.", &first).unwrap();
    let fm: FrontMatter = serde_yaml::from_value(Value::Mapping(consumer.clone())).unwrap();
    common::validate_safe_outputs_keys(&fm).unwrap();
    common::validate_pull_request_outputs_config(&fm).unwrap();
    assert_eq!(fm.safe_outputs["add-pull-request-comment"]["max"], 3);
    assert_eq!(fm.safe_outputs["add-pull-request-labels"]["max"], 3);
    assert_eq!(
        body,
        "Parent prompt.\n\nChild add-labels prompt, limit 3.\n\nConsumer prompt."
    );
    assert_eq!(fetcher.fetch_calls.load(Ordering::SeqCst), 2);
    let cached =
        resolve_imports_with_repo_root(&entries, repo.path(), repo.path(), &OfflineFetcher)
            .await
            .unwrap();
    let mut again: Mapping = serde_yaml::from_str("name: test\ndescription: test").unwrap();
    assert_eq!(
        merge_resolved(&mut again, "Consumer prompt.", &cached).unwrap(),
        body
    );
    assert_eq!(again, consumer);
    assert_eq!(cache_snapshot(repo.path()), snapshot);
}

fn cache_snapshot(root: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
    fn visit(path: &Path, out: &mut std::collections::BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(&path, out);
            } else {
                out.insert(path.clone(), fs::read(path).unwrap());
            }
        }
    }
    let mut snapshot = Default::default();
    visit(&root.join(".ado-aw"), &mut snapshot);
    snapshot
}
