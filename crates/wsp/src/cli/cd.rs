use anyhow::{Result, bail};
use clap::{Arg, ArgMatches, Command};
use clap_complete::engine::ArgValueCandidates;

use wsp_core::config::Paths;
use wsp_core::output::{Output, PathOutput};
use wsp_core::workspace;

use super::completers;

pub fn cmd() -> Command {
    Command::new("cd")
        // Destination arrives on stdout, gated on WSP_SHELL.
        .add(crate::shellnav::ShellNav::prints_path())
        .about("Change directory into a workspace")
        .long_about(
            "Change directory into a workspace.\n\n\
             Requires shell integration to be active (see `wsp completion`). Without it, \
             prints the workspace path instead.",
        )
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

    if std::env::var("WSP_SHELL").is_err() {
        eprintln!(
            "hint: shell integration not active, printing path only\n\
             hint: run `eval \"$(wsp completion zsh)\"` to enable `wsp cd`"
        );
    }
    Ok(Output::Path(PathOutput {
        path: ws_dir.display().to_string(),
    }))
}
