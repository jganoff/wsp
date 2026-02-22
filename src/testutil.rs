//! Shared test utilities for git-based integration tests.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Creates a source repo with a single commit on main, clones it,
/// and checks out a `feature` branch in the clone.
/// Returns (clone_dir, source_dir, clone_tempdir, source_tempdir).
pub fn setup_clone_repo() -> (PathBuf, PathBuf, tempfile::TempDir, tempfile::TempDir) {
    let source_tmp = tempfile::tempdir().unwrap();
    let source = source_tmp.path().to_path_buf();
    for args in &[
        vec!["git", "init", "--initial-branch=main"],
        vec!["git", "config", "user.email", "test@test.com"],
        vec!["git", "config", "user.name", "Test"],
        vec!["git", "config", "commit.gpgsign", "false"],
        vec!["git", "commit", "--allow-empty", "-m", "initial"],
    ] {
        let out = Command::new(args[0])
            .args(&args[1..])
            .current_dir(&source)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{:?}: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }

    let clone_tmp = tempfile::tempdir().unwrap();
    let clone_dir = clone_tmp.path().join("repo");
    let out = Command::new("git")
        .args([
            "clone",
            source.to_str().unwrap(),
            clone_dir.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "clone: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Configure clone
    for args in &[
        vec!["git", "config", "user.email", "test@test.com"],
        vec!["git", "config", "user.name", "Test"],
        vec!["git", "config", "commit.gpgsign", "false"],
    ] {
        let out = Command::new(args[0])
            .args(&args[1..])
            .current_dir(&clone_dir)
            .output()
            .unwrap();
        assert!(out.status.success());
    }

    // Create a feature branch from main
    let out = Command::new("git")
        .args(["checkout", "-b", "feature", "--no-track", "origin/main"])
        .current_dir(&clone_dir)
        .output()
        .unwrap();
    assert!(out.status.success());

    (clone_dir, source, clone_tmp, source_tmp)
}

/// Commits a file in a repo on the current branch.
pub fn local_commit(dir: &Path, file: &str, content: &str) {
    std::fs::write(dir.join(file), content).unwrap();
    let out = Command::new("git")
        .args(["add", file])
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(out.status.success());
    let out = Command::new("git")
        .args(["commit", "-m", &format!("add {}", file)])
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "commit: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
