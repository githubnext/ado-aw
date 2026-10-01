use super::*;

pub(crate) fn command(repo: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

pub(crate) fn repository(files: &[(&str, &[u8])]) -> (tempfile::TempDir, CommitSha) {
    let repo = tempfile::tempdir().unwrap();
    command(repo.path(), &["init", "--quiet", "--initial-branch=main"]);
    command(repo.path(), &["config", "user.name", "Patch Fixture"]);
    command(repo.path(), &["config", "user.email", "patch@example.test"]);
    command(repo.path(), &["config", "core.autocrlf", "false"]);
    for (path, bytes) in files {
        let file = repo.path().join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, bytes).unwrap();
    }
    command(repo.path(), &["add", "."]);
    command(
        repo.path(),
        &["commit", "--quiet", "--allow-empty", "-m", "base"],
    );
    let base = CommitSha::parse(command(repo.path(), &["rev-parse", "HEAD"])).unwrap();
    (repo, base)
}

fn policy(excluded: &[String]) -> PatchPolicy<'_> {
    PatchPolicy {
        limit: Default::default(),
        max_files: 100,
        excluded_files: excluded,
        protected_files: ProtectedFiles::Blocked,
        exact: true,
    }
}

pub(crate) fn movement(kind: &str, source: &str, destination: &str) -> String {
    format!(
        "diff --git a/{source} b/{destination}\nsimilarity index 100%\n{kind} from {source}\n{kind} to {destination}\n"
    )
}

#[test]
fn size_configuration_is_shared_strict_and_measured_in_kib() {
    assert_eq!(PatchSizeKiB::default().bytes(), 4 * 1024 * 1024);
    for value in [1, 4096, 10240] {
        let size: PatchSizeKiB = serde_json::from_value(json!(value)).unwrap();
        assert_eq!(size.bytes(), value * 1024);
        assert_eq!(serde_json::to_value(size).unwrap(), json!(value));
        for tool in ["create", "push"] {
            let config = json!({"max-patch-size":value});
            if tool == "create" {
                assert_eq!(
                    serde_json::from_value::<super::super::create_pull_request::CreatePrConfig>(
                        config
                    )
                    .unwrap()
                    .max_patch_size,
                    size
                );
            } else {
                assert_eq!(
                    serde_json::from_value::<
                        super::super::push_to_pull_request_branch::PushToPullRequestBranchConfig,
                    >(config)
                    .unwrap()
                    .max_patch_size,
                    size
                );
            }
        }
    }
    for value in [
        json!(0),
        json!(10241),
        json!(-1),
        json!(1.5),
        json!(true),
        json!("4096"),
        Value::Null,
    ] {
        assert!(serde_json::from_value::<PatchSizeKiB>(value.clone()).is_err());
        assert!(
            serde_json::from_value::<super::super::create_pull_request::CreatePrConfig>(
                json!({"max-patch-size":value})
            )
            .is_err()
        );
    }
}

#[tokio::test]
async fn native_copies_and_renames_preserve_blobs_and_real_checkout_state() {
    for kind in ["copy", "rename"] {
        let (repo, base) = repository(&[("old name.txt", b"original\n")]);
        let before = command(repo.path(), &["status", "--porcelain"]);
        let index = std::fs::read(repo.path().join(".git").join("index")).unwrap();
        let text = movement(kind, "old name.txt", "new name.txt");
        let prepared = prepare(repo.path(), &base, text.as_bytes(), &policy(&[]))
            .await
            .unwrap();
        let result = prepared.apply_to_index(repo.path(), &base).await.unwrap();
        let added = result
            .changes
            .iter()
            .find(|change| change["item"]["path"] == "/new name.txt")
            .unwrap();
        assert_eq!(added["newContent"]["content"], "original\n");
        assert_eq!(
            result
                .changes
                .iter()
                .any(|change| change["changeType"] == "delete"),
            kind == "rename"
        );
        assert_eq!(
            std::fs::read(repo.path().join(".git").join("index")).unwrap(),
            index
        );
        assert_eq!(command(repo.path(), &["status", "--porcelain"]), before);
        assert_eq!(command(repo.path(), &["rev-parse", "HEAD"]), base.as_str());
        assert!(!repo.path().join("new name.txt").exists());
    }
}

#[tokio::test]
async fn native_swaps_chains_and_copies_use_the_whole_commit_preimage() {
    for binary in [false, true] {
        let left: &[u8] = if binary { b"\0\xffleft" } else { b"left\n" };
        let right: &[u8] = if binary { b"\0\xferight" } else { b"right\n" };
        for swap in [true, false] {
            let (repo, base) = repository(&[("a", left), ("b", right)]);
            let patch = movement("rename", "a", "b")
                + &movement("rename", "b", if swap { "a" } else { "c" })
                + &movement("copy", "a", "copy");
            let prepared = prepare(repo.path(), &base, patch.as_bytes(), &policy(&[]))
                .await
                .unwrap();
            let result = prepared.apply_to_index(repo.path(), &base).await.unwrap();
            for (path, expected) in [
                ("/b", left),
                (if swap { "/a" } else { "/c" }, right),
                ("/copy", left),
            ] {
                let content = &result
                    .changes
                    .iter()
                    .find(|change| change["item"]["path"] == path)
                    .unwrap()["newContent"];
                let actual = if content["contentType"] == "rawtext" {
                    content["content"].as_str().unwrap().as_bytes().to_vec()
                } else {
                    use base64::Engine;
                    base64::engine::general_purpose::STANDARD
                        .decode(content["content"].as_str().unwrap())
                        .unwrap()
                };
                assert_eq!(actual, expected);
            }
        }
    }
}

#[tokio::test]
async fn mailbox_intermediate_sources_are_applied_as_separate_commits() {
    let (repo, base) = repository(&[("a", b"first\n")]);
    let envelope = format!("From {} Mon Sep 17 00:00:00 2001\n", "a".repeat(40));
    let patch =
        envelope.clone() + &movement("rename", "a", "b") + &envelope + &movement("copy", "b", "c");
    let prepared = prepare(repo.path(), &base, patch.as_bytes(), &policy(&[]))
        .await
        .unwrap();
    assert_eq!(prepared.batches.len(), 2);
    let result = prepared.apply_to_index(repo.path(), &base).await.unwrap();
    assert_eq!(result.changes.len(), 3);
    for path in ["/b", "/c"] {
        assert_eq!(
            result
                .changes
                .iter()
                .find(|change| change["item"]["path"] == path)
                .unwrap()["newContent"]["content"],
            "first\n"
        );
    }
    let raw_chain = movement("rename", "a", "b") + &movement("copy", "b", "c");
    assert!(
        prepare(repo.path(), &base, raw_chain.as_bytes(), &policy(&[]))
            .await
            .err()
            .unwrap()
            .to_string()
            .contains("preimage is unavailable")
    );
}

#[tokio::test]
async fn native_copy_amplification_fails_even_at_the_maximum_configured_limit() {
    let (repo, base) = repository(&[("source", &vec![b'x'; 429_575])]);
    let patch = (0..99)
        .map(|index| movement("copy", "source", &format!("copy-{index}")))
        .collect::<String>();
    let mut config = policy(&[]);
    config.limit = PatchSizeKiB::try_from(10240).unwrap();
    let before = std::fs::read(repo.path().join(".git").join("index")).unwrap();
    assert!(
        prepare(repo.path(), &base, patch.as_bytes(), &config)
            .await
            .err()
            .unwrap()
            .to_string()
            .contains("pre-application expansion")
    );
    assert_eq!(
        std::fs::read(repo.path().join(".git").join("index")).unwrap(),
        before
    );
    assert!(!repo.path().join("copy-0").exists());
}

#[tokio::test]
async fn native_movement_exclusions_are_whole_operations_not_git_globs() {
    for (source, destination, pattern) in [
        ("nested/secret.txt", "public.txt", "secret.txt"),
        ("public.txt", "secret.txt", "**/secret.txt"),
    ] {
        for kind in ["copy", "rename"] {
            let (repo, base) = repository(&[(source, b"excluded\n"), ("keep[1].txt", b"old\n")]);
            let text = movement(kind, source, destination)
                + "diff --git a/keep[1].txt b/keep[1].txt\n--- a/keep[1].txt\n+++ b/keep[1].txt\n@@ -1 +1 @@\n-old\n+new\n";
            let excluded = vec![pattern.to_string()];
            let prepared = prepare(repo.path(), &base, text.as_bytes(), &policy(&excluded))
                .await
                .unwrap();
            assert_eq!(prepared.omitted.len(), 1);
            assert_eq!(prepared.omitted[0].operation, kind);
            assert_eq!(prepared.omitted[0].source.as_deref(), Some(source));
            assert_eq!(
                prepared.omitted[0].destination.as_deref(),
                Some(destination)
            );
            let result = prepared.apply_to_index(repo.path(), &base).await.unwrap();
            assert_eq!(result.changes.len(), 1);
            assert_eq!(result.changes[0]["item"]["path"], "/keep[1].txt");
            assert_eq!(result.changes[0]["newContent"]["content"], "new\n");
        }
    }
}

#[tokio::test]
async fn retained_operation_cannot_depend_on_an_omitted_move() {
    let (repo, base) = repository(&[("private.txt", b"old\n")]);
    let text = movement("rename", "private.txt", "middle.txt")
        + &format!("From {} Mon Sep 17 00:00:00 2001\n", "a".repeat(40))
        + "diff --git a/middle.txt b/middle.txt\n--- a/middle.txt\n+++ b/middle.txt\n@@ -1 +1 @@\n-old\n+new\n";
    let excluded = vec!["private.txt".into()];
    let result = prepare(repo.path(), &base, text.as_bytes(), &policy(&excluded)).await;
    assert!(
        result
            .err()
            .unwrap()
            .to_string()
            .contains("depends on an excluded")
    );
    assert!(!repo.path().join("middle.txt").exists());
}

#[tokio::test]
async fn configured_expansion_limit_is_exact_before_application() {
    for size in [1024, 1025] {
        let bytes = vec![b'x'; size];
        let (repo, base) = repository(&[("source.txt", &bytes)]);
        let mut policy = policy(&[]);
        policy.limit = PatchSizeKiB::try_from(1).unwrap();
        let text = movement("copy", "source.txt", "destination.txt");
        let result = prepare(repo.path(), &base, text.as_bytes(), &policy).await;
        if size == 1024 {
            let applied = result
                .unwrap()
                .apply_to_index(repo.path(), &base)
                .await
                .unwrap();
            assert_eq!(
                applied.changes[0]["newContent"]["content"]
                    .as_str()
                    .unwrap()
                    .len(),
                size
            );
        } else {
            assert!(
                result
                    .err()
                    .unwrap()
                    .to_string()
                    .contains("pre-application expansion")
            );
        }
    }
}

#[tokio::test]
async fn pushes_reject_filtered_preimages_even_when_deleted_or_attributes_removed() {
    for delete in [true, false] {
        let (repo, base) = repository(&[
            (".gitattributes", b"data.txt filter=fixture\n"),
            ("data.txt", b"old\n"),
        ]);
        let text = if delete {
            "diff --git a/data.txt b/data.txt\ndeleted file mode 100644\n--- a/data.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-old\n"
        } else {
            "diff --git a/.gitattributes b/.gitattributes\n--- a/.gitattributes\n+++ b/.gitattributes\n@@ -1 +0,0 @@\n-data.txt filter=fixture\n\
diff --git a/data.txt b/data.txt\n--- a/data.txt\n+++ b/data.txt\n@@ -1 +1 @@\n-old\n+new\n"
        };
        let prepared = prepare(repo.path(), &base, text.as_bytes(), &policy(&[]))
            .await
            .unwrap();
        let before = std::fs::read(repo.path().join(".git").join("index")).unwrap();
        let error = prepared
            .apply_to_index(repo.path(), &base)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("custom-filtered"), "{error:#}");
        assert_eq!(
            std::fs::read(repo.path().join(".git").join("index")).unwrap(),
            before
        );
    }
}

#[tokio::test]
async fn raw_non_utf8_text_hunks_preserve_binary_bytes() {
    let (repo, base) = repository(&[("latin.txt", b"old\n")]);
    let mut bytes = b"diff --git a/latin.txt b/latin.txt\n--- a/latin.txt\n+++ b/latin.txt\n@@ -1 +1 @@\n-old\n+".to_vec();
    bytes.extend_from_slice(b"\xe9\n");
    let prepared = prepare(repo.path(), &base, &bytes, &policy(&[]))
        .await
        .unwrap();
    let result = prepared.apply_to_index(repo.path(), &base).await.unwrap();
    assert_eq!(
        result.changes[0]["newContent"]["contentType"],
        "base64encoded"
    );
    use base64::Engine;
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(result.changes[0]["newContent"]["content"].as_str().unwrap())
            .unwrap(),
        b"\xe9\n"
    );
}

#[test]
fn encoded_request_limit_is_exact_and_covers_json_expansion() {
    let value = Value::String("x".repeat(MAX_REQUEST_BYTES - 2));
    assert_eq!(request_bytes(&value).unwrap().len(), MAX_REQUEST_BYTES);
    assert!(request_bytes(&Value::String("x".repeat(MAX_REQUEST_BYTES - 1))).is_err());
    assert!(request_bytes(&Value::String("\0".repeat(2 * 1024 * 1024))).is_err());
}

#[tokio::test]
async fn raw_patch_size_checks_the_whole_artifact_at_the_byte_boundary() {
    let (repo, base) = repository(&[("old.txt", b"base\n")]);
    let text = movement("copy", "old.txt", "new.txt");
    let mut policy = policy(&[]);
    policy.limit = PatchSizeKiB::try_from(1).unwrap();
    for length in [1024, 1025] {
        let padded = format!("{}\n{text}", "x".repeat(length - text.len() - 1));
        assert_eq!(padded.len(), length);
        let result = prepare(repo.path(), &base, padded.as_bytes(), &policy).await;
        assert_eq!(result.is_ok(), length == 1024);
    }
}

#[tokio::test]
async fn git_binary_delta_and_literal_metadata_are_bounded_before_application() {
    let original = (0u8..=250).cycle().take(64 * 1024).collect::<Vec<_>>();
    let (repo, base) = repository(&[("data.bin", &original)]);
    let mut edited = original.clone();
    edited[100..104].copy_from_slice(&[254, 253, 252, 0]);
    std::fs::write(repo.path().join("data.bin"), &edited).unwrap();
    let output = std::process::Command::new("git")
        .args(["diff", "--binary", "--full-index"])
        .current_dir(repo.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(
        text.contains("\ndelta "),
        "fixture must exercise the actual delta decoder"
    );
    let prepared = prepare(repo.path(), &base, text.as_bytes(), &policy(&[]))
        .await
        .unwrap();
    let applied = prepared.apply_to_index(repo.path(), &base).await.unwrap();
    use base64::Engine;
    let content = &applied.changes[0]["newContent"];
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(content["content"].as_str().unwrap())
            .unwrap(),
        edited
    );
    for size in ["999999999", "18446744073709551615"] {
        let invalid =
            format!("diff --git a/x b/x\nnew file mode 100644\nGIT binary patch\nliteral {size}\n");
        assert!(inspected_paths(invalid.as_bytes()).is_err());
    }
    for bytes in [&[0xff; 20][..], &[0x80][..]] {
        assert!(super::parse::delta_size(bytes, &mut 0).is_err());
    }
}

#[test]
fn quoted_paths_and_conflicting_metadata_use_the_production_parser() {
    let quoted = b"diff --git \"a/old name.txt\" \"b/\\303\\251.txt\"\nsimilarity index 100%\nrename from old name.txt\nrename to \"\\303\\251.txt\"\n";
    let paths = inspected_paths(quoted).unwrap();
    assert!(paths.contains("old name.txt") && paths.contains("\u{e9}.txt"));
    for text in [
        "diff --git a/old b/new\ncopy from old\ncopy to new\n--- a/other\n+++ b/new\n@@ -1 +1 @@\n-a\n+b\n",
        "diff --git a/old b/new\ncopy from old\ncopy from old\ncopy to new\n",
        "diff --git a/old b/new\ncopy from old\n",
        "diff --git a/old b/new\ncopy from ../private\ncopy to new\n",
        "diff --git a/file b/file\nnew file mode 120000\n",
        &(movement("copy", "old", "new") + &movement("copy", "other", "new")),
    ] {
        assert!(inspected_paths(text.as_bytes()).is_err(), "{text}");
    }
}
