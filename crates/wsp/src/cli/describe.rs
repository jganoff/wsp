use anyhow::{Result, bail};
use clap::{Arg, ArgMatches, Command};
use clap_complete::engine::ArgValueCandidates;

use wsp_core::config::Paths;
use wsp_core::filelock;
use wsp_core::output::{MutationOutput, Output};
use wsp_core::workspace;

use super::completers;

pub fn cmd() -> Command {
    Command::new("describe")
        .about("Set or update a workspace description")
        .long_about(
            "Set or update a workspace description.\n\n\
             Stores a short purpose string in the workspace metadata. The description \
             appears in `wsp ls` output to help identify workspaces at a glance.",
        )
        .arg(
            Arg::new("workspace")
                .required(true)
                .add(ArgValueCandidates::new(completers::complete_workspaces)),
        )
        .arg(Arg::new("text").required(true))
}

pub fn run(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let ws_name = matches.get_one::<String>("workspace").unwrap();
    let text = matches.get_one::<String>("text").unwrap();

    workspace::validate_name(ws_name)?;
    let ws_dir = workspace::dir(&paths.workspaces_dir, ws_name);
    if !ws_dir.join(workspace::METADATA_FILE).exists() {
        bail!("workspace '{}' not found", ws_name);
    }

    let desc = if text.is_empty() {
        None
    } else {
        Some(text.clone())
    };

    filelock::with_metadata(&ws_dir, |meta| {
        meta.description = desc;
        Ok(())
    })?;

    if text.is_empty() {
        Ok(Output::Mutation(MutationOutput::new(format!(
            "Description cleared for {}",
            ws_name
        ))))
    } else {
        Ok(Output::Mutation(MutationOutput::new(format!(
            "Description set for {}",
            ws_name
        ))))
    }
}
