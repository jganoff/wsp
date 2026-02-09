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

pub fn cmd() -> Command {
    Command::new("status")
        .about("Git status across workspace repos")
        .arg(Arg::new("workspace").add(ArgValueCandidates::new(completers::complete_workspaces)))
}

pub fn run(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let ws_dir: PathBuf = if let Some(name) = matches.get_one::<String>("workspace") {
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
                    status: String::new(),
                    error: Some(e.to_string()),
                });
                continue;
            }
        };

        let repo_dir = ws_dir.join(&parsed.repo);

        let branch = git::branch_current(&repo_dir).unwrap_or_else(|_| "?".to_string());
        let ahead = git::ahead_count(&repo_dir).unwrap_or(0);
        let changed = git::changed_file_count(&repo_dir).unwrap_or(0);
        let status = output::format_repo_status(ahead, changed);

        repos.push(RepoStatusEntry {
            name: parsed.repo,
            branch,
            ahead,
            changed,
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
