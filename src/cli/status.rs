use std::path::PathBuf;

use anyhow::Result;
use clap::{Arg, ArgMatches, Command};

use crate::config::Paths;
use crate::git;
use crate::giturl;
use crate::output;
use crate::workspace;

pub fn cmd() -> Command {
    Command::new("status")
        .about("Git status across workspace repos")
        .arg(Arg::new("workspace"))
}

pub fn run(matches: &ArgMatches, paths: &Paths) -> Result<()> {
    let ws_dir: PathBuf = if let Some(name) = matches.get_one::<String>("workspace") {
        workspace::dir(&paths.workspaces_dir, name)
    } else {
        let cwd = std::env::current_dir()?;
        workspace::detect(&cwd)?
    };

    let meta = workspace::load_metadata(&ws_dir)
        .map_err(|e| anyhow::anyhow!("reading workspace: {}", e))?;

    println!("Workspace: {}  Branch: {}\n", meta.name, meta.branch);

    struct RepoStatus {
        name: String,
        branch: String,
        ahead: u32,
        changed: u32,
        err: Option<String>,
    }

    let mut rows = Vec::new();

    for identity in meta.repos.keys() {
        let parsed = match giturl::Parsed::from_identity(identity) {
            Ok(p) => p,
            Err(e) => {
                rows.push(RepoStatus {
                    name: identity.clone(),
                    branch: String::new(),
                    ahead: 0,
                    changed: 0,
                    err: Some(e.to_string()),
                });
                continue;
            }
        };

        let repo_dir = ws_dir.join(&parsed.repo);

        let branch = git::branch_current(&repo_dir).unwrap_or_else(|_| "?".to_string());
        let ahead = git::ahead_count(&repo_dir).unwrap_or(0);
        let changed = git::changed_file_count(&repo_dir).unwrap_or(0);

        rows.push(RepoStatus {
            name: parsed.repo,
            branch,
            ahead,
            changed,
            err: None,
        });
    }

    let mut table = output::Table::new(
        Box::new(std::io::stdout()),
        vec![
            "Repository".to_string(),
            "Branch".to_string(),
            "Status".to_string(),
        ],
    );

    for rs in &rows {
        let status = if let Some(ref e) = rs.err {
            output::format_error(e)
        } else {
            output::format_repo_status(rs.ahead, rs.changed)
        };
        table.add_row(vec![rs.name.clone(), rs.branch.clone(), status])?;
    }

    table.render()
}
