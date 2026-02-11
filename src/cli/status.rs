use std::path::PathBuf;

use anyhow::Result;
use clap::{Arg, ArgMatches, Command};
use clap_complete::engine::ArgValueCandidates;

use crate::config::Paths;
use crate::git;
use crate::giturl;
use crate::output::{self, Output, RepoStatusEntry, StatusOutput};
use crate::workspace;

use super::completers;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::build_cli;
    use crate::config::Paths;
    use std::path::PathBuf;

    fn dummy_paths() -> Paths {
        Paths {
            config_path: PathBuf::from("/nonexistent/config.yaml"),
            mirrors_dir: PathBuf::from("/nonexistent/mirrors"),
            workspaces_dir: PathBuf::from("/nonexistent/workspaces"),
        }
    }

    #[test]
    fn run_with_root_matches_does_not_panic() {
        // When `ws` is run with no subcommand inside a workspace, dispatch
        // passes root-level ArgMatches (which lack a "workspace" arg) to
        // status::run. This must not panic — it should gracefully fall
        // through to workspace detection via cwd.
        let matches = build_cli().get_matches_from(["ws"]);

        // The only thing we're testing is that this doesn't panic.
        // The result depends on whether tests run inside a workspace.
        let _ = run(&matches, &dummy_paths());
    }
}

pub fn cmd() -> Command {
    Command::new("status")
        .about("Git status across workspace repos")
        .arg(Arg::new("workspace").add(ArgValueCandidates::new(completers::complete_workspaces)))
}

pub fn run(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let ws_dir: PathBuf = if let Some(name) = matches.try_get_one::<String>("workspace").ok().flatten() {
        workspace::dir(&paths.workspaces_dir, name)
    } else {
        let cwd = std::env::current_dir()?;
        workspace::detect(&cwd)?
    };

    let meta = workspace::load_metadata(&ws_dir)
        .map_err(|e| anyhow::anyhow!("reading workspace: {}", e))?;

    let mut repos = Vec::new();

    for identity in meta.repos.keys() {
        let parsed = match giturl::Parsed::from_identity(identity) {
            Ok(p) => p,
            Err(e) => {
                repos.push(RepoStatusEntry {
                    name: identity.clone(),
                    branch: String::new(),
                    ahead: 0,
                    changed: 0,
                    has_upstream: false,
                    status: String::new(),
                    error: Some(e.to_string()),
                });
                continue;
            }
        };

        let repo_dir = ws_dir.join(&parsed.repo);

        let branch = git::branch_current(&repo_dir).unwrap_or_else(|_| "?".to_string());
        let upstream = git::resolve_upstream_ref(&repo_dir);
        let has_upstream = matches!(upstream, git::UpstreamRef::Tracking);
        let ahead = git::ahead_count_from(&repo_dir, &upstream).unwrap_or(0);
        let changed = git::changed_file_count(&repo_dir).unwrap_or(0);
        let status = output::format_repo_status(ahead, changed, has_upstream);

        repos.push(RepoStatusEntry {
            name: parsed.repo,
            branch,
            ahead,
            changed,
            has_upstream,
            status,
            error: None,
        });
    }

    Ok(Output::Status(StatusOutput {
        workspace: meta.name,
        branch: meta.branch,
        repos,
    }))
}
