use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

fn path_str(p: &Path) -> Result<&str> {
    p.to_str().context("path contains non-UTF8 characters")
}

pub fn run(dir: Option<&Path>, args: &[&str]) -> Result<String> {
    let mut cmd = Command::new("git");
    cmd.args(args);
    if let Some(d) = dir {
        cmd.current_dir(d);
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

pub fn worktree_add(
    repo_dir: &Path,
    worktree_path: &Path,
    branch: &str,
    start_point: &str,
) -> Result<()> {
    let wt = path_str(worktree_path)?;
    run(
        Some(repo_dir),
        &["worktree", "add", "-b", branch, "--", wt, start_point],
    )?;
    Ok(())
}

pub fn worktree_add_existing(repo_dir: &Path, worktree_path: &Path, branch: &str) -> Result<()> {
    let wt = path_str(worktree_path)?;
    run(Some(repo_dir), &["worktree", "add", "--", wt, branch])?;
    Ok(())
}

pub fn worktree_add_detached(repo_dir: &Path, worktree_path: &Path, git_ref: &str) -> Result<()> {
    let wt = path_str(worktree_path)?;
    run(
        Some(repo_dir),
        &["worktree", "add", "--detach", "--", wt, git_ref],
    )?;
    Ok(())
}

pub fn worktree_move(repo_dir: &Path, old_path: &Path, new_path: &Path) -> Result<()> {
    let old = path_str(old_path)?;
    let new = path_str(new_path)?;
    run(Some(repo_dir), &["worktree", "move", old, new])?;
    Ok(())
}

pub fn worktree_remove(repo_dir: &Path, worktree_path: &Path) -> Result<()> {
    let wt = path_str(worktree_path)?;
    run(Some(repo_dir), &["worktree", "remove", "--force", wt])?;
    Ok(())
}

pub fn branch_delete(dir: &Path, branch: &str) -> Result<()> {
    run(Some(dir), &["branch", "-D", "--", branch])?;
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

pub fn branch_exists(dir: &Path, branch: &str) -> bool {
    let ref_path = format!("refs/heads/{}", branch);
    run(Some(dir), &["rev-parse", "--verify", &ref_path]).is_ok()
}

pub fn ref_exists(dir: &Path, git_ref: &str) -> bool {
    run(Some(dir), &["rev-parse", "--verify", git_ref]).is_ok()
}

// TODO: unused currently, will be used by future commands
#[allow(dead_code)]
pub fn status(dir: &Path) -> Result<String> {
    run(Some(dir), &["status", "--short"])
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

pub fn changed_file_count(dir: &Path) -> Result<u32> {
    let out = run(Some(dir), &["status", "--short"])?;
    if out.is_empty() {
        Ok(0)
    } else {
        Ok(out.lines().count() as u32)
    }
}
