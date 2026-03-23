//! Shared test utilities for git-based integration tests.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::Paths;

/// Creates a bare git repo at `dir` (runs `git init --bare`) and returns `dir`
/// as a [`PathBuf`].  The default branch is set to `main` via
/// `git symbolic-ref`, which works on all git versions (no `--initial-branch`
/// flag required).
pub fn setup_bare_repo(dir: &Path) -> PathBuf {
    let dir_str = dir.to_str().expect("non-UTF-8 path");
    let out = Command::new("git")
        .args(["init", "--bare", dir_str])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git init --bare: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    // Set default branch to main (compatible with all git versions).
    let out = Command::new("git")
        .args(["symbolic-ref", "HEAD", "refs/heads/main"])
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "symbolic-ref HEAD: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    dir.to_path_buf()
}

/// Constructs a [`Paths`] struct rooted inside `tmp`.
///
/// Layout:
/// - `tmp/wsp/` — data directory (config, mirrors, gc, templates)
/// - `tmp/workspaces/` — workspaces directory (created eagerly)
///
/// This mirrors the structure used by `setup_test_env` in `workspace.rs`
/// tests and is the canonical way to build test paths without touching the
/// real filesystem.
pub fn make_test_paths(tmp: &tempfile::TempDir) -> Paths {
    let data_dir = tmp.path().join("wsp");
    let workspaces_dir = tmp.path().join("workspaces");
    std::fs::create_dir_all(&workspaces_dir).unwrap();
    Paths {
        config_path: data_dir.join("config.yaml"),
        mirrors_dir: data_dir.join("mirrors"),
        gc_dir: data_dir.join("gc"),
        templates_dir: data_dir.join("templates"),
        workspaces_dir,
    }
}

/// Creates a source repo with a single commit on main, clones it,
/// and checks out a `feature` branch in the clone.
/// Returns (clone_dir, source_dir, clone_tempdir, source_tempdir).
pub fn setup_clone_repo() -> (PathBuf, PathBuf, tempfile::TempDir, tempfile::TempDir) {
    let source_tmp = tempfile::tempdir().unwrap();
    let source = source_tmp.path().to_path_buf();
    for args in &[
        vec!["git", "init"],
        vec!["git", "symbolic-ref", "HEAD", "refs/heads/main"],
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
    for args in &[
        vec!["git", "config", "user.email", "test@test.local"],
        vec!["git", "config", "user.name", "Test"],
        vec!["git", "config", "commit.gpgsign", "false"],
        vec!["git", "add", file],
    ] {
        let out = Command::new(args[0])
            .args(&args[1..])
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{:?}: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }
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
