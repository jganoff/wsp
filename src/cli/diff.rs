use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::{Arg, ArgMatches, Command};

use crate::config::Paths;
use crate::git;
use crate::output::{DiffOutput, Output, RepoDiffEntry};
use crate::workspace;

pub fn cmd() -> Command {
    Command::new("diff")
        .about("Show git diff across workspace repos")
        .arg(Arg::new("workspace"))
        .arg(
            Arg::new("args")
                .num_args(1..)
                .last(true)
                .allow_hyphen_values(true),
        )
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

    let extra_args: Vec<&str> = matches
        .get_many::<String>("args")
        .map(|vals| vals.map(|s| s.as_str()).collect())
        .unwrap_or_default();

    let mut repos = Vec::new();
    for identity in meta.repos.keys() {
        let dir_name = match meta.dir_name(identity) {
            Ok(d) => d,
            Err(e) => {
                repos.push(RepoDiffEntry {
                    name: identity.clone(),
                    diff: String::new(),
                    error: Some(e.to_string()),
                });
                continue;
            }
        };

        let repo_dir = ws_dir.join(&dir_name);

        let mut args = vec!["diff"];
        let diff_base = if extra_args.is_empty() {
            Some(resolve_diff_base(&repo_dir))
        } else {
            None
        };
        if let Some(ref base) = diff_base {
            args.push(base);
        }
        args.extend(&extra_args);

        let diff = match git::run(Some(&repo_dir), &args) {
            Ok(o) => o,
            Err(e) => {
                repos.push(RepoDiffEntry {
                    name: dir_name,
                    diff: String::new(),
                    error: Some(e.to_string()),
                });
                continue;
            }
        };

        repos.push(RepoDiffEntry {
            name: dir_name,
            diff,
            error: None,
        });
    }

    Ok(Output::Diff(DiffOutput { repos }))
}

/// Pick the best ref to diff against: upstream tracking branch if set,
/// otherwise origin/<default_branch>.
fn resolve_diff_base(repo_dir: &Path) -> String {
    match git::resolve_upstream_ref(repo_dir) {
        git::UpstreamRef::Tracking => "@{upstream}".to_string(),
        git::UpstreamRef::DefaultBranch(b) => format!("origin/{}", b),
        git::UpstreamRef::Head => "HEAD".to_string(),
    }
}
