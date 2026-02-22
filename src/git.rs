use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BranchSafety {
    Merged,
    SquashMerged,
    PushedToRemote,
    Unmerged,
}

fn path_str(p: &Path) -> Result<&str> {
    p.to_str().context("path contains non-UTF8 characters")
}

pub fn run(dir: Option<&Path>, args: &[&str]) -> Result<String> {
    run_with_env(dir, args, &[])
}

pub fn run_with_env(dir: Option<&Path>, args: &[&str], env: &[(&str, &str)]) -> Result<String> {
    let mut cmd = Command::new("git");
    cmd.args(args);
    if let Some(d) = dir {
        cmd.current_dir(d);
    }
    for (k, v) in env {
        cmd.env(k, v);
    }

    let output = cmd.output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let args_str = args.join(" ");
        if let Some(d) = dir {
            bail!(
                "git {} (in {}): {}\n{}",
                args_str,
                d.display(),
                output.status,
                stderr
            );
        } else {
            bail!("git {}: {}\n{}", args_str, output.status, stderr);
        }
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub fn clone_bare(url: &str, dest: &Path) -> Result<()> {
    let dest_str = path_str(dest)?;
    run(None, &["clone", "--bare", url, dest_str])?;
    Ok(())
}

pub fn configure_fetch_refspec(dir: &Path) -> Result<()> {
    run(
        Some(dir),
        &[
            "config",
            "remote.origin.fetch",
            "+refs/heads/*:refs/remotes/origin/*",
        ],
    )?;
    Ok(())
}

fn ensure_fetch_refspec(dir: &Path) -> Result<()> {
    let has_refspec = run(Some(dir), &["config", "--get", "remote.origin.fetch"]).is_ok();
    if !has_refspec {
        configure_fetch_refspec(dir)?;
    }
    Ok(())
}

pub fn fetch(dir: &Path, prune: bool) -> Result<()> {
    ensure_fetch_refspec(dir)?;
    let mut args = vec!["fetch", "--all"];
    if prune {
        args.push("--prune");
    }
    run(Some(dir), &args)?;
    Ok(())
}

pub fn default_branch(dir: &Path) -> Result<String> {
    let r = run(Some(dir), &["symbolic-ref", "refs/remotes/origin/HEAD"]);
    let ref_str = match r {
        Ok(s) => s,
        Err(_) => run(Some(dir), &["symbolic-ref", "HEAD"])
            .map_err(|e| anyhow::anyhow!("cannot detect default branch: {}", e))?,
    };

    let parts: Vec<&str> = ref_str.split('/').collect();
    if parts.len() < 3 {
        bail!("unexpected ref format: {}", ref_str);
    }
    Ok(parts[parts.len() - 1].to_string())
}

/// Configure wsp-mirror remote to fetch refs/remotes/origin/* from the bare mirror
/// into refs/remotes/wsp-mirror/* in the clone. This is needed because bare mirrors
/// store fetched refs under refs/remotes/origin/*, not refs/heads/*.
pub fn configure_wsp_mirror_refspec(dir: &Path) -> Result<()> {
    run(
        Some(dir),
        &[
            "config",
            "remote.wsp-mirror.fetch",
            "+refs/remotes/origin/*:refs/remotes/wsp-mirror/*",
        ],
    )?;
    Ok(())
}

pub fn clone_local(mirror_dir: &Path, dest: &Path) -> Result<()> {
    let src = path_str(mirror_dir)?;
    let dst = path_str(dest)?;
    run(
        None,
        &["clone", "--local", "--origin", "wsp-mirror", src, dst],
    )?;
    Ok(())
}

pub fn remote_set_origin(dir: &Path, url: &str) -> Result<()> {
    // Remove origin if it exists (ignore error if it doesn't)
    let _ = run(Some(dir), &["remote", "remove", "origin"]);
    run(Some(dir), &["remote", "add", "origin", url])?;
    Ok(())
}

pub fn fetch_remote(dir: &Path, remote: &str) -> Result<()> {
    run(Some(dir), &["fetch", remote])?;
    Ok(())
}

pub fn fetch_remote_prune(dir: &Path, remote: &str) -> Result<()> {
    run(Some(dir), &["fetch", "--prune", remote])?;
    Ok(())
}

pub fn checkout_new_branch(dir: &Path, branch: &str, start_point: &str) -> Result<()> {
    run(
        Some(dir),
        &["checkout", "-b", branch, "--no-track", start_point],
    )?;
    Ok(())
}

pub fn checkout(dir: &Path, ref_or_branch: &str) -> Result<()> {
    run(Some(dir), &["checkout", ref_or_branch])?;
    Ok(())
}

pub fn checkout_detached(dir: &Path, git_ref: &str) -> Result<()> {
    run(Some(dir), &["checkout", "--detach", git_ref])?;
    Ok(())
}

pub fn default_branch_for_remote(dir: &Path, remote: &str) -> Result<String> {
    let ref_path = format!("refs/remotes/{}/HEAD", remote);
    let r = run(Some(dir), &["symbolic-ref", &ref_path]);
    let ref_str = match r {
        Ok(s) => s,
        Err(_) => run(Some(dir), &["symbolic-ref", "HEAD"])
            .map_err(|e| anyhow::anyhow!("cannot detect default branch for {}: {}", remote, e))?,
    };

    let parts: Vec<&str> = ref_str.split('/').collect();
    if parts.len() < 3 {
        bail!("unexpected ref format: {}", ref_str);
    }
    Ok(parts[parts.len() - 1].to_string())
}

pub fn remote_set_head(dir: &Path, remote: &str, branch: &str) -> Result<()> {
    run(Some(dir), &["remote", "set-head", remote, branch])?;
    Ok(())
}

pub fn branch_is_merged(dir: &Path, branch: &str, target: &str) -> Result<bool> {
    let mut cmd = Command::new("git");
    cmd.args(["merge-base", "--is-ancestor", branch, target]);
    cmd.current_dir(dir);
    let output = cmd.output()?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            bail!(
                "git merge-base --is-ancestor (in {}): {}\n{}",
                dir.display(),
                output.status,
                stderr
            );
        }
    }
}

/// Detects if a branch was squash-merged into target using the commit-tree + cherry algorithm.
pub fn branch_is_squash_merged(dir: &Path, branch: &str, target: &str) -> Result<bool> {
    let mb = merge_base(dir, branch, target)?;
    let tree = run(Some(dir), &["rev-parse", &format!("{}^{{tree}}", branch)])?;
    let env = [
        ("GIT_AUTHOR_NAME", "wsp"),
        ("GIT_AUTHOR_EMAIL", "wsp@localhost"),
        ("GIT_COMMITTER_NAME", "wsp"),
        ("GIT_COMMITTER_EMAIL", "wsp@localhost"),
    ];
    let temp_commit = run_with_env(
        Some(dir),
        &["commit-tree", &tree, "-p", &mb, "-m", "_"],
        &env,
    )?;
    let cherry_out = run(Some(dir), &["cherry", target, &temp_commit])?;
    Ok(cherry_out.starts_with('-'))
}

/// Detects if a branch's changes are already present in target by comparing file contents.
/// This catches squash merges where the cherry/patch-id algorithm fails due to diverged context
/// (e.g. when the branch was not rebased onto target before the squash merge).
pub fn is_content_merged(dir: &Path, branch: &str, target: &str) -> Result<bool> {
    let mb = merge_base(dir, branch, target)?;
    let changed_output = run(Some(dir), &["diff", "--name-only", &mb, branch])?;
    if changed_output.is_empty() {
        // No file changes on this branch; can't determine squash-merge from content alone
        return Ok(false);
    }
    let files: Vec<&str> = changed_output.lines().collect();
    let mut cmd = Command::new("git");
    cmd.args(["diff", "--quiet", target, branch, "--"]);
    for f in &files {
        cmd.arg(f);
    }
    cmd.current_dir(dir);
    let output = cmd.output()?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            bail!(
                "git diff --quiet (in {}): {}\n{}",
                dir.display(),
                output.status,
                stderr
            );
        }
    }
}

pub fn remote_branch_exists(dir: &Path, branch: &str) -> bool {
    let remote_ref = format!("refs/remotes/origin/{}", branch);
    ref_exists(dir, &remote_ref)
}

/// Composite safety check for a workspace branch.
/// Checks in order: merged → squash-merged → pushed to remote → unmerged.
pub fn branch_safety(dir: &Path, branch: &str, target: &str) -> BranchSafety {
    if branch_is_merged(dir, branch, target).unwrap_or(false) {
        return BranchSafety::Merged;
    }
    if branch_is_squash_merged(dir, branch, target).unwrap_or(false) {
        return BranchSafety::SquashMerged;
    }
    if is_content_merged(dir, branch, target).unwrap_or(false) {
        return BranchSafety::SquashMerged;
    }
    if remote_branch_exists(dir, branch) {
        return BranchSafety::PushedToRemote;
    }
    BranchSafety::Unmerged
}

pub fn branch_exists(dir: &Path, branch: &str) -> bool {
    let ref_path = format!("refs/heads/{}", branch);
    run(Some(dir), &["rev-parse", "--verify", &ref_path]).is_ok()
}

pub fn ref_exists(dir: &Path, git_ref: &str) -> bool {
    run(Some(dir), &["rev-parse", "--verify", git_ref]).is_ok()
}

pub fn branch_current(dir: &Path) -> Result<String> {
    run(Some(dir), &["rev-parse", "--abbrev-ref", "HEAD"])
}

/// Resolved upstream reference for the current branch.
pub enum UpstreamRef {
    /// @{upstream} tracking branch exists.
    Tracking,
    /// No tracking branch; fell back to origin/<default>.
    DefaultBranch(String),
    /// Nothing available — use HEAD.
    Head,
}

/// Probe once and return the best upstream reference.
pub fn resolve_upstream_ref(dir: &Path) -> UpstreamRef {
    if run(Some(dir), &["rev-parse", "--verify", "@{upstream}"]).is_ok() {
        return UpstreamRef::Tracking;
    }
    if let Ok(branch) = default_branch(dir) {
        return UpstreamRef::DefaultBranch(branch);
    }
    UpstreamRef::Head
}

pub fn merge_base(dir: &Path, a: &str, b: &str) -> Result<String> {
    run(Some(dir), &["merge-base", a, b])
}

pub fn ahead_count(dir: &Path) -> Result<u32> {
    ahead_count_from(dir, &resolve_upstream_ref(dir))
}

pub fn ahead_count_from(dir: &Path, upstream: &UpstreamRef) -> Result<u32> {
    let range = match upstream {
        UpstreamRef::Tracking => "@{upstream}..HEAD".to_string(),
        UpstreamRef::DefaultBranch(b) => format!("origin/{}..HEAD", b),
        UpstreamRef::Head => return Ok(0),
    };
    let out = run(Some(dir), &["rev-list", "--count", &range])?;
    Ok(out.parse::<u32>().unwrap_or(0))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncAction {
    UpToDate,
    FastForward { commits: u32 },
    Rebased { commits: u32 },
    Merged,
}

pub fn commit_count(dir: &Path, from: &str, to: &str) -> Result<u32> {
    let range = format!("{}..{}", from, to);
    let out = run(Some(dir), &["rev-list", "--count", &range])?;
    Ok(out.parse::<u32>().unwrap_or(0))
}

pub fn rebase_onto(dir: &Path, target: &str) -> Result<SyncAction> {
    let head_sha = run(Some(dir), &["rev-parse", "HEAD"])?;
    let target_sha = run(Some(dir), &["rev-parse", target])?;

    if head_sha == target_sha {
        return Ok(SyncAction::UpToDate);
    }

    // HEAD is ancestor of target → fast-forward
    if branch_is_merged(dir, "HEAD", target)? {
        let commits = commit_count(dir, "HEAD", target)?;
        run(Some(dir), &["rebase", target])?;
        return Ok(SyncAction::FastForward { commits });
    }

    // target is ancestor of HEAD → HEAD is ahead, rebase is no-op
    if branch_is_merged(dir, target, "HEAD")? {
        return Ok(SyncAction::UpToDate);
    }

    // Diverged: count commits ahead, attempt rebase
    let mb = merge_base(dir, "HEAD", target)?;
    let commits = commit_count(dir, &mb, "HEAD")?;
    match run(Some(dir), &["rebase", target]) {
        Ok(_) => Ok(SyncAction::Rebased { commits }),
        Err(e) => {
            let _ = run(Some(dir), &["rebase", "--abort"]);
            Err(e)
        }
    }
}

pub fn merge_from(dir: &Path, target: &str) -> Result<SyncAction> {
    let head_sha = run(Some(dir), &["rev-parse", "HEAD"])?;
    let target_sha = run(Some(dir), &["rev-parse", target])?;

    if head_sha == target_sha {
        return Ok(SyncAction::UpToDate);
    }

    // HEAD is ancestor of target → fast-forward
    if branch_is_merged(dir, "HEAD", target)? {
        let commits = commit_count(dir, "HEAD", target)?;
        run(Some(dir), &["merge", "--ff-only", target])?;
        return Ok(SyncAction::FastForward { commits });
    }

    // target is ancestor of HEAD → HEAD is ahead, nothing to merge
    if branch_is_merged(dir, target, "HEAD")? {
        return Ok(SyncAction::UpToDate);
    }

    // Diverged: attempt merge
    match run(Some(dir), &["merge", "--no-edit", target]) {
        Ok(_) => Ok(SyncAction::Merged),
        Err(e) => {
            let _ = run(Some(dir), &["merge", "--abort"]);
            Err(e)
        }
    }
}

pub fn push(
    dir: &Path,
    remote: &str,
    branch: &str,
    set_upstream: bool,
    force_with_lease: bool,
) -> Result<()> {
    let mut args = vec!["push"];
    if set_upstream {
        args.push("--set-upstream");
    }
    if force_with_lease {
        args.push("--force-with-lease");
    }
    args.push(remote);
    args.push(branch);
    run(Some(dir), &args)?;
    Ok(())
}

pub fn changed_file_count(dir: &Path) -> Result<u32> {
    let out = run(Some(dir), &["status", "--short"])?;
    if out.is_empty() {
        Ok(0)
    } else {
        Ok(out.lines().count() as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{local_commit, setup_clone_repo};
    use std::path::PathBuf;
    use std::process::Command as StdCommand;

    /// Creates a bare repo with a single commit on main, plus a source repo.
    /// Returns (bare_dir, source_dir, TempDir handles to keep alive).
    fn setup_bare_repo() -> (PathBuf, PathBuf, tempfile::TempDir, tempfile::TempDir) {
        let source_tmp = tempfile::tempdir().unwrap();
        let source = source_tmp.path().to_path_buf();
        for args in &[
            vec!["git", "init", "--initial-branch=main"],
            vec!["git", "config", "user.email", "test@test.com"],
            vec!["git", "config", "user.name", "Test"],
            vec!["git", "config", "commit.gpgsign", "false"],
            vec!["git", "commit", "--allow-empty", "-m", "initial"],
        ] {
            let out = StdCommand::new(args[0])
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

        let bare_tmp = tempfile::tempdir().unwrap();
        let bare = bare_tmp.path().join("repo.git");
        clone_bare(source.to_str().unwrap(), &bare).unwrap();
        configure_fetch_refspec(&bare).unwrap();
        fetch(&bare, true).unwrap();

        // Set symbolic HEAD so default_branch works
        let out = StdCommand::new("git")
            .args([
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/main",
            ])
            .current_dir(&bare)
            .output()
            .unwrap();
        assert!(out.status.success());

        (bare, source, bare_tmp, source_tmp)
    }

    /// Creates a commit on a branch in the source repo with a unique file change.
    fn commit_on_branch(dir: &Path, branch: &str, file: &str) {
        for args in &[
            vec!["git", "checkout", "-B", branch],
            vec!["git", "config", "user.email", "test@test.com"],
            vec!["git", "config", "user.name", "Test"],
            vec!["git", "config", "commit.gpgsign", "false"],
        ] {
            let out = StdCommand::new(args[0])
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
        std::fs::write(dir.join(file), file).unwrap();
        let out = StdCommand::new("git")
            .args(["add", file])
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(out.status.success());
        let out = StdCommand::new("git")
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

    /// Simulates a squash-merge of `branch` into `target` on the source repo.
    fn squash_merge(dir: &Path, branch: &str, target: &str) {
        for args in &[
            vec!["git", "checkout", target],
            vec!["git", "merge", "--squash", branch],
            vec!["git", "commit", "-m", &format!("squash-merge {}", branch)],
        ] {
            let out = StdCommand::new(args[0])
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
    }

    #[test]
    fn test_branch_is_squash_merged() {
        let (bare, source, _bt, _st) = setup_bare_repo();

        // Create a feature branch with a commit, then squash-merge it
        commit_on_branch(&source, "feature", "feat.txt");
        squash_merge(&source, "feature", "main");

        // Fetch into bare so it has the updated refs
        fetch(&bare, true).unwrap();

        let result = branch_is_squash_merged(&bare, "origin/feature", "origin/main").unwrap();
        assert!(result, "squash-merged branch should be detected");
    }

    #[test]
    fn test_branch_is_squash_merged_false() {
        let (bare, source, _bt, _st) = setup_bare_repo();

        // Create a feature branch with a commit but don't merge it
        commit_on_branch(&source, "unmerged", "unmerged.txt");

        fetch(&bare, true).unwrap();

        let result = branch_is_squash_merged(&bare, "origin/unmerged", "origin/main").unwrap();
        assert!(
            !result,
            "unmerged branch should not be detected as squash-merged"
        );
    }

    #[test]
    fn test_remote_branch_exists() {
        let (bare, source, _bt, _st) = setup_bare_repo();
        commit_on_branch(&source, "exists-branch", "e.txt");
        fetch(&bare, true).unwrap();

        assert!(remote_branch_exists(&bare, "exists-branch"));
    }

    #[test]
    fn test_remote_branch_not_exists() {
        let (bare, _source, _bt, _st) = setup_bare_repo();
        assert!(!remote_branch_exists(&bare, "no-such-branch"));
    }

    #[test]
    fn test_branch_safety_variants() {
        let (bare, source, _bt, _st) = setup_bare_repo();

        // Create branches on source for each scenario
        // 1. Regular merged branch
        commit_on_branch(&source, "merged-br", "m.txt");
        let out = StdCommand::new("git")
            .args(["checkout", "main"])
            .current_dir(&source)
            .output()
            .unwrap();
        assert!(out.status.success());
        let out = StdCommand::new("git")
            .args(["merge", "merged-br"])
            .current_dir(&source)
            .output()
            .unwrap();
        assert!(out.status.success());

        // 2. Squash-merged branch
        commit_on_branch(&source, "squash-br", "s.txt");
        squash_merge(&source, "squash-br", "main");

        // 3. Pushed but unmerged branch (exists on remote but not merged)
        commit_on_branch(&source, "pushed-br", "p.txt");
        let out = StdCommand::new("git")
            .args(["checkout", "main"])
            .current_dir(&source)
            .output()
            .unwrap();
        assert!(out.status.success());

        // Fetch everything into bare — creates refs/remotes/origin/* for all branches
        fetch(&bare, true).unwrap();

        // Create local branches (refs/heads/*) mirroring the remote tracking refs.
        // This simulates what workspace clones do: the workspace branch is a
        // local branch that may or may not have a corresponding origin/<branch>.
        for name in &["merged-br", "squash-br", "pushed-br"] {
            let sha = run(Some(&bare), &["rev-parse", &format!("origin/{}", name)]).unwrap();
            run(Some(&bare), &["branch", name, &sha]).unwrap();
        }

        // 4. Unmerged local-only branch (no remote ref)
        let main_sha = run(Some(&bare), &["rev-parse", "origin/main"]).unwrap();
        run(Some(&bare), &["branch", "local-only", &main_sha]).unwrap();
        // Add a commit to make it diverge
        let tree = run(Some(&bare), &["rev-parse", "local-only^{tree}"]).unwrap();
        let env = [
            ("GIT_AUTHOR_NAME", "wsp"),
            ("GIT_AUTHOR_EMAIL", "wsp@localhost"),
            ("GIT_COMMITTER_NAME", "wsp"),
            ("GIT_COMMITTER_EMAIL", "wsp@localhost"),
        ];
        let new_commit = run_with_env(
            Some(&bare),
            &["commit-tree", &tree, "-p", "local-only", "-m", "diverge"],
            &env,
        )
        .unwrap();
        run(
            Some(&bare),
            &["update-ref", "refs/heads/local-only", &new_commit],
        )
        .unwrap();

        // All cases use local branch names (refs/heads/*), matching real workspace usage
        let cases = vec![
            ("merged-br", "origin/main", BranchSafety::Merged),
            ("squash-br", "origin/main", BranchSafety::SquashMerged),
            ("pushed-br", "origin/main", BranchSafety::PushedToRemote),
            ("local-only", "origin/main", BranchSafety::Unmerged),
        ];

        for (branch, target, expected) in cases {
            let result = branch_safety(&bare, branch, target);
            assert_eq!(
                result, expected,
                "branch_safety({}, {}) = {:?}, want {:?}",
                branch, target, result, expected
            );
        }
    }

    #[test]
    fn test_is_content_merged_after_squash_merge() {
        let (bare, source, _bt, _st) = setup_bare_repo();

        commit_on_branch(&source, "feature", "feat.txt");
        squash_merge(&source, "feature", "main");
        fetch(&bare, true).unwrap();

        let result = is_content_merged(&bare, "origin/feature", "origin/main").unwrap();
        assert!(result, "squash-merged branch should be content-merged");
    }

    #[test]
    fn test_is_content_merged_false_for_unmerged() {
        let (bare, source, _bt, _st) = setup_bare_repo();

        commit_on_branch(&source, "unmerged", "unmerged.txt");
        fetch(&bare, true).unwrap();

        let result = is_content_merged(&bare, "origin/unmerged", "origin/main").unwrap();
        assert!(!result, "unmerged branch should not be content-merged");
    }

    #[test]
    fn test_is_content_merged_with_diverged_main() {
        let (bare, source, _bt, _st) = setup_bare_repo();

        // Create feature branch
        commit_on_branch(&source, "feature", "feat.txt");

        // Add diverging commits to main (different files)
        let out = StdCommand::new("git")
            .args(["checkout", "main"])
            .current_dir(&source)
            .output()
            .unwrap();
        assert!(out.status.success());
        std::fs::write(source.join("other.txt"), "other content").unwrap();
        for args in &[
            vec!["git", "add", "other.txt"],
            vec!["git", "commit", "-m", "diverge main"],
        ] {
            let out = StdCommand::new(args[0])
                .args(&args[1..])
                .current_dir(&source)
                .output()
                .unwrap();
            assert!(out.status.success());
        }

        // Squash-merge feature into main
        squash_merge(&source, "feature", "main");
        fetch(&bare, true).unwrap();

        // cherry/patch-id may fail here, but content-based detection should work
        let result = is_content_merged(&bare, "origin/feature", "origin/main").unwrap();
        assert!(
            result,
            "squash-merged branch should be content-merged even with diverged main"
        );
    }

    /// Add a commit to the source repo on the given branch and fetch it in the clone.
    fn advance_origin(source: &Path, clone: &Path, branch: &str, file: &str, content: &str) {
        let out = StdCommand::new("git")
            .args(["checkout", branch])
            .current_dir(source)
            .output()
            .unwrap();
        assert!(out.status.success());
        std::fs::write(source.join(file), content).unwrap();
        for args in &[
            vec!["git", "add", file],
            vec!["git", "commit", "-m", &format!("add {}", file)],
        ] {
            let out = StdCommand::new(args[0])
                .args(&args[1..])
                .current_dir(source)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{:?}: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
        }
        fetch_remote(clone, "origin").unwrap();
    }

    #[test]
    fn test_commit_count() {
        let (clone, source, _ct, _st) = setup_clone_repo();
        advance_origin(&source, &clone, "main", "a.txt", "a");
        advance_origin(&source, &clone, "main", "b.txt", "b");

        let count = commit_count(&clone, "HEAD", "origin/main").unwrap();
        assert_eq!(count, 2);

        // Reverse direction
        let count = commit_count(&clone, "origin/main", "HEAD").unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_rebase_onto_up_to_date() {
        let (clone, _source, _ct, _st) = setup_clone_repo();
        // HEAD and origin/main point to the same commit
        let result = rebase_onto(&clone, "origin/main").unwrap();
        assert_eq!(result, SyncAction::UpToDate);
    }

    #[test]
    fn test_rebase_onto_fast_forward() {
        let (clone, source, _ct, _st) = setup_clone_repo();
        advance_origin(&source, &clone, "main", "upstream.txt", "upstream");

        let result = rebase_onto(&clone, "origin/main").unwrap();
        assert_eq!(result, SyncAction::FastForward { commits: 1 });
    }

    #[test]
    fn test_rebase_onto_with_diverged_commits() {
        let (clone, source, _ct, _st) = setup_clone_repo();

        // Local commit on feature branch
        local_commit(&clone, "local.txt", "local");
        // Upstream commit on main
        advance_origin(&source, &clone, "main", "upstream.txt", "upstream");

        let result = rebase_onto(&clone, "origin/main").unwrap();
        assert_eq!(result, SyncAction::Rebased { commits: 1 });
    }

    #[test]
    fn test_rebase_onto_conflict_aborts() {
        let (clone, source, _ct, _st) = setup_clone_repo();

        // Same file, different content → conflict
        local_commit(&clone, "conflict.txt", "local version");
        advance_origin(&source, &clone, "main", "conflict.txt", "upstream version");

        let result = rebase_onto(&clone, "origin/main");
        assert!(result.is_err(), "should fail with conflict");

        // Repo should be clean (rebase aborted)
        let rebase_dir = clone.join(".git").join("rebase-merge");
        assert!(
            !rebase_dir.exists(),
            "rebase-merge dir should not exist after abort"
        );
    }

    #[test]
    fn test_rebase_onto_head_ahead() {
        let (clone, _source, _ct, _st) = setup_clone_repo();

        // HEAD is ahead of origin/main (local commit, no upstream advance)
        local_commit(&clone, "ahead.txt", "ahead");

        let result = rebase_onto(&clone, "origin/main").unwrap();
        assert_eq!(result, SyncAction::UpToDate);
    }

    #[test]
    fn test_merge_from_up_to_date() {
        let (clone, _source, _ct, _st) = setup_clone_repo();
        let result = merge_from(&clone, "origin/main").unwrap();
        assert_eq!(result, SyncAction::UpToDate);
    }

    #[test]
    fn test_merge_from_fast_forward() {
        let (clone, source, _ct, _st) = setup_clone_repo();
        advance_origin(&source, &clone, "main", "upstream.txt", "upstream");

        let result = merge_from(&clone, "origin/main").unwrap();
        assert_eq!(result, SyncAction::FastForward { commits: 1 });
    }

    #[test]
    fn test_merge_from_diverged() {
        let (clone, source, _ct, _st) = setup_clone_repo();

        local_commit(&clone, "local.txt", "local");
        advance_origin(&source, &clone, "main", "upstream.txt", "upstream");

        let result = merge_from(&clone, "origin/main").unwrap();
        assert_eq!(result, SyncAction::Merged);
    }

    #[test]
    fn test_merge_from_conflict_aborts() {
        let (clone, source, _ct, _st) = setup_clone_repo();

        local_commit(&clone, "conflict.txt", "local version");
        advance_origin(&source, &clone, "main", "conflict.txt", "upstream version");

        let result = merge_from(&clone, "origin/main");
        assert!(result.is_err(), "should fail with conflict");

        // Repo should be clean (merge aborted)
        let merge_head = clone.join(".git").join("MERGE_HEAD");
        assert!(
            !merge_head.exists(),
            "MERGE_HEAD should not exist after abort"
        );
    }

    #[test]
    fn test_push_to_remote() {
        let (clone, source, _ct, _st) = setup_clone_repo();

        // Push without upstream (feature branch has no tracking branch)
        local_commit(&clone, "push-test.txt", "push content");
        push(&clone, "origin", "feature", true, false).unwrap();

        // Verify the commit arrived at the source
        let out = StdCommand::new("git")
            .args(["log", "--oneline", "feature"])
            .current_dir(&source)
            .output()
            .unwrap();
        assert!(out.status.success());
        let log = String::from_utf8_lossy(&out.stdout);
        assert!(
            log.contains("push-test.txt"),
            "source should have the pushed commit"
        );

        // Now the tracking branch exists — push a second commit without --set-upstream
        local_commit(&clone, "push-test2.txt", "more content");
        push(&clone, "origin", "feature", false, false).unwrap();

        let out = StdCommand::new("git")
            .args(["log", "--oneline", "feature"])
            .current_dir(&source)
            .output()
            .unwrap();
        assert!(out.status.success());
        let log = String::from_utf8_lossy(&out.stdout);
        assert!(
            log.contains("push-test2.txt"),
            "source should have the second pushed commit"
        );
    }

    #[test]
    fn test_push_force_with_lease() {
        let (clone, source, _ct, _st) = setup_clone_repo();

        // Initial push to create the remote branch
        local_commit(&clone, "fwl.txt", "v1");
        push(&clone, "origin", "feature", true, false).unwrap();

        // Amend the commit (rewrite history)
        std::fs::write(clone.join("fwl.txt"), "v2").unwrap();
        let out = StdCommand::new("git")
            .args(["add", "fwl.txt"])
            .current_dir(&clone)
            .output()
            .unwrap();
        assert!(out.status.success());
        let out = StdCommand::new("git")
            .args(["commit", "--amend", "-m", "add fwl.txt (amended)"])
            .current_dir(&clone)
            .output()
            .unwrap();
        assert!(out.status.success());

        // Force-with-lease push should succeed (no competing changes)
        push(&clone, "origin", "feature", false, true).unwrap();

        // Verify amended content arrived
        let out = StdCommand::new("git")
            .args(["show", "feature:fwl.txt"])
            .current_dir(&source)
            .output()
            .unwrap();
        assert!(out.status.success());
        let content = String::from_utf8_lossy(&out.stdout);
        assert_eq!(content.trim(), "v2", "source should have amended content");
    }
}
