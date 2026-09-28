use anyhow::{Result, bail};
use clap::Command;

use wsp_core::config::Availability;
use wsp_core::filelock;
use wsp_core::output::{MutationOutput, Output};

use crate::context::InvocationContext;

pub fn cmd() -> Command {
    Command::new("refresh-registry")
        .about("Refresh registry names captured in this workspace")
        .long_about("Refresh registry names captured in this workspace.\n\nCopies the current global registry's repository URLs into workspace metadata so `wsp repo add <name>` can resolve them when the workspace is later mounted without global wsp state. This changes only the workspace metadata; it does not add repositories, fetch, or create mirrors.")
}

pub fn run(context: &InvocationContext) -> Result<Output> {
    let ws = context.workspace_dir(None)?;
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
