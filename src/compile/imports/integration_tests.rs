use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::Result;

use super::{
    ImportNotFound, ManifestFetcher, RemoteRepositoryIdentity, ResolvedImport,
    resolve_imports_with_repo_root,
};
use crate::compile::imports::merge::merge_resolved;
use crate::compile::types::{ImportEntry, ParsedImportSpec};
use crate::secure::CommitSha;

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";
const SHA2: &str = "89abcdef0123456789abcdef0123456789abcdef";

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

fn entry(uses: &str) -> ImportEntry {
    ImportEntry {
        uses: uses.to_string(),
        with: Default::default(),
        repository: None,
        source: None,
    }
}

fn remote_entry(uses: &str, repository: &str) -> ImportEntry {
    ImportEntry {
        uses: uses.to_string(),
        with: Default::default(),
        repository: Some(repository.to_string()),
        source: None,
    }
}

fn write(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

#[derive(Default)]
struct FakeFetcher {
    manifests: HashMap<String, Vec<u8>>,
    resolve_calls: AtomicUsize,
    fetch_calls: AtomicUsize,
}

impl FakeFetcher {
    fn with_manifest(mut self, path: &str, manifest: &str) -> Self {
        self.manifests
            .insert(path.to_string(), manifest.as_bytes().to_vec());
        self
    }
}

#[async_trait::async_trait]
impl ManifestFetcher for FakeFetcher {
    async fn resolve_ref(&self, spec: &ParsedImportSpec) -> Result<CommitSha> {
        self.resolve_calls.fetch_add(1, Ordering::SeqCst);
        let ParsedImportSpec::Remote { requested_ref, .. } = spec else {
            panic!("remote fetcher received local import");
        };
        match requested_ref.as_str() {
            "missing" => Err(anyhow::Error::new(ImportNotFound::new("missing ref"))),
            "broken" => anyhow::bail!("authentication failed"),
            _ => CommitSha::parse(SHA),
        }
    }

    async fn fetch(&self, spec: &ParsedImportSpec, _sha: &CommitSha) -> Result<Vec<u8>> {
        self.fetch_calls.fetch_add(1, Ordering::SeqCst);
        let ParsedImportSpec::Remote { path, .. } = spec else {
            panic!("remote fetcher received local import");
        };
        self.manifests
            .get(path)
            .cloned()
            .ok_or_else(|| anyhow::Error::new(ImportNotFound::new(path)))
    }
}

struct OfflineFetcher;

#[async_trait::async_trait]
impl ManifestFetcher for OfflineFetcher {
    async fn resolve_ref(&self, _spec: &ParsedImportSpec) -> Result<CommitSha> {
        anyhow::bail!("offline")
    }

    async fn fetch(&self, _spec: &ParsedImportSpec, _sha: &CommitSha) -> Result<Vec<u8>> {
        panic!("committed cache should satisfy offline fetch")
    }
}

struct MovingMissingFetcher;

#[async_trait::async_trait]
impl ManifestFetcher for MovingMissingFetcher {
    async fn resolve_ref(&self, _spec: &ParsedImportSpec) -> Result<CommitSha> {
        CommitSha::parse(SHA2)
    }

    async fn fetch(&self, _spec: &ParsedImportSpec, _sha: &CommitSha) -> Result<Vec<u8>> {
        Err(anyhow::Error::new(ImportNotFound::new(
            "manifest removed at new ref",
        )))
    }
}

struct MissingRefFetcher;

#[async_trait::async_trait]
impl ManifestFetcher for MissingRefFetcher {
    async fn resolve_ref(&self, _spec: &ParsedImportSpec) -> Result<CommitSha> {
        Err(anyhow::Error::new(ImportNotFound::new(
            "ref no longer exists",
        )))
    }

    async fn fetch(&self, _spec: &ParsedImportSpec, _sha: &CommitSha) -> Result<Vec<u8>> {
        panic!("missing refs must not fetch manifests")
    }
}

struct InaccessibleRepositoryFetcher;

#[async_trait::async_trait]
impl ManifestFetcher for InaccessibleRepositoryFetcher {
    async fn ensure_repository_accessible(&self, _spec: &ParsedImportSpec) -> Result<()> {
        anyhow::bail!("repository access could not be verified")
    }

    async fn resolve_ref(&self, _spec: &ParsedImportSpec) -> Result<CommitSha> {
        panic!("inaccessible repositories must fail before ref resolution")
    }

    async fn fetch(&self, _spec: &ParsedImportSpec, _sha: &CommitSha) -> Result<Vec<u8>> {
        panic!("inaccessible repositories must not fetch manifests")
    }
}

struct ContextualFetcher {
    source: &'static str,
    project: &'static str,
    body: &'static str,
}

#[async_trait::async_trait]
impl ManifestFetcher for ContextualFetcher {
    async fn remote_identity(&self, _spec: &ParsedImportSpec) -> Result<RemoteRepositoryIdentity> {
        Ok(RemoteRepositoryIdentity {
            source: self.source.to_string(),
            project: Some(self.project.to_string()),
        })
    }

    async fn resolve_ref(&self, _spec: &ParsedImportSpec) -> Result<CommitSha> {
        CommitSha::parse(SHA)
    }

    async fn fetch(&self, _spec: &ParsedImportSpec, _sha: &CommitSha) -> Result<Vec<u8>> {
        Ok(format!("---\n{{}}\n---\n{}", self.body).into_bytes())
    }
}

struct PanicFetcher;

#[async_trait::async_trait]
impl ManifestFetcher for PanicFetcher {
    async fn resolve_ref(&self, _spec: &ParsedImportSpec) -> Result<CommitSha> {
        panic!("local imports must not resolve remote refs")
    }

    async fn fetch(&self, _spec: &ParsedImportSpec, _sha: &CommitSha) -> Result<Vec<u8>> {
        panic!("local imports must not fetch remote manifests")
    }
}

#[tokio::test]
async fn nested_local_imports_use_breadth_first_order_and_relative_origins() {
    let repo = temp_repo();
    write(
        &repo.path().join("a.md"),
        "---\nimports:\n  - ./nested/child.md\n---\nA",
    );
    write(&repo.path().join("b.md"), "---\n{}\n---\nB");
    write(
        &repo.path().join("nested/child.md"),
        "---\ntools:\n  edit: true\n---\nChild",
    );

    let resolved = resolve_imports_with_repo_root(
        &[entry("./a.md"), entry("./b.md")],
        repo.path(),
        repo.path(),
        &PanicFetcher,
    )
    .await
    .unwrap();
    assert_eq!(
        resolved
            .iter()
            .map(|import| import.body.as_str())
            .collect::<Vec<_>>(),
        ["A", "B", "Child"]
    );

    let mut consumer = serde_yaml::from_str("name: c").unwrap();
    let body = merge_resolved(&mut consumer, "Consumer", &resolved).unwrap();
    assert_eq!(body, "A\n\nB\n\nChild\n\nConsumer");
    assert_eq!(consumer["tools"]["edit"], true);
}

#[tokio::test]
async fn local_import_base_must_be_inside_canonical_repository_root() {
    let repo = temp_repo();
    let outside = temp_repo();
    write(
        &outside.path().join("component.md"),
        "---\n{}\n---\nOutside",
    );

    let error = resolve_imports_with_repo_root(
        &[entry("./component.md")],
        outside.path(),
        repo.path(),
        &PanicFetcher,
    )
    .await
    .unwrap_err();

    assert!(
        format!("{error:#}").contains("outside repository root"),
        "{error:#}"
    );
}

#[tokio::test]
async fn remote_branch_is_sha_resolved_cached_with_metadata_and_available_offline() {
    let repo = temp_repo();
    let fetcher =
        FakeFetcher::default().with_manifest("components/notify.md", "---\n{}\n---\nRemote");
    let import = remote_entry("components/notify.md@main", "project/shared");

    let first = resolve_imports_with_repo_root(
        std::slice::from_ref(&import),
        repo.path(),
        repo.path(),
        &fetcher,
    )
    .await
    .unwrap();
    assert_eq!(first[0].provenance.sha.as_deref(), Some(SHA));
    assert_eq!(first[0].provenance.requested_ref.as_deref(), Some("main"));
    assert_eq!(fetcher.resolve_calls.load(Ordering::SeqCst), 1);
    assert_eq!(fetcher.fetch_calls.load(Ordering::SeqCst), 1);

    let refs = fs::read_to_string(repo.path().join(".ado-aw/imports/refs.json")).unwrap();
    assert!(refs.contains("\"requested-ref\": \"main\""));
    assert!(refs.contains(SHA));

    let offline =
        resolve_imports_with_repo_root(&[import], repo.path(), repo.path(), &OfflineFetcher)
            .await
            .unwrap();
    assert_eq!(offline[0].body, "Remote");
}

#[tokio::test]
async fn effective_remote_context_partitions_cache_and_ref_metadata() {
    let repo = temp_repo();
    let import = remote_entry("component.md@main", "shared");

    let first = resolve_imports_with_repo_root(
        std::slice::from_ref(&import),
        repo.path(),
        repo.path(),
        &ContextualFetcher {
            source: "azure-repos:https://dev.azure.com/one",
            project: "ProjectOne",
            body: "One",
        },
    )
    .await
    .unwrap();
    assert_eq!(first[0].body, "One");

    let second = resolve_imports_with_repo_root(
        &[import],
        repo.path(),
        repo.path(),
        &ContextualFetcher {
            source: "azure-repos:https://dev.azure.com/two",
            project: "ProjectTwo",
            body: "Two",
        },
    )
    .await
    .unwrap();
    assert_eq!(second[0].body, "Two");

    let refs = fs::read_to_string(repo.path().join(".ado-aw/imports/refs.json")).unwrap();
    assert!(refs.contains("azure-repos:https://dev.azure.com/one"));
    assert!(refs.contains("ProjectOne/shared"));
    assert!(refs.contains("azure-repos:https://dev.azure.com/two"));
    assert!(refs.contains("ProjectTwo/shared"));
}

#[tokio::test]
async fn nested_remote_relative_import_inherits_repo_source_and_resolved_sha() {
    let repo = temp_repo();
    let fetcher = FakeFetcher::default()
        .with_manifest(
            "components/parent.md",
            "---\nimports:\n  - ./nested/deep/child.md@feature/child\n---\nParent",
        )
        .with_manifest("components/nested/deep/child.md", "---\n{}\n---\nChild");

    let resolved = resolve_imports_with_repo_root(
        &[remote_entry(
            "components/parent.md@release/v1",
            "project/shared",
        )],
        repo.path(),
        repo.path(),
        &fetcher,
    )
    .await
    .unwrap();
    assert_eq!(resolved.len(), 2);
    assert_eq!(resolved[1].body, "Child");
    assert_eq!(fetcher.resolve_calls.load(Ordering::SeqCst), 2);
    assert_eq!(fetcher.fetch_calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn cycles_report_the_import_path() {
    let repo = temp_repo();
    write(
        &repo.path().join("a.md"),
        "---\nimports:\n  - ./b.md\n---\nA",
    );
    write(
        &repo.path().join("b.md"),
        "---\nimports:\n  - ./a.md\n---\nB",
    );
    let error =
        resolve_imports_with_repo_root(&[entry("./a.md")], repo.path(), repo.path(), &PanicFetcher)
            .await
            .unwrap_err();
    let message = format!("{error:#}");
    assert!(message.contains("import cycle detected"), "{message}");
    assert!(
        message.contains("a.md") && message.contains("b.md"),
        "{message}"
    );

    let error = resolve_imports_with_repo_root(
        &[entry("./a.md"), entry("./b.md")],
        repo.path(),
        repo.path(),
        &PanicFetcher,
    )
    .await
    .unwrap_err();
    assert!(
        format!("{error:#}").contains("import cycle detected"),
        "{error:#}"
    );
}

#[tokio::test]
async fn duplicate_resolved_files_dedupe_same_inputs_and_reject_different_inputs() {
    let repo = temp_repo();
    write(
        &repo.path().join("component.md"),
        "---\nimport-schema:\n  value:\n    type: string\n    default: x\n---\nBody",
    );
    let same = resolve_imports_with_repo_root(
        &[entry("./component.md"), entry("./component.md")],
        repo.path(),
        repo.path(),
        &PanicFetcher,
    )
    .await
    .unwrap();
    assert_eq!(same.len(), 1);

    let mut one = entry("./component.md");
    one.with.insert("value".into(), "one".into());
    let mut two = entry("./component.md");
    two.with.insert("value".into(), "two".into());
    let error =
        resolve_imports_with_repo_root(&[one, two], repo.path(), repo.path(), &PanicFetcher)
            .await
            .unwrap_err();
    assert!(error.to_string().contains("different `with:` values"));
}

#[tokio::test]
async fn optional_remote_skips_only_typed_not_found() {
    let repo = temp_repo();
    let optional_missing = remote_entry("component.md@missing?", "project/repo");
    let resolved = resolve_imports_with_repo_root(
        &[optional_missing],
        repo.path(),
        repo.path(),
        &FakeFetcher::default(),
    )
    .await
    .unwrap();
    assert!(resolved.is_empty());

    let optional_broken = remote_entry("component.md@broken?", "project/repo");
    let error = resolve_imports_with_repo_root(
        &[optional_broken],
        repo.path(),
        repo.path(),
        &FakeFetcher::default(),
    )
    .await
    .unwrap_err();
    assert!(format!("{error:#}").contains("authentication failed"));

    let optional_inaccessible = remote_entry("component.md@main?", "project/repo");
    let error = resolve_imports_with_repo_root(
        &[optional_inaccessible],
        repo.path(),
        repo.path(),
        &InaccessibleRepositoryFetcher,
    )
    .await
    .unwrap_err();
    assert!(
        format!("{error:#}").contains("repository access could not be verified"),
        "{error:#}"
    );
}

#[tokio::test]
async fn optional_missing_manifest_updates_ref_metadata_instead_of_reviving_stale_cache() {
    let repo = temp_repo();
    let import = remote_entry("component.md@main?", "project/repo");
    let initial = FakeFetcher::default().with_manifest("component.md", "---\n{}\n---\nOld");
    resolve_imports_with_repo_root(
        std::slice::from_ref(&import),
        repo.path(),
        repo.path(),
        &initial,
    )
    .await
    .unwrap();

    let resolved =
        resolve_imports_with_repo_root(&[import], repo.path(), repo.path(), &MovingMissingFetcher)
            .await
            .unwrap();
    assert!(resolved.is_empty());
    let refs = fs::read_to_string(repo.path().join(".ado-aw/imports/refs.json")).unwrap();
    assert!(refs.contains(SHA2));
    assert!(!refs.contains(&format!("\"resolved-sha\": \"{SHA}\"")));
}

#[tokio::test]
async fn missing_remote_ref_tombstones_stale_metadata_and_disables_offline_fallback() {
    let repo = temp_repo();
    let import = remote_entry("component.md@main?", "project/repo");
    let initial = FakeFetcher::default().with_manifest("component.md", "---\n{}\n---\nOld");
    resolve_imports_with_repo_root(
        std::slice::from_ref(&import),
        repo.path(),
        repo.path(),
        &initial,
    )
    .await
    .unwrap();

    let resolved = resolve_imports_with_repo_root(
        std::slice::from_ref(&import),
        repo.path(),
        repo.path(),
        &MissingRefFetcher,
    )
    .await
    .unwrap();
    assert!(resolved.is_empty());

    let refs = fs::read_to_string(repo.path().join(".ado-aw/imports/refs.json")).unwrap();
    assert!(!refs.contains("\"requested-ref\": \"main\""), "{refs}");
    assert!(!refs.contains(SHA), "{refs}");

    let error =
        resolve_imports_with_repo_root(&[import], repo.path(), repo.path(), &OfflineFetcher)
            .await
            .unwrap_err();
    assert!(format!("{error:#}").contains("offline"), "{error:#}");
}

#[tokio::test]
async fn unique_count_depth_and_manifest_size_limits_are_enforced() {
    let repo = temp_repo();
    let mut entries = Vec::new();
    for index in 0..=20 {
        let name = format!("component-{index}.md");
        write(&repo.path().join(&name), "---\n{}\n---\nBody");
        entries.push(entry(&format!("./{name}")));
    }
    let error = resolve_imports_with_repo_root(&entries, repo.path(), repo.path(), &PanicFetcher)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("at most 20 unique imports"));

    let deep = temp_repo();
    for index in 1..=6 {
        let nested = if index < 6 {
            format!("imports:\n  - ./{}.md\n", index + 1)
        } else {
            String::new()
        };
        write(
            &deep.path().join(format!("{index}.md")),
            &format!("---\n{nested}---\n{index}"),
        );
    }
    let error =
        resolve_imports_with_repo_root(&[entry("./1.md")], deep.path(), deep.path(), &PanicFetcher)
            .await
            .unwrap_err();
    assert!(error.to_string().contains("nesting depth exceeds"));

    let large = temp_repo();
    fs::write(large.path().join("large.md"), vec![b'x'; 256 * 1024 + 1]).unwrap();
    let error = resolve_imports_with_repo_root(
        &[entry("./large.md")],
        large.path(),
        large.path(),
        &PanicFetcher,
    )
    .await
    .unwrap_err();
    assert!(format!("{error:#}").contains("262144 byte limit"));
}

#[allow(dead_code)]
fn _assert_resolved_is_send_sync(_: &ResolvedImport) {}
