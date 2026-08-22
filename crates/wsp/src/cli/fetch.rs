use std::path::PathBuf;
use std::sync::Mutex;

use anyhow::{Result, bail};
use clap::{ArgMatches, Command};

use wsp_core::config::{self, Paths};
use wsp_core::gc;
use wsp_core::git;
use wsp_core::giturl;
use wsp_core::mirror;
use wsp_core::output::{FetchOutput, FetchRepoResult, Output};
use wsp_core::workspace;

/// Fetch each mirror from upstream in parallel, reporting each result as it
/// arrives.
///
/// `wsp new` and `wsp add` both build their clones from local bare mirrors, so
/// without this they reproduce whatever the mirror last saw. That affects more
/// than file contents: `wsp add` decides whether to track the workspace branch
/// by looking for `refs/remotes/origin/<branch>` in the mirror, so a stale
/// mirror makes a recently pushed branch look nonexistent and starts a fresh
/// one from origin/default instead.
///
/// Failures are reported but not fatal — a mirror that cannot be reached still
/// has its previous contents, which is better than refusing to create the
/// workspace.
pub(crate) fn prefetch_mirrors(mirrors: &[(String, PathBuf)]) {
    if mirrors.is_empty() {
        return;
    }
    eprintln!("Fetching {} mirrors...", mirrors.len());
    let progress = Mutex::new(());
    std::thread::scope(|s| {
        let handles: Vec<_> = mirrors
            .iter()
            .map(|(id, mirror_dir)| {
                let progress = &progress;
                s.spawn(move || {
                    let result = git::fetch(mirror_dir, true);
                    let _lock = progress.lock().unwrap_or_else(|e| e.into_inner());
                    match &result {
                        Ok(()) => eprintln!("  ok    {}", id),
                        Err(e) => eprintln!("  FAIL  {} ({})", id, e),
                    }
                })
            })
            .collect();
        for h in handles {
            let _ = h.join();
        }
    });
}

pub fn cmd() -> Command {
    Command::new("fetch")
        .about("Fetch updates for workspace repos")
        .long_about(
            "Fetch updates for workspace repos.\n\n\
             Fetches from upstream into the bare mirror, then propagates to each clone via \
             local path-based fetch. This two-layer fetch means upstream is only contacted \
             once per repo, regardless of how many workspaces share it.",
        )
        .arg(
            clap::Arg::new("all")
                .long("all")
                .action(clap::ArgAction::SetTrue)
                .help("Fetch all registered repos (not just current workspace)"),
        )
        .arg(
            clap::Arg::new("prune")
                .long("prune")
                .action(clap::ArgAction::SetTrue)
                .help("Prune deleted remote branches"),
        )
}

pub fn run(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let all = matches.get_flag("all");
    let prune = matches.get_flag("prune");

    // Detect current workspace (if not --all)
    let current_ws: Option<(std::path::PathBuf, workspace::Metadata)> = if !all {
        let cwd = std::env::current_dir()?;
        match workspace::detect(&cwd) {
            Ok(ws_dir) => {
                gc::check_workspace(&ws_dir, /* read_only */ false)?;
                let meta = workspace::load_metadata(&ws_dir)?;
                Some((ws_dir, meta))
            }
            Err(_) => None,
        }
    } else {
        None
    };

    // Loaded once: --all reads the repo list from it, and ref propagation uses
    // it to tell an unregistered repo from a registered one whose mirror is
    // gone. A workspace-scoped fetch still works without a readable config —
    // only the precision of that warning suffers.
    let cfg = match config::Config::load_from(&paths.config_path) {
        Ok(cfg) => cfg,
        Err(e) if all => bail!("loading config: {}", e),
        Err(_) => config::Config::default(),
    };

    let identities: Vec<String> = if all {
        cfg.repos.keys().cloned().collect()
    } else {
        match &current_ws {
            Some((_, meta)) => meta.repos.keys().cloned().collect(),
            None => bail!("not in a workspace, use --all to fetch all registered repos"),
        }
    };

    if identities.is_empty() {
        return Ok(Output::Fetch(FetchOutput {
            workspace: current_ws
                .as_ref()
                .map(|(_, m)| m.name.clone())
                .unwrap_or_default(),
            repos: vec![],
        }));
    }

    // Phase 1: Fetch mirrors (network, parallel)
    let repos: Vec<(String, std::path::PathBuf)> = identities
        .into_iter()
        .filter_map(|id| match giturl::Parsed::from_identity(&id) {
            Ok(parsed) => Some((id, mirror::dir(&paths.mirrors_dir, &parsed))),
            Err(e) => {
                eprintln!("  {}: error parsing identity: {}", id, e);
                None
            }
        })
        .collect();

    let ids: Vec<String> = repos.iter().map(|(id, _)| id.clone()).collect();
    let shortnames = giturl::shortnames(&ids);

    if repos.len() == 1 {
        let name = shortnames
            .get(&repos[0].0)
            .map(|s| s.as_str())
            .unwrap_or(&repos[0].0);
        eprintln!("Fetching {}...", name);
    } else {
        eprintln!("Fetching {} repos...", repos.len());
    }

    let progress = Mutex::new(());
    let results: Vec<(String, Result<()>)> = std::thread::scope(|s| {
        let handles: Vec<_> = repos
            .iter()
            .map(|(id, mirror_dir)| {
                let progress = &progress;
                let shortnames = &shortnames;
                s.spawn(move || {
                    let result = git::fetch(mirror_dir, prune);
                    let _lock = progress.lock().unwrap_or_else(|e| e.into_inner());
                    let name = shortnames.get(id).map(|s| s.as_str()).unwrap_or(id);
                    match &result {
                        Ok(()) => eprintln!("  ok    {}", name),
                        Err(e) => eprintln!("  FAIL  {} ({})", name, e),
                    }
                    result
                })
            })
            .collect();

        repos
            .iter()
            .zip(handles)
            .map(|((id, _), h)| {
                (
                    id.clone(),
                    h.join().unwrap_or_else(|panic_val| {
                        let msg = panic_val
                            .downcast_ref::<&str>()
                            .map(|s| s.to_string())
                            .or_else(|| panic_val.downcast_ref::<String>().cloned())
                            .unwrap_or_else(|| "unknown panic".to_string());
                        Err(anyhow::anyhow!("thread panicked: {}", msg))
                    }),
                )
            })
            .collect()
    });

    // Phase 2: Propagate mirror refs to workspace clones
    if all {
        // Propagate to all workspaces
        if let Ok(ws_names) = workspace::list_all(&paths.workspaces_dir) {
            for ws_name in &ws_names {
                let ws_dir = workspace::dir(&paths.workspaces_dir, ws_name);
                if let Ok(meta) = workspace::load_metadata(&ws_dir) {
                    workspace::propagate_mirror_to_clones(
                        &paths.mirrors_dir,
                        &ws_dir,
                        &meta,
                        &cfg,
                        prune,
                    );
                }
            }
        }
    } else if let Some((ws_dir, meta)) = &current_ws {
        workspace::propagate_mirror_to_clones(&paths.mirrors_dir, ws_dir, meta, &cfg, prune);
    }

    let output = FetchOutput {
        workspace: current_ws
            .as_ref()
            .map(|(_, m)| m.name.clone())
            .unwrap_or_default(),
        repos: results
            .into_iter()
            .map(|(id, result)| {
                let name = shortnames.get(&id).cloned().unwrap_or_else(|| id.clone());
                FetchRepoResult {
                    identity: id,
                    shortname: name,
                    ok: result.is_ok(),
                    error: result.err().map(|e| e.to_string()),
                }
            })
            .collect(),
    };

    Ok(Output::Fetch(output))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command as StdCommand;

    fn git_in(dir: &std::path::Path, args: &[&str]) {
        let out = StdCommand::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {:?}: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// An upstream repo with one commit on `main`, plus a bare mirror of it
    /// built the same way `wsp` builds mirrors (`clone_bare` +
    /// `configure_fetch_refspec`), so the refspec behaviour matches production.
    fn upstream_and_mirror() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let upstream = tmp.path().join("upstream");
        std::fs::create_dir_all(&upstream).unwrap();
        git_in(&upstream, &["init", "--initial-branch=main"]);
        git_in(&upstream, &["config", "user.email", "test@test.local"]);
        git_in(&upstream, &["config", "user.name", "Test"]);
        git_in(&upstream, &["config", "commit.gpgsign", "false"]);
        git_in(&upstream, &["commit", "--allow-empty", "-m", "initial"]);

        let mirror = tmp.path().join("mirror.git");
        git::clone_bare(upstream.to_str().unwrap(), &mirror).unwrap();
        git::configure_fetch_refspec(&mirror).unwrap();

        (tmp, upstream, mirror)
    }

    /// The bug this guards: `wsp add` decides whether to track the workspace
    /// branch by looking for `refs/remotes/origin/<branch>` in the mirror. On a
    /// mirror that was never refreshed, a branch pushed since then is invisible,
    /// so a tracking branch silently becomes a fresh one off origin/default.
    #[test]
    fn prefetch_picks_up_a_branch_pushed_after_the_mirror_was_made() {
        let (_tmp, upstream, mirror) = upstream_and_mirror();
        let remote_ref = "refs/remotes/origin/feature/x";

        git_in(&upstream, &["checkout", "-q", "-b", "feature/x"]);
        git_in(&upstream, &["commit", "--allow-empty", "-m", "later work"]);

        assert!(
            !git::ref_exists(&mirror, remote_ref),
            "precondition: a stale mirror must not know the new branch"
        );

        prefetch_mirrors(&[("acme/repo".to_string(), mirror.clone())]);

        assert!(
            git::ref_exists(&mirror, remote_ref),
            "after prefetch the mirror must see the branch that `wsp add` consults"
        );
    }

    /// Also picks up new commits on an existing branch — the plainer symptom of
    /// a stale mirror, where the clone is simply behind.
    #[test]
    fn prefetch_picks_up_new_commits_on_an_existing_branch() {
        let (_tmp, upstream, mirror) = upstream_and_mirror();
        let before = git::run(Some(&mirror), &["rev-parse", "refs/heads/main"]).unwrap();

        git_in(&upstream, &["commit", "--allow-empty", "-m", "later work"]);
        prefetch_mirrors(&[("acme/repo".to_string(), mirror.clone())]);

        let after = git::run(Some(&mirror), &["rev-parse", "refs/heads/main"]).unwrap();
        assert_ne!(
            before, after,
            "mirror should have advanced to the new commit"
        );
    }

    #[test]
    fn prefetch_on_empty_input_does_nothing() {
        prefetch_mirrors(&[]);
    }

    /// Best-effort contract: an unreachable mirror is reported, not fatal, so a
    /// single bad repo cannot block creating the workspace.
    #[test]
    fn prefetch_does_not_panic_when_a_mirror_is_unusable() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("not-a-repo.git");
        prefetch_mirrors(&[("acme/broken".to_string(), missing)]);
    }
}
