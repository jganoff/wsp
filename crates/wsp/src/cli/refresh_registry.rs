use anyhow::{Result, bail};
use clap::{Arg, ArgMatches, Command};
use clap_complete::engine::ArgValueCandidates;

use wsp_core::config::Availability;
use wsp_core::filelock;
use wsp_core::output::{MutationOutput, Output};

use super::completers;
use crate::context::InvocationContext;

pub fn cmd() -> Command {
    Command::new("refresh-registry")
        .about("Refresh registry names captured for a workspace")
        .long_about("Refresh registry names captured for a workspace.\n\nCopies the current global registry's repository URLs into workspace metadata so `wsp repo add <name>` can resolve them when the workspace is later mounted without global wsp state. Omit the workspace name when running inside it. This changes only the workspace metadata; it does not add repositories, fetch, or create mirrors.")
        .arg(Arg::new("workspace").required(false).add(ArgValueCandidates::new(completers::complete_workspaces)))
}

pub fn run(matches: &ArgMatches, context: &InvocationContext) -> Result<Output> {
    let ws = context.workspace_dir(matches.get_one::<String>("workspace").map(String::as_str))?;
    if !matches!(
        context.global_state,
        Availability::Available | Availability::ReadOnly
    ) {
        bail!(
            "refresh-registry requires a readable global registry; run it on the host before mounting the workspace in a sandbox"
        );
    }
    let urls = context
        .config
        .repos
        .iter()
        .map(|(identity, entry)| (identity.clone(), entry.url.clone()))
        .collect();
    let meta = filelock::with_metadata(&ws, |meta| {
        meta.registry_urls = urls;
        Ok(())
    })?;
    let mut out = MutationOutput::new(format!(
        "Captured {} registry repos in workspace.",
        meta.registry_urls.len()
    ));
    out.workspace = Some(meta.name);
    Ok(Output::Mutation(out))
}
