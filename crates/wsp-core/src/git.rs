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

/// Validate that a string is a valid git branch name.
/// Uses `git check-ref-format` with the `--branch` flag so bare names
/// (without `refs/heads/` prefix) are accepted.
pub fn validate_branch_name(name: &str) -> Result<()> {
    let output = Command::new("git")
        .args(["check-ref-format", "--branch", name])
        .output()?;
    if !output.status.success() {
        bail!("{:?} is not a valid git branch name", name);
    }
    Ok(())
}

/// Execute a git command and return trimmed stdout.
///
/// # Security
///
/// All arguments are passed directly to `git` via `std::process::Command` —
/// there is no shell involved. However, certain git subcommands can execute
/// arbitrary code (e.g. `config core.sshCommand`, `config core.hooksPath`,
/// `config diff.external`). **Callers must never pass untrusted input as
/// arguments.** All call sites in this library use only statically-known
/// argument strings or values that have been validated upstream.
pub fn run(dir: Option<&Path>, args: &[&str]) -> Result<String> {
    run_with_env(dir, args, &[])
}

/// Execute a git command with additional environment variables.
///
/// # Security
///
/// See [`run`] — the same trust constraints apply. Additionally, `env` keys
/// and values are injected directly into the child process environment without
/// sanitization. Callers must not derive these from untrusted input.
pub(crate) fn run_with_env(
    dir: Option<&Path>,
    args: &[&str],
    env: &[(&str, &str)],
) -> Result<String> {
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
    // Clear any existing refspecs first (ignore error if none exist)
    let _ = run(Some(dir), &["config", "--unset-all", "remote.origin.fetch"]);
    // Keep refs/heads/* in sync so git clone --local gets a current checkout
    run(
        Some(dir),
        &[
            "config",
            "remote.origin.fetch",
            "+refs/heads/*:refs/heads/*",
        ],
    )?;
    // Also map upstream branches into refs/remotes/origin/* for workspace clones
    run(
        Some(dir),
        &[
            "config",
            "--add",
            "remote.origin.fetch",
            "+refs/heads/*:refs/remotes/origin/*",
        ],
    )?;
    Ok(())
}

fn ensure_fetch_refspec(dir: &Path) -> Result<()> {
    let refspecs =
        run(Some(dir), &["config", "--get-all", "remote.origin.fetch"]).unwrap_or_default();
    // Reconfigure if missing entirely or missing the refs/heads→refs/heads mapping
    // (old mirrors only had +refs/heads/*:refs/remotes/origin/*)
    if !refspecs.contains("+refs/heads/*:refs/heads/*") {
        configure_fetch_refspec(dir)?;
    }
    Ok(())
}

/// Write a local git config key/value.
///
/// # Security
///
/// `key` and `value` are passed directly to `git config --local`. Git config
/// keys such as `core.sshCommand`, `core.hooksPath`, `core.pager`, and
/// `diff.external` can execute arbitrary commands on subsequent git operations.
/// **Callers must never derive `key` or `value` from untrusted input.**
pub(crate) fn set_config(dir: &Path, key: &str, value: &str) -> Result<()> {
    run(Some(dir), &["config", "--local", key, value])?;
    Ok(())
}

/// Read a local git config value. Returns Err if the key is not set.
pub fn get_config(dir: &Path, key: &str) -> Result<String> {
    run(Some(dir), &["config", "--local", key])
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
    strip_ref_branch(&ref_str, "origin")
}

/// Fetch from a local path with an explicit refspec, leaving no remote configured.
pub fn fetch_from_path(dir: &Path, source_path: &Path, refspec: &str, prune: bool) -> Result<()> {
    let src = path_str(source_path)?;
    let mut args = vec!["fetch"];
    if prune {
        args.push("--prune");
    }
    args.push("--");
    args.push(src);
    args.push(refspec);
    run(Some(dir), &args)?;
    Ok(())
}

/// Read the default branch from a bare mirror's refs/remotes/origin/HEAD.
///
/// Bare mirrors cloned with `git clone --bare` write `refs/remotes/origin/HEAD`
/// as a symref pointing to `refs/heads/<branch>` (not `refs/remotes/origin/<branch>`).
/// The `refs/heads/` fallback arm handles this case — it is load-bearing and must
/// not be removed as "dead code".
pub fn default_branch_from_mirror(mirror_dir: &Path) -> Result<String> {
    let ref_str = run(
        Some(mirror_dir),
        &["symbolic-ref", "refs/remotes/origin/HEAD"],
    )?;
    strip_ref_branch(&ref_str, "origin")
}

/// Strip the remote-tracking or heads prefix from a symbolic-ref output, returning
/// just the branch name (which may itself contain slashes, e.g. `release/2.x`).
fn strip_ref_branch(ref_str: &str, remote: &str) -> Result<String> {
    let remote_prefix = format!("refs/remotes/{}/", remote);
    ref_str
        .strip_prefix(remote_prefix.as_str())
        .or_else(|| ref_str.strip_prefix("refs/heads/"))
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("unexpected ref format: {}", ref_str))
}

/// Check whether a named remote exists in a repo.
pub fn has_remote(dir: &Path, name: &str) -> bool {
    run(Some(dir), &["remote", "get-url", name]).is_ok()
}

/// Return the URL configured for a named remote.
pub fn remote_get_url(dir: &Path, name: &str) -> Result<String> {
    run(Some(dir), &["remote", "get-url", name])
}

/// Remove a named remote. Errors if the remote does not exist.
pub fn remove_remote(dir: &Path, name: &str) -> Result<()> {
    run(Some(dir), &["remote", "remove", name])?;
    Ok(())
}

/// Set the URL for an existing remote.
pub fn remote_set_url(dir: &Path, remote: &str, url: &str) -> Result<()> {
    run(Some(dir), &["remote", "set-url", remote, url])?;
    Ok(())
}

pub fn clone_local(mirror_dir: &Path, dest: &Path) -> Result<()> {
    let src = path_str(mirror_dir)?;
    let dst = path_str(dest)?;
    run(None, &["clone", "--local", src, dst])?;
    Ok(())
}

// `#[cfg(test)]` (not `any(test, feature = "test-utils")`) is intentional
// here: this helper is only used by wsp-core's own unit tests and is not
// needed by the binary crate's test suite. If it ever is needed cross-crate,
// promote the gate to `any(test, feature = "test-utils")` like fetch_remote_prune.
#[cfg(test)]
pub fn fetch_remote(dir: &Path, remote: &str) -> Result<()> {
    run(Some(dir), &["fetch", remote])?;
    Ok(())
}

// `#[cfg(test)]` alone would make this invisible to the binary crate's test
// suite — `cfg(test)` items in a dependency are not compiled when running
// tests in a dependent crate. Use `any(test, feature = "test-utils")` for
// any helper that must be callable from crates/wsp tests. The binary crate's
// dev-dependencies declare `wsp-core = { features = ["test-utils"] }`.
#[cfg(any(test, feature = "test-utils"))]
#[doc(hidden)]
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

/// Like [`checkout_new_branch`] but uses `--track` instead of `--no-track`,
/// so the new branch automatically tracks `start_point` for push/pull.
/// Use only when `start_point` is the correct upstream (e.g. `origin/<branch>`
/// for a branch that already exists remotely).
pub(crate) fn checkout_new_branch_tracking(
    dir: &Path,
    branch: &str,
    start_point: &str,
) -> Result<()> {
    run(
        Some(dir),
        &["checkout", "-b", branch, "--track", start_point],
    )?;
    Ok(())
}

pub fn checkout_orphan(dir: &Path, branch: &str) -> Result<()> {
    run(Some(dir), &["checkout", "--orphan", branch])?;
    Ok(())
}

pub fn branch_rename(dir: &Path, old: &str, new: &str) -> Result<()> {
    run(Some(dir), &["branch", "-m", old, new])?;
    Ok(())
}

pub fn checkout(dir: &Path, ref_or_branch: &str) -> Result<()> {
    run(Some(dir), &["checkout", ref_or_branch])?;
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

    strip_ref_branch(&ref_str, remote)
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
    let mb = match try_merge_base(dir, branch, target)? {
        Some(mb) => mb,
        None => return Ok(false), // unrelated histories cannot be squash-merged
    };
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
    let mb = match try_merge_base(dir, branch, target)? {
        Some(mb) => mb,
        None => return Ok(false), // unrelated histories cannot have content merged
    };
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
/// Fails closed: any git error during the merge probes returns `Unmerged`
/// rather than silently downgrading to `PushedToRemote`.
pub fn branch_safety(dir: &Path, branch: &str, target: &str) -> BranchSafety {
    match branch_is_merged(dir, branch, target) {
        Ok(true) => return BranchSafety::Merged,
        Ok(false) => {}
        Err(_) => return BranchSafety::Unmerged,
    }
    match branch_is_squash_merged(dir, branch, target) {
        Ok(true) => return BranchSafety::SquashMerged,
        Ok(false) => {}
        Err(_) => return BranchSafety::Unmerged,
    }
    match is_content_merged(dir, branch, target) {
        Ok(true) => return BranchSafety::SquashMerged,
        Ok(false) => {}
        Err(_) => return BranchSafety::Unmerged,
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

pub(crate) fn update_ref(dir: &Path, refname: &str, target: &str) -> Result<()> {
    run(Some(dir), &["update-ref", "--no-deref", refname, target])?;
    Ok(())
}

pub fn is_ancestor(dir: &Path, ancestor: &str, descendant: &str) -> bool {
    run(
        Some(dir),
        &["merge-base", "--is-ancestor", ancestor, descendant],
    )
    .is_ok()
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
///
/// Resolution order:
/// 1. `@{upstream}` — tracking branch is configured → `Tracking`
/// 2. `default_branch()` — tries `refs/remotes/origin/HEAD`, then
///    `git symbolic-ref HEAD` as fallback → `DefaultBranch`
/// 3. Both fail (e.g. origin/HEAD absent AND detached HEAD) → `Head`
///
/// In practice `Head` is rare: `default_branch` falls back to
/// `symbolic-ref HEAD`, which succeeds on any non-detached checkout.
/// Tests that need `Head` must remove origin/HEAD *and* detach HEAD.
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

/// Like `merge_base`, but distinguishes "no common ancestor" (exit 1) from a
/// true git error (exit 128). Returns `Ok(None)` for unrelated histories and
/// `Err` only for genuine failures.
fn try_merge_base(dir: &Path, a: &str, b: &str) -> Result<Option<String>> {
    let mut cmd = Command::new("git");
    cmd.args(["merge-base", a, b]);
    cmd.current_dir(dir);
    let output = cmd.output()?;
    match output.status.code() {
        Some(0) => Ok(Some(
            String::from_utf8_lossy(&output.stdout).trim().to_string(),
        )),
        Some(1) => Ok(None), // unrelated histories — no common ancestor
        _ => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            bail!(
                "git merge-base (in {}): {}\n{}",
                dir.display(),
                output.status,
                stderr
            )
        }
    }
}

pub fn ahead_count(dir: &Path) -> Result<u32> {
    ahead_count_from(dir, &resolve_upstream_ref(dir))
}

pub fn ahead_count_from(dir: &Path, upstream: &UpstreamRef) -> Result<u32> {
    let range = match upstream {
        UpstreamRef::Tracking => "@{upstream}..HEAD".to_string(),
        UpstreamRef::DefaultBranch(b) => format!("origin/{}..HEAD", b),
        // UpstreamRef::Head means no tracking branch and no origin/<default>
        // could be resolved. We return 0 here, which is a conservative
        // choice that lets the caller proceed. The branch_safety check that
        // follows in workspace::remove / remove_repos catches the case where
        // the branch has never been pushed (Unmerged variant), so local-only
        // commits are not silently discarded by the 0 here.
        UpstreamRef::Head => return Ok(0),
    };
    let out = run(Some(dir), &["rev-list", "--count", &range])?;
    Ok(out.parse::<u32>().unwrap_or(0))
}

pub fn behind_count_from(dir: &Path, upstream: &UpstreamRef) -> Result<u32> {
    let range = match upstream {
        UpstreamRef::Tracking => "HEAD..@{upstream}".to_string(),
        UpstreamRef::DefaultBranch(b) => format!("HEAD..origin/{}", b),
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

/// Detect an in-progress rebase or merge and return what kind, if any.
pub enum InProgressOp {
    Rebase,
    Merge,
}

pub fn in_progress_op(dir: &Path) -> Option<InProgressOp> {
    let git_dir = dir.join(".git");
    if git_dir.join("rebase-merge").exists() || git_dir.join("rebase-apply").exists() {
        Some(InProgressOp::Rebase)
    } else if git_dir.join("MERGE_HEAD").exists() {
        Some(InProgressOp::Merge)
    } else {
        None
    }
}

/// Abort an in-progress rebase or merge.
pub fn abort_in_progress(dir: &Path, op: &InProgressOp) -> Result<()> {
    match op {
        InProgressOp::Rebase => run(Some(dir), &["rebase", "--abort"]).map(|_| ()),
        InProgressOp::Merge => run(Some(dir), &["merge", "--abort"]).map(|_| ()),
    }
}

pub fn set_upstream(dir: &Path, branch: &str, upstream: &str) -> Result<()> {
    run(
        Some(dir),
        &["branch", "--set-upstream-to", upstream, branch],
    )?;
    Ok(())
}

pub fn unset_upstream(dir: &Path, branch: &str) -> Result<()> {
    run(Some(dir), &["branch", "--unset-upstream", branch])?;
    Ok(())
}

/// A linked git worktree (not the main working tree).
#[derive(Debug, Clone)]
pub struct LinkedWorktree {
    /// Absolute path to the worktree's working directory.
    pub path: std::path::PathBuf,
    /// The branch checked out in this worktree, or `None` for detached HEAD.
    pub branch: Option<String>,
}

/// Return all linked worktrees for a git repository.
///
/// Parses `git worktree list --porcelain`. The main worktree (at `dir`) is
/// excluded — only linked worktrees are returned. Returns an empty vec if
/// there are none; returns `Err` if the git command fails.
pub fn list_linked_worktrees(dir: &Path) -> Result<Vec<LinkedWorktree>> {
    let out = run(Some(dir), &["worktree", "list", "--porcelain"])?;
    let mut result = Vec::new();

    // Each worktree entry is a blank-line-separated stanza. The first stanza
    // is always the main worktree; skip it (i == 0).
    for (i, stanza) in out.split("\n\n").enumerate() {
        if i == 0 || stanza.trim().is_empty() {
            continue;
        }
        let mut path = None;
        let mut branch = None;
        for line in stanza.lines() {
            if let Some(p) = line.strip_prefix("worktree ") {
                path = Some(std::path::PathBuf::from(p));
            } else if let Some(b) = line.strip_prefix("branch refs/heads/") {
                branch = Some(b.to_string());
            }
        }
        if let Some(p) = path {
            result.push(LinkedWorktree { path: p, branch });
        }
    }

    Ok(result)
}

pub fn changed_file_count(dir: &Path) -> Result<u32> {
    let out = run(Some(dir), &["status", "--short"])?;
    if out.is_empty() {
        Ok(0)
    } else {
        Ok(out.lines().count() as u32)
    }
}

pub fn changed_files(dir: &Path) -> Result<Vec<String>> {
    let out = run(Some(dir), &["status", "--short"])?;
    if out.is_empty() {
        Ok(vec![])
    } else {
        Ok(out.lines().map(|l| l.to_string()).collect())
    }
}

/// List top-level file names in a tree-ish (e.g., HEAD) of a bare repo.
pub fn ls_tree_names(git_dir: &Path, rev: &str) -> Result<Vec<String>> {
    let out = run(Some(git_dir), &["ls-tree", "--name-only", rev])?;
    if out.is_empty() {
        Ok(vec![])
    } else {
        Ok(out.lines().map(|l| l.to_string()).collect())
    }
}

/// Extract file content from a bare repo at a given revision and path.
pub fn show_file(git_dir: &Path, rev: &str, path: &str) -> Result<Vec<u8>> {
    let spec = format!("{}:{}", rev, path);
    let mut cmd = Command::new("git");
    cmd.args(["show", &spec]);
    cmd.current_dir(git_dir);
    let output = cmd.output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        bail!("git show {} (in {}): {}", spec, git_dir.display(), stderr);
    }
    Ok(output.stdout)
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

        // Ensure local branches (refs/heads/*) mirror the remote tracking refs.
        // This simulates what workspace clones do: the workspace branch is a
        // local branch that may or may not have a corresponding origin/<branch>.
        // Use update-ref since the fetch refspec may have already created them.
        for name in &["merged-br", "squash-br", "pushed-br"] {
            let sha = run(Some(&bare), &["rev-parse", &format!("origin/{}", name)]).unwrap();
            run(
                Some(&bare),
                &["update-ref", &format!("refs/heads/{}", name), &sha],
            )
            .unwrap();
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
    fn test_behind_count_from() {
        let (clone, source, _ct, _st) = setup_clone_repo();

        // No upstream commits → 0 behind
        let upstream = resolve_upstream_ref(&clone);
        assert_eq!(behind_count_from(&clone, &upstream).unwrap(), 0);

        // Add 3 upstream commits, fetch → 3 behind
        for i in 0..3 {
            advance_origin(&source, &clone, "main", &format!("up{i}.txt"), "data");
        }
        assert_eq!(behind_count_from(&clone, &upstream).unwrap(), 3);

        // Add local commit → still 3 behind (and 1 ahead)
        local_commit(&clone, "local.txt", "local");
        assert_eq!(behind_count_from(&clone, &upstream).unwrap(), 3);
        assert_eq!(ahead_count_from(&clone, &upstream).unwrap(), 1);
    }

    #[test]
    fn test_in_progress_op_none() {
        let (clone, _source, _ct, _st) = setup_clone_repo();
        assert!(in_progress_op(&clone).is_none());
    }

    #[test]
    fn test_in_progress_op_rebase() {
        let (clone, source, _ct, _st) = setup_clone_repo();

        // Create conflict to leave rebase in progress
        local_commit(&clone, "conflict.txt", "local version");
        advance_origin(&source, &clone, "main", "conflict.txt", "upstream version");

        // Start rebase manually (don't use rebase_onto which auto-aborts)
        let out = StdCommand::new("git")
            .args(["rebase", "origin/main"])
            .current_dir(&clone)
            .output()
            .unwrap();
        assert!(!out.status.success(), "rebase should fail with conflict");

        // Should detect rebase in progress
        let op = in_progress_op(&clone);
        assert!(matches!(op, Some(InProgressOp::Rebase)));

        // Abort and verify clean state
        abort_in_progress(&clone, &op.unwrap()).unwrap();
        assert!(in_progress_op(&clone).is_none());
    }

    #[test]
    fn test_in_progress_op_merge() {
        let (clone, source, _ct, _st) = setup_clone_repo();

        local_commit(&clone, "conflict.txt", "local version");
        advance_origin(&source, &clone, "main", "conflict.txt", "upstream version");

        // Start merge manually (don't use merge_from which auto-aborts)
        let out = StdCommand::new("git")
            .args(["merge", "origin/main"])
            .current_dir(&clone)
            .output()
            .unwrap();
        assert!(!out.status.success(), "merge should fail with conflict");

        // Should detect merge in progress
        let op = in_progress_op(&clone);
        assert!(matches!(op, Some(InProgressOp::Merge)));

        // Abort and verify clean state
        abort_in_progress(&clone, &op.unwrap()).unwrap();
        assert!(in_progress_op(&clone).is_none());
    }

    /// When UpstreamRef::Head is passed to ahead_count_from (no tracking branch
    /// and no origin/<default> could be resolved), it returns 0. This is a
    /// known false-clean result — there is no reference point to count against.
    ///
    /// The downstream branch_safety check is the real safety net: it returns
    /// Unmerged for a branch that was never pushed, blocking workspace removal
    /// even though the first-pass ahead_count check gave an all-clear.
    #[test]
    fn test_upstream_ref_head_reports_zero_ahead_but_branch_safety_blocks() {
        let (clone, _source, _ct, _st) = setup_clone_repo();

        // Create a local-only branch with a commit, then remove origin so there
        // is no remote ref at all. This is the scenario where ahead_count_from
        // falls back to 0 and workspace::remove's unwrap_or(0) gives a false-clean.
        let out = StdCommand::new("git")
            .args(["checkout", "-b", "local-only"])
            .current_dir(&clone)
            .output()
            .unwrap();
        assert!(out.status.success());
        local_commit(&clone, "work.txt", "local work");
        let out = StdCommand::new("git")
            .args(["remote", "remove", "origin"])
            .current_dir(&clone)
            .output()
            .unwrap();
        assert!(out.status.success());

        // Direct test: ahead_count_from returns 0 for UpstreamRef::Head
        // regardless of how many local commits exist. Documented known behavior.
        assert_eq!(
            ahead_count_from(&clone, &UpstreamRef::Head).unwrap(),
            0,
            "ahead_count_from(Head) must return 0 — no reference point available"
        );

        // With origin removed, ahead_count also errors (origin/<default> missing),
        // and workspace::remove uses unwrap_or(0) — same false-clean result.
        assert!(
            ahead_count(&clone).is_err(),
            "ahead_count must error when no remote ref is available"
        );

        // The real safety net: branch_safety returns Unmerged for a branch that
        // was never pushed. workspace::remove blocks on this even if ahead_count
        // reported 0.
        let safety = branch_safety(&clone, "local-only", "main");
        assert!(
            matches!(safety, BranchSafety::Unmerged),
            "branch_safety must return Unmerged for a never-pushed local branch"
        );
    }

    #[test]
    fn test_validate_branch_name() {
        let cases = vec![
            ("simple", "my-feature", true),
            ("with slash", "user/feature", true),
            ("dotted", "fix.bug", true),
            ("bare dot", ".", false),
            ("double dot", "..", false),
            ("leading dot", ".hidden", false),
            ("space", "bad name", false),
            ("tilde", "bad~name", false),
            ("caret", "bad^name", false),
            ("colon", "bad:name", false),
            ("at-brace", "bad@{name", false),
            ("double dot mid", "bad..name", false),
            ("trailing dot-lock", "bad.lock", false),
            ("trailing slash", "bad/", false),
        ];
        for (label, name, want_ok) in cases {
            let result = validate_branch_name(name);
            assert_eq!(result.is_ok(), want_ok, "{}: {:?}", label, result);
        }
    }

    #[test]
    fn test_list_linked_worktrees_none() {
        let (clone, _source, _ct, _st) = setup_clone_repo();
        let wts = list_linked_worktrees(&clone).unwrap();
        assert!(
            wts.is_empty(),
            "fresh clone should have no linked worktrees"
        );
    }

    #[test]
    fn test_list_linked_worktrees_detects_linked() {
        let (clone, _source, _ct, _st) = setup_clone_repo();
        let wt_dir = clone.parent().unwrap().join("side-work");

        // Add a linked worktree on a new branch
        let out = Command::new("git")
            .args(["worktree", "add", wt_dir.to_str().unwrap(), "-b", "side"])
            .current_dir(&clone)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git worktree add: {}",
            String::from_utf8_lossy(&out.stderr)
        );

        let wts = list_linked_worktrees(&clone).unwrap();
        assert_eq!(wts.len(), 1);
        // Canonicalize both sides: git resolves symlinks (e.g. /var → /private/var on macOS)
        assert_eq!(
            wts[0].path.canonicalize().unwrap(),
            wt_dir.canonicalize().unwrap()
        );
        assert_eq!(wts[0].branch.as_deref(), Some("side"));
    }

    /// Creates a commit on an orphan branch (no common ancestor with main) in the source repo.
    fn commit_on_orphan_branch(dir: &Path, branch: &str, file: &str) {
        for args in &[
            vec!["git", "checkout", "--orphan", branch],
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
        // Unstage files inherited from the previous branch
        let _ = StdCommand::new("git")
            .args(["rm", "-rf", "--cached", "."])
            .current_dir(dir)
            .output();
        std::fs::write(dir.join(file), file).unwrap();
        let out = StdCommand::new("git")
            .args(["add", file])
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(out.status.success());
        let out = StdCommand::new("git")
            .args(["commit", "-m", &format!("orphan: {}", file)])
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "orphan commit: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn test_branch_safety_git_error_fails_closed() {
        // A non-existent ref causes git to exit 128 (fatal). branch_safety must
        // fail closed and return Unmerged rather than silently fall to PushedToRemote.
        let (bare, _source, _bt, _st) = setup_bare_repo();
        let result = branch_safety(&bare, "non-existent-branch-xyz", "origin/main");
        assert_eq!(
            result,
            BranchSafety::Unmerged,
            "git error must return Unmerged (fail-closed), not PushedToRemote"
        );
    }

    #[test]
    fn test_branch_safety_orphan_pushed() {
        // An orphan branch (no common ancestor with main) that has been pushed to
        // the remote must be PushedToRemote — code is safe on the remote.
        let (bare, source, _bt, _st) = setup_bare_repo();

        commit_on_orphan_branch(&source, "orphan-feature", "orphan.txt");
        fetch(&bare, true).unwrap();

        // Create a local branch ref so branch_safety can evaluate it
        let sha = run(Some(&bare), &["rev-parse", "origin/orphan-feature"]).unwrap();
        run(
            Some(&bare),
            &["update-ref", "refs/heads/orphan-feature", &sha],
        )
        .unwrap();

        let result = branch_safety(&bare, "orphan-feature", "origin/main");
        assert_eq!(
            result,
            BranchSafety::PushedToRemote,
            "orphan branch on remote should be PushedToRemote (not a hard Unmerged block)"
        );
    }

    #[test]
    fn test_branch_safety_orphan_local_only() {
        // An orphan branch that exists only locally (never pushed) must be Unmerged
        // since the code would be permanently lost on deletion.
        let (bare, source, _bt, _st) = setup_bare_repo();

        commit_on_orphan_branch(&source, "orphan-local", "orphan-local.txt");
        fetch(&bare, true).unwrap();

        let sha = run(Some(&bare), &["rev-parse", "origin/orphan-local"]).unwrap();
        run(
            Some(&bare),
            &["update-ref", "refs/heads/orphan-local", &sha],
        )
        .unwrap();

        // Delete the remote tracking ref — makes it local-only
        run(
            Some(&bare),
            &["update-ref", "-d", "refs/remotes/origin/orphan-local"],
        )
        .unwrap();

        let result = branch_safety(&bare, "orphan-local", "origin/main");
        assert_eq!(
            result,
            BranchSafety::Unmerged,
            "local-only orphan branch must be Unmerged (would be lost on deletion)"
        );
    }

    #[test]
    fn test_try_merge_base_nonexistent_ref_is_err_not_none() {
        // git merge-base with a nonexistent ref exits 128 (fatal), not 1.
        // try_merge_base must propagate this as Err so branch_safety fails
        // closed (Unmerged) rather than silently treating it as Ok(None) and
        // falling through to PushedToRemote.
        let (bare, _source, _bt, _st) = setup_bare_repo();
        let result = try_merge_base(&bare, "definitely-does-not-exist-xyz", "origin/main");
        assert!(
            result.is_err(),
            "nonexistent ref must be Err, not Ok(None): {:?}",
            result
        );
    }
}
