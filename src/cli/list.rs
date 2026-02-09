use anyhow::Result;
use clap::{ArgMatches, Command};

use crate::config::Paths;
use crate::output::{Output, WorkspaceListEntry, WorkspaceListOutput};
use crate::workspace;

pub fn cmd() -> Command {
    Command::new("list").about("List active workspaces")
}

pub fn run(_matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let names = workspace::list_all(&paths.workspaces_dir)?;

    let mut workspaces = Vec::new();
    for name in &names {
        let ws_dir = workspace::dir(&paths.workspaces_dir, name);
        let meta = match workspace::load_metadata(&ws_dir) {
            Ok(m) => m,
            Err(_) => {
                workspaces.push(WorkspaceListEntry {
                    name: name.clone(),
                    branch: "ERROR".to_string(),
                    repo_count: 0,
                    path: ws_dir.display().to_string(),
                });
                continue;
            }
        };
        workspaces.push(WorkspaceListEntry {
            name: name.clone(),
            branch: meta.branch,
            repo_count: meta.repos.len(),
            path: ws_dir.display().to_string(),
        });
    }

    Ok(Output::WorkspaceList(WorkspaceListOutput { workspaces }))
}
