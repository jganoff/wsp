use std::collections::BTreeMap;

use anyhow::{Result, bail};
use clap::{Arg, ArgMatches, Command};
use clap_complete::engine::ArgValueCandidates;

use crate::config::{self, Paths};
use crate::giturl;
use crate::group;
use crate::output::{MutationOutput, Output};
use crate::workspace;

use super::completers;

pub fn cmd() -> Command {
    Command::new("add")
        .about("Add repos to current workspace")
        .arg(
            Arg::new("repos")
                .num_args(0..)
                .add(ArgValueCandidates::new(completers::complete_repos)),
        )
        .arg(
            Arg::new("group")
                .short('g')
                .long("group")
                .help("Add repos from a group")
                .add(ArgValueCandidates::new(completers::complete_groups)),
        )
}

pub fn run(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let repo_args: Vec<&String> = matches
        .get_many::<String>("repos")
        .map(|v| v.collect())
        .unwrap_or_default();
    let group_name = matches.get_one::<String>("group");

    let cwd = std::env::current_dir()?;
    let ws_dir = workspace::detect(&cwd)?;

    let cfg = config::Config::load_from(&paths.config_path)
        .map_err(|e| anyhow::anyhow!("loading config: {}", e))?;

    let identities: Vec<String> = cfg.repos.keys().cloned().collect();

    let mut repo_refs: BTreeMap<String, String> = BTreeMap::new();

    if let Some(gn) = group_name {
        let group_repos = group::get(&cfg, gn)?;
        for id in group_repos {
            repo_refs.insert(id, String::new());
        }
    }

    for rn in &repo_args {
        let (name, r) = giturl::parse_repo_ref(rn);
        let id = giturl::resolve(name, &identities)?;
        repo_refs.insert(id, r.to_string());
    }

    if repo_refs.is_empty() {
        bail!("no repos specified (use repo args or --group)");
    }

    eprintln!("Adding {} repos to workspace...", repo_refs.len());
    workspace::add_repos(&paths.mirrors_dir, &ws_dir, &repo_refs)?;

    Ok(Output::Mutation(MutationOutput {
        ok: true,
        message: "Done.".into(),
    }))
}
