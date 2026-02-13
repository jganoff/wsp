use anyhow::Result;
use clap::{ArgMatches, Command};

use crate::config::Paths;
use crate::giturl;
use crate::output::{Output, WorkspaceRepoListEntry, WorkspaceRepoListOutput};
use crate::workspace;

pub fn cmd() -> Command {
    Command::new("ls")
        .visible_alias("list")
        .about("List repos in the current workspace")
}

pub fn run(_matches: &ArgMatches, _paths: &Paths) -> Result<Output> {
    let cwd = std::env::current_dir()?;
    let ws_dir = workspace::detect(&cwd)?;

    let meta = workspace::load_metadata(&ws_dir)
        .map_err(|e| anyhow::anyhow!("reading workspace: {}", e))?;

    let identities: Vec<String> = meta.repos.keys().cloned().collect();
    let shortnames = giturl::shortnames(&identities);

    let repos = identities
        .iter()
        .map(|id| {
            let short = shortnames.get(id).cloned().unwrap_or_default();
            let dir_name = match meta.dir_name(id) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("  warning: cannot resolve dir for {}: {}", id, e);
                    String::new()
                }
            };
            let git_ref = meta.repos[id]
                .as_ref()
                .map(|r| r.r#ref.clone())
                .filter(|r| !r.is_empty());
            WorkspaceRepoListEntry {
                identity: id.clone(),
                shortname: short,
                dir_name,
                git_ref,
            }
        })
        .collect();

    Ok(Output::WorkspaceRepoList(WorkspaceRepoListOutput { repos }))
}
