use anyhow::{Result, bail};
use clap::{Arg, ArgMatches, Command};
use clap_complete::engine::ArgValueCandidates;

use crate::config::{self, Paths};
use crate::giturl;
use crate::output::{MutationOutput, Output};
use crate::workspace;

use super::completers;

pub fn cmd() -> Command {
    Command::new("rm")
        .visible_alias("remove")
        .about("Remove repo(s) from the current workspace")
        .arg(
            Arg::new("repos")
                .required(true)
                .num_args(1..)
                .add(ArgValueCandidates::new(completers::complete_repos)),
        )
        .arg(
            Arg::new("force")
                .short('f')
                .long("force")
                .action(clap::ArgAction::SetTrue)
                .help("Remove even if repos have pending changes or unmerged branches"),
        )
}

pub fn run(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let repo_args: Vec<&String> = matches.get_many::<String>("repos").unwrap().collect();
    let force = matches.get_flag("force");

    let cwd = std::env::current_dir()?;
    let ws_dir = workspace::detect(&cwd)?;

    let meta = workspace::load_metadata(&ws_dir)
        .map_err(|e| anyhow::anyhow!("reading workspace: {}", e))?;

    // Resolve repo args to full identities using workspace repos
    let ws_identities: Vec<String> = meta.repos.keys().cloned().collect();

    // Also load config to resolve against registered repos
    let cfg = config::Config::load_from(&paths.config_path)
        .map_err(|e| anyhow::anyhow!("loading config: {}", e))?;
    let cfg_identities: Vec<String> = cfg.repos.keys().cloned().collect();

    let mut resolved = Vec::new();
    for rn in &repo_args {
        // Try workspace repos first, fall back to config repos
        let id = giturl::resolve(rn, &ws_identities)
            .or_else(|_| giturl::resolve(rn, &cfg_identities))?;
        if !meta.repos.contains_key(&id) {
            bail!("repo {} is not in this workspace", id);
        }
        resolved.push(id);
    }

    eprintln!("Removing {} repo(s) from workspace...", resolved.len());
    workspace::remove_repos(&paths.mirrors_dir, &ws_dir, &resolved, force)?;

    match workspace::load_metadata(&ws_dir) {
        Ok(updated_meta) => crate::lang::run_integrations(&ws_dir, &updated_meta, &cfg),
        Err(e) => eprintln!("warning: skipping language integrations: {}", e),
    }

    Ok(Output::Mutation(MutationOutput {
        ok: true,
        message: "Done.".into(),
    }))
}
