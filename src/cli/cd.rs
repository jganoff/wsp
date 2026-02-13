use anyhow::{Result, bail};
use clap::{Arg, ArgMatches, Command};
use clap_complete::engine::ArgValueCandidates;

use crate::config::Paths;
use crate::output::{Output, PathOutput};
use crate::workspace;

use super::completers;

pub fn cmd() -> Command {
    Command::new("cd")
        .about("Change directory into a workspace")
        .arg(
            Arg::new("workspace")
                .required(true)
                .add(ArgValueCandidates::new(completers::complete_workspaces)),
        )
}

pub fn run(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let name = matches.get_one::<String>("workspace").unwrap();
    let ws_dir = workspace::dir(&paths.workspaces_dir, name);
    if !ws_dir.join(workspace::METADATA_FILE).exists() {
        bail!("workspace '{}' not found", name);
    }

    // Propagate mirror refs to clones
    if let Ok(meta) = workspace::load_metadata(&ws_dir) {
        workspace::propagate_mirror_to_clones(&ws_dir, &meta);
    }

    if std::env::var("WSP_SHELL").is_err() {
        eprintln!(
            "hint: shell integration not active, printing path only\n\
             hint: run `eval \"$(wsp setup completion zsh)\"` to enable `wsp cd`"
        );
    }
    Ok(Output::Path(PathOutput {
        path: ws_dir.display().to_string(),
    }))
}
