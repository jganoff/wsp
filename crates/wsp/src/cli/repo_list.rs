use anyhow::Result;
use clap::{ArgMatches, Command};

use crate::output::print_gc_warning;
use wsp_core::config::Paths;
use wsp_core::gc;
use wsp_core::giturl;
use wsp_core::output::{Output, WorkspaceRepoListEntry, WorkspaceRepoListOutput};
use wsp_core::workspace;

pub fn cmd() -> Command {
    Command::new("ls")
        .visible_alias("list")
        .about("List repos in the current workspace [read-only]")
        .long_about(
            "List repos in the current workspace [read-only].\n\n\
             Shows each repo's identity, directory name, and role within the workspace.",
        )
}

pub fn run(_matches: &ArgMatches, _paths: &Paths) -> Result<Output> {
    let cwd = crate::shellcd::invocation_dir()?;
    let ws_dir = workspace::detect(&cwd)?;

    if let Some(warning) = gc::check_workspace(&ws_dir, /* read_only */ true)? {
        print_gc_warning(&warning);
    }

    let meta = workspace::load_metadata(&ws_dir)
        .map_err(|e| anyhow::anyhow!("reading workspace: {}", e))?;

    let identities: Vec<String> = meta.repos.keys().cloned().collect();
    let shortnames = giturl::shortnames(&identities);

    let repos = identities
        .iter()
        .map(|id| {
            let short = shortnames.get(id).cloned().unwrap_or_else(|| id.clone());
            let dir_name = match meta.dir_name(id) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("  warning: cannot resolve dir for {}: {}", id, e);
                    String::new()
                }
            };
            WorkspaceRepoListEntry {
                identity: id.clone(),
                shortname: short,
                dir_name,
            }
        })
        .collect();

    Ok(Output::WorkspaceRepoList(WorkspaceRepoListOutput {
        workspace: meta.name,
        branch: meta.branch,
        workspace_dir: ws_dir,
        repos,
    }))
}
