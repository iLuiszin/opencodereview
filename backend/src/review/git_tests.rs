use super::*;
use std::process::Command;

fn run(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_NO_LAZY_FETCH", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

#[test]
fn diff_blob_ids_skip_null_ids_and_paths() {
    let (old, new) = ("a".repeat(40), "b".repeat(40));
    let raw = format!(
        ":100644 100644 {old} {new} M\0src/a.rs\0:000000 100644 {} {new} A\0src/b.rs\0",
        "0".repeat(40)
    );
    assert_eq!(diff_blob_ids(raw.as_bytes()), vec![old, new]);
}

#[test]
fn blobless_clones_fetch_every_diff_blob_before_diffing() {
    let root = std::env::temp_dir().join(format!(
        "ocr-prefetch-{}-{}",
        std::process::id(),
        crate::db::now_millis()
    ));
    let (origin, clone) = (root.join("origin"), root.join("clone"));
    fs::create_dir_all(&origin).unwrap();
    run(&origin, &["init", "-q", "-b", "main"]);
    run(&origin, &["config", "uploadpack.allowFilter", "true"]);
    run(&origin, &["config", "uploadpack.allowAnySHA1InWant", "true"]);
    let commit = |message: &str, suffix: &str| {
        for index in 0..5 {
            fs::write(
                origin.join(format!("file-{index}.txt")),
                format!("line {index}{suffix}\n"),
            )
            .unwrap();
        }
        run(&origin, &["add", "."]);
        run(
            &origin,
            &[
                "-c",
                "user.name=OpenCodeReview",
                "-c",
                "user.email=ocr@example.com",
                "commit",
                "-q",
                "-m",
                message,
            ],
        );
        run(&origin, &["rev-parse", "HEAD"])
    };
    let base = commit("base", "");
    let head = commit("head", " changed");
    let url = format!("file://{}", origin.display());
    let output = Command::new("git")
        .args(["clone", "-q", "--filter=blob:none", "--no-checkout", &url])
        .arg(&clone)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let range = format!("{base}...{head}");
    let ids = diff_blob_ids(
        &git_capture(
            &clone,
            &["diff", "--raw", "--no-abbrev", "--no-renames", "-z", &range],
            &AtomicBool::new(false),
            60,
        )
        .unwrap(),
    );
    assert_eq!(ids.len(), 10);
    let missing = |ids: &[String]| {
        ids.iter()
            .filter(|id| {
                !Command::new("git")
                    .arg("-C")
                    .arg(&clone)
                    .args(["cat-file", "-e", id])
                    .env("GIT_NO_LAZY_FETCH", "1")
                    .status()
                    .unwrap()
                    .success()
            })
            .count()
    };
    assert!(missing(&ids) > 0);
    prefetch_diff_blobs(&clone, &range, &AtomicBool::new(false)).unwrap();
    assert_eq!(missing(&ids), 0);
    assert!(diff_local(&clone, &range, &AtomicBool::new(false))
        .unwrap()
        .contains("+line 4 changed"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn path_diffs_read_bracketed_names_literally() {
    let repo = std::env::temp_dir().join(format!(
        "ocr-literal-paths-{}-{}",
        std::process::id(),
        crate::db::now_millis()
    ));
    fs::create_dir_all(&repo).unwrap();
    run(&repo, &["init", "-q", "-b", "main"]);
    run(&repo, &["config", "user.email", "test@example.invalid"]);
    run(&repo, &["config", "user.name", "Test"]);
    fs::write(repo.join("[key].ts"), "export const route = 1;\n").unwrap();
    fs::write(repo.join("k.ts"), "glob decoy\n").unwrap();
    run(&repo, &["add", "."]);
    run(&repo, &["commit", "-q", "-m", "base"]);
    let base = run(&repo, &["rev-parse", "HEAD"]);
    fs::write(repo.join("[key].ts"), "export const route = 2;\n").unwrap();
    fs::write(repo.join("k.ts"), "glob decoy changed\n").unwrap();
    run(&repo, &["commit", "-q", "-am", "head"]);
    let diff = diff_local_paths(
        &repo,
        &format!("{base}..HEAD"),
        &["[key].ts".to_string()],
        &AtomicBool::new(false),
    );
    let _ = fs::remove_dir_all(&repo);
    assert!(diff.contains("diff --git a/[key].ts b/[key].ts"), "{diff}");
    assert!(!diff.contains("k.ts b/k.ts"), "{diff}");
}

fn reviewed_prefix_fixture(test: &str) -> (PathBuf, PathBuf, String, String) {
    let root = std::env::temp_dir().join(format!(
        "ocr-reviewed-prefix-{test}-{}-{}",
        std::process::id(),
        crate::db::now_millis()
    ));
    let (origin, clone) = (root.join("origin"), root.join("clone"));
    fs::create_dir_all(&origin).unwrap();
    run(&origin, &["init", "-q", "-b", "main"]);
    run(&origin, &["config", "user.email", "test@example.invalid"]);
    run(&origin, &["config", "user.name", "Test"]);
    let commit = |repo: &Path, file: &str, text: &str, message: &str| {
        fs::write(repo.join(file), text).unwrap();
        run(repo, &["add", file]);
        run(repo, &["commit", "-q", "-m", message]);
        run(repo, &["rev-parse", "HEAD"])
    };
    fs::write(origin.join("base.txt"), "base\n").unwrap();
    run(&origin, &["add", "."]);
    run(&origin, &["commit", "-q", "-m", "base"]);
    let base = run(&origin, &["rev-parse", "HEAD"]);
    run(&origin, &["checkout", "-q", "-b", "integration"]);
    let reviewed = commit(&origin, "reviewed-one.rs", "fn one() {}\n", "reviewed one");
    commit(&origin, "reviewed-two.rs", "fn two() {}\n", "reviewed two");
    run(&origin, &["checkout", "-q", "main"]);
    let output = Command::new("git")
        .args(["clone", "-q", &format!("file://{}", origin.display())])
        .arg(&clone)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    run(&clone, &["config", "user.email", "test@example.invalid"]);
    run(&clone, &["config", "user.name", "Test"]);
    (root, clone, base, reviewed)
}

fn commit_in_clone(clone: &Path, file: &str, text: &str, message: &str) -> String {
    fs::write(clone.join(file), text).unwrap();
    run(clone, &["add", file]);
    run(clone, &["commit", "-q", "-m", message]);
    run(clone, &["rev-parse", "HEAD"])
}

#[test]
fn reviewed_prefix_skips_only_the_already_reviewed_prefix() {
    let (root, clone, base, reviewed) = reviewed_prefix_fixture("prefix");
    run(&clone, &["checkout", "-q", "--detach", &base]);
    run(&clone, &["cherry-pick", &reviewed]);
    run(&clone, &["cherry-pick", "origin/integration"]);
    let prefix_head = run(&clone, &["rev-parse", "HEAD"]);
    let head = commit_in_clone(&clone, "new.rs", "fn new() {}\n", "new work");
    let cancel = AtomicBool::new(false);
    let prefix = crate::review::scope::reviewed_prefix(&clone, &["integration".to_string()], &base, &head, &cancel)
        .unwrap()
        .unwrap();
    assert_eq!(prefix.base_sha, prefix_head);
    assert_eq!(prefix.skipped_commits, 2);
    assert_eq!(prefix.reviewed_commits, 1);
    let files = changed_file_targets_local(&clone, &format!("{}..{head}", prefix.base_sha), &cancel).unwrap();
    assert_eq!(files, vec!["new.rs"]);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn reviewed_prefix_stops_before_a_matching_commit_after_new_work() {
    let (root, clone, base, reviewed) = reviewed_prefix_fixture("gap");
    run(&clone, &["checkout", "-q", "--detach", &base]);
    commit_in_clone(&clone, "new.rs", "fn new() {}\n", "new work");
    run(&clone, &["cherry-pick", &reviewed]);
    let head = run(&clone, &["rev-parse", "HEAD"]);
    let prefix = crate::review::scope::reviewed_prefix(
        &clone,
        &["integration".to_string()],
        &base,
        &head,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(prefix, None);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn whitespace_semantic_patch_id_matches_do_not_count_as_reviewed() {
    let (root, clone, base, _) = reviewed_prefix_fixture("whitespace");
    let origin = root.join("origin");
    run(&origin, &["checkout", "-q", "integration"]);
    fs::write(origin.join("semantic.py"), "def f():\n    return 1\n").unwrap();
    run(&origin, &["add", "semantic.py"]);
    run(&origin, &["commit", "-q", "-m", "reviewed python"]);
    run(&origin, &["checkout", "-q", "main"]);
    run(&clone, &["checkout", "-q", "--detach", &base]);
    fs::write(clone.join("semantic.py"), "def f():\n  return 1\n").unwrap();
    run(&clone, &["add", "semantic.py"]);
    run(&clone, &["commit", "-q", "-m", "whitespace semantic change"]);
    let head = run(&clone, &["rev-parse", "HEAD"]);
    let prefix = crate::review::scope::reviewed_prefix(
        &clone,
        &["integration".to_string()],
        &base,
        &head,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(prefix, None);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn reviewed_prefix_is_empty_without_setting_and_full_on_git_failure() {
    let (root, clone, base, _) = reviewed_prefix_fixture("fallback");
    run(&clone, &["checkout", "-q", "--detach", &base]);
    let head = commit_in_clone(&clone, "new.rs", "fn new() {}\n", "new work");
    let cancel = AtomicBool::new(false);
    assert_eq!(
        crate::review::scope::reviewed_prefix(&clone, &[], &base, &head, &cancel).unwrap(),
        None
    );
    assert_eq!(
        crate::review::scope::reviewed_prefix_or_full(&clone, &["missing-branch".to_string()], &base, &head, &cancel,)
            .unwrap(),
        None
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn file_limit_count_intersects_the_pr_net_diff_after_a_base_merge() {
    let repo = std::env::temp_dir().join(format!(
        "ocr-file-limit-merge-{}-{}",
        std::process::id(),
        crate::db::now_millis()
    ));
    fs::create_dir_all(&repo).unwrap();
    run(&repo, &["init", "-q", "-b", "main"]);
    run(&repo, &["config", "user.email", "test@example.invalid"]);
    run(&repo, &["config", "user.name", "Test"]);
    fs::write(repo.join("base.txt"), "base\n").unwrap();
    run(&repo, &["add", "."]);
    run(&repo, &["commit", "-q", "-m", "base"]);
    run(&repo, &["checkout", "-q", "-b", "feature"]);
    fs::write(repo.join("reviewed.rs"), "fn reviewed() {}\n").unwrap();
    run(&repo, &["add", "reviewed.rs"]);
    run(&repo, &["commit", "-q", "-m", "reviewed work"]);
    let reviewed = run(&repo, &["rev-parse", "HEAD"]);
    run(&repo, &["checkout", "-q", "main"]);
    fs::write(repo.join("main-only.rs"), "fn main_only() {}\n").unwrap();
    run(&repo, &["add", "main-only.rs"]);
    run(&repo, &["commit", "-q", "-m", "base work"]);
    let base_tip = run(&repo, &["rev-parse", "HEAD"]);
    run(&repo, &["checkout", "-q", "feature"]);
    run(&repo, &["merge", "--no-ff", "-q", "main", "-m", "sync base"]);
    fs::write(repo.join("new.rs"), "fn new() {}\n").unwrap();
    run(&repo, &["add", "new.rs"]);
    run(&repo, &["commit", "-q", "-m", "new work"]);
    let head = run(&repo, &["rev-parse", "HEAD"]);
    let cancel = AtomicBool::new(false);
    let raw = changed_file_targets_local(&repo, &format!("{reviewed}..{head}"), &cancel).unwrap();
    assert!(raw.iter().any(|path| path == "main-only.rs"), "{raw:?}");
    let full_files = changed_files_local(&repo, &format!("{base_tip}...{head}"), &cancel).unwrap();
    assert!(!full_files.iter().any(|path| path == "main-only.rs"), "{full_files:?}");
    assert_eq!(
        crate::review::reviewable_file_count_in_range(&repo, &format!("{reviewed}..{head}"), &full_files, &cancel)
            .unwrap(),
        1
    );
    let _ = fs::remove_dir_all(repo);
}

#[test]
fn force_full_counts_and_reviews_the_full_range_despite_a_matching_prefix() {
    let (root, clone, base, reviewed) = reviewed_prefix_fixture("force-full");
    run(&clone, &["checkout", "-q", "--detach", &base]);
    run(&clone, &["cherry-pick", &reviewed]);
    run(&clone, &["cherry-pick", "origin/integration"]);
    let head = commit_in_clone(&clone, "new.rs", "fn new() {}\n", "new work");
    let cancel = AtomicBool::new(false);
    let branches = vec!["integration".to_string()];
    let full_pr_range = format!("{base}...{head}");
    let full_files = changed_files_local(&clone, &full_pr_range, &cancel).unwrap();

    let narrowed =
        crate::review::scope::reviewed_count_scope(&clone, &branches, true, &base, &head, None, &cancel).unwrap();
    let prefix = narrowed.prefix.clone().expect("the matching prefix is detected");
    let count_base = narrowed.count_base.clone().expect("the prefix is the count base");
    assert_eq!(count_base, prefix.base_sha);
    assert!(crate::review::scope::reviewed_prefix_in_use(
        narrowed.prefix.as_ref(),
        narrowed.count_base.as_deref()
    ));
    let narrowed_range = crate::review::scope::file_limit_range(&full_pr_range, Some(&count_base), &head);
    assert_eq!(
        crate::review::reviewable_file_count_in_range(&clone, &narrowed_range, &full_files, &cancel).unwrap(),
        1,
        "without force-full only the new file counts"
    );

    let forced =
        crate::review::scope::reviewed_count_scope(&clone, &branches, false, &base, &head, None, &cancel).unwrap();
    assert_eq!(forced.prefix, None, "force-full never detects a prefix");
    assert_eq!(forced.count_base, None, "force-full never narrows the count base");
    let forced_range = crate::review::scope::file_limit_range(&full_pr_range, forced.count_base.as_deref(), &head);
    assert_eq!(
        forced_range, full_pr_range,
        "force-full reviews and counts the full range"
    );
    assert_eq!(
        crate::review::reviewable_file_count_in_range(&clone, &forced_range, &full_files, &cancel).unwrap(),
        3,
        "the forced count covers both reviewed files and the new work"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn scope_note_is_emitted_only_when_the_reviewed_prefix_is_used() {
    let (root, clone, base, reviewed) = reviewed_prefix_fixture("scope-note");
    run(&clone, &["checkout", "-q", "--detach", &base]);
    run(&clone, &["cherry-pick", &reviewed]);
    run(&clone, &["cherry-pick", "origin/integration"]);
    let prior = commit_in_clone(&clone, "middle.rs", "fn middle() {}\n", "middle work");
    let head = commit_in_clone(&clone, "new.rs", "fn new() {}\n", "new work");
    let cancel = AtomicBool::new(false);
    let branches = vec!["integration".to_string()];

    let scope =
        crate::review::scope::reviewed_count_scope(&clone, &branches, true, &base, &head, None, &cancel).unwrap();
    let note = crate::review::scope::reviewed_scope_note(
        &clone,
        scope.prefix.as_ref(),
        scope.count_base.as_deref(),
        &head,
        &cancel,
    )
    .expect("the note fires while the prefix is the count base");
    assert!(
        note.contains("2 commits already on `integration` were skipped"),
        "{note}"
    );

    // An incremental prior head newer than the prefix base displaces it: the review starts
    // from the prior head, so the skipped-commits note would misreport what was reviewed.
    let scope =
        crate::review::scope::reviewed_count_scope(&clone, &branches, true, &base, &head, Some(&prior), &cancel)
            .unwrap();
    assert_eq!(scope.count_base.as_deref(), Some(prior.as_str()));
    assert!(!crate::review::scope::reviewed_prefix_in_use(
        scope.prefix.as_ref(),
        scope.count_base.as_deref()
    ));
    assert_eq!(
        crate::review::scope::reviewed_scope_note(
            &clone,
            scope.prefix.as_ref(),
            scope.count_base.as_deref(),
            &head,
            &cancel,
        ),
        None
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn blobs_of_deleted_bracketed_paths_are_absent() {
    let repo = std::env::temp_dir().join(format!(
        "ocr-deleted-blob-{}-{}",
        std::process::id(),
        crate::db::now_millis()
    ));
    fs::create_dir_all(&repo).unwrap();
    run(&repo, &["init", "-q", "-b", "main"]);
    run(&repo, &["config", "user.email", "test@example.invalid"]);
    run(&repo, &["config", "user.name", "Test"]);
    fs::write(repo.join("[key].ts"), "export const route = 1;\n").unwrap();
    run(&repo, &["add", "."]);
    run(&repo, &["commit", "-q", "-m", "base"]);
    run(&repo, &["mv", "[key].ts", "[...key].ts"]);
    run(&repo, &["commit", "-q", "-m", "head"]);
    let cancel = AtomicBool::new(false);
    let (moved, kept) = (
        show_blob(&repo, "HEAD", "[key].ts", &cancel),
        show_blob(&repo, "HEAD", "[...key].ts", &cancel),
    );
    let _ = fs::remove_dir_all(&repo);
    assert_eq!(moved, None);
    assert_eq!(kept.as_deref(), Some("export const route = 1;\n"));
}
