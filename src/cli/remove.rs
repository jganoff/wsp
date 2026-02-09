use anyhow::{Result, bail};
use clap::{Arg, ArgMatches, Command};
use clap_complete::engine::ArgValueCandidates;

use crate::config::Paths;
use crate::output::{MutationOutput, Output};
use crate::workspace;

use super::completers;

pub fn cmd() -> Command {
    Command::new("remove")
        .visible_alias("rm")
        .about("Remove a workspace and its worktrees")
        .arg(Arg::new("workspace").add(ArgValueCandidates::new(completers::complete_workspaces)))
        .arg(
            Arg::new("force")
                .short('f')
                .long("force")
                .action(clap::ArgAction::SetTrue)
                .help("Remove even if repos have pending changes or unmerged branches"),
        )
}

pub fn run(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let force = matches.get_flag("force");

    let name = if let Some(n) = matches.get_one::<String>("workspace") {
        n.clone()
    } else {
        let cwd = std::env::current_dir()?;
        let ws_dir = workspace::detect(&cwd)?;
        let meta = workspace::load_metadata(&ws_dir)
            .map_err(|e| anyhow::anyhow!("reading workspace: {}", e))?;
        meta.name
    };

    if !force {
        let ws_dir = workspace::dir(&paths.workspaces_dir, &name);
        let dirty = workspace::has_pending_changes(&ws_dir)?;
        if !dirty.is_empty() {
            let mut sorted = dirty;
            sorted.sort();
            let mut list = String::new();
            for r in &sorted {
                list.push_str(&format!("\n  - {}", r));
            }
            bail!(
                "workspace {:?} has pending changes:{}\n\nUse --force to remove anyway",
                name,
                list
            );
        }
    }

    eprintln!("Removing workspace {:?}...", name);
    workspace::remove(paths, &name, force)?;

    Ok(Output::Mutation(MutationOutput {
        ok: true,
        message: format!("Workspace {:?} removed.", name),
    }))
}
