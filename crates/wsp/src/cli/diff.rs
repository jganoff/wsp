use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::{Arg, ArgMatches, Command};
use clap_complete::engine::ArgValueCandidates;

use crate::output::print_gc_warning;
use wsp_core::config::Paths;
use wsp_core::gc;
use wsp_core::git;
use wsp_core::output::{DiffOutput, Output, RepoDiffEntry};
use wsp_core::workspace;

use super::completers;

pub fn cmd() -> Command {
    Command::new("diff")
        .about("Show git diff across workspace repos [read-only]")
        .long_about(
            "Show git diff across workspace repos [read-only].\n\n\
             Runs `git diff` in each repo and aggregates the output. By default, diffs \
             against the merge-base with the upstream branch so only changes introduced \
             by this workspace branch are shown.\n\n\
             Extra arguments after `--` are forwarded to git diff:\n\n  \
             wsp diff -- --staged          # staged changes only\n  \
             wsp diff -- --name-only       # list changed filenames\n  \
             wsp diff -- --stat            # diffstat summary\n  \
             wsp diff -- -- path/to/file   # diff a specific file",
        )
        .arg(Arg::new("workspace").add(ArgValueCandidates::new(completers::complete_workspaces)))
        .arg(
            Arg::new("args")
                .num_args(1..)
                .last(true)
                .allow_hyphen_values(true)
                .help("Extra args forwarded to git diff (e.g., -- --staged, -- --name-only)"),
        )
}

pub fn run(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let ws_dir: PathBuf = if let Some(name) = matches.get_one::<String>("workspace") {
        workspace::dir(&paths.workspaces_dir, name)
    } else {
        let cwd = std::env::current_dir()?;
        workspace::detect(&cwd)?
    };

    if let Some(warning) = gc::check_workspace(&ws_dir, /* read_only */ true)? {
        print_gc_warning(&warning);
    }

    let meta = workspace::load_metadata(&ws_dir)
        .map_err(|e| anyhow::anyhow!("reading workspace: {}", e))?;

    let extra_args: Vec<&str> = matches
        .get_many::<String>("args")
        .map(|vals| vals.map(|s| s.as_str()).collect())
        .unwrap_or_default();

    let is_json = matches.get_flag("json");
    let use_color = !is_json && std::io::stdout().is_terminal();

    let mut repos = Vec::new();
    for identity in meta.repos.keys() {
        let dir_name = match meta.dir_name(identity) {
            Ok(d) => d,
            Err(e) => {
                repos.push(RepoDiffEntry {
                    identity: identity.clone(),
                    shortname: identity.rsplit('/').next().unwrap_or(identity).to_string(),
                    path: String::new(),
                    diff: String::new(),
                    error: Some(e.to_string()),
                });
                continue;
            }
        };

        let repo_dir = ws_dir.join(&dir_name);

        let mut args = vec!["diff"];
        if use_color {
            args.push("--color=always");
        }
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
                    identity: identity.clone(),
                    shortname: dir_name,
                    path: repo_dir.to_string_lossy().to_string(),
                    diff: String::new(),
                    error: Some(e.to_string()),
                });
                continue;
            }
        };

        repos.push(RepoDiffEntry {
            identity: identity.clone(),
            shortname: dir_name,
            path: repo_dir.to_string_lossy().to_string(),
            diff,
            error: None,
        });
    }

    Ok(Output::Diff(DiffOutput {
        workspace: meta.name,
        branch: meta.branch,
        workspace_dir: ws_dir,
        repos,
    }))
}

/// Pick the best ref to diff against: the merge-base between the upstream
/// ref and HEAD, so only changes introduced by this branch are shown.
fn resolve_diff_base(repo_dir: &Path) -> String {
    let upstream = match git::resolve_upstream_ref(repo_dir) {
        git::UpstreamRef::Tracking => "@{upstream}".to_string(),
        git::UpstreamRef::DefaultBranch(b) => format!("origin/{}", b),
        git::UpstreamRef::Head => return "HEAD".to_string(),
    };
    // Use merge-base so we only show changes introduced by this branch,
    // not changes that landed on the upstream since the branch diverged.
    git::merge_base(repo_dir, &upstream, "HEAD").unwrap_or(upstream)
}
