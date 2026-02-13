use anyhow::{Result, bail};
use chrono::Utc;
use clap::{Arg, ArgMatches, Command};
use clap_complete::engine::ArgValueCandidates;

use crate::config::{self, Paths, RepoEntry};
use crate::giturl;
use crate::mirror;
use crate::output::{MutationOutput, Output, RepoListEntry, RepoListOutput};

use super::completers;

pub fn add_cmd() -> Command {
    Command::new("add")
        .about("Register and bare-clone a repository")
        .arg(Arg::new("url").required(true))
}

pub fn list_cmd() -> Command {
    Command::new("list").about("List registered repositories")
}

pub fn remove_cmd() -> Command {
    Command::new("remove")
        .about("Remove a repository and its mirror")
        .arg(
            Arg::new("name")
                .required(true)
                .add(ArgValueCandidates::new(completers::complete_repos)),
        )
}

pub fn run_add(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let raw_url = matches.get_one::<String>("url").unwrap();

    let parsed = giturl::parse(raw_url)?;
    let mut cfg = config::Config::load_from(&paths.config_path)?;

    let identity = parsed.identity();
    if cfg.repos.contains_key(&identity) {
        bail!("repo {} already registered", identity);
    }

    let exists = mirror::exists(&paths.mirrors_dir, &parsed);
    if exists {
        bail!("mirror already exists for {}", identity);
    }

    eprintln!("Cloning {}...", raw_url);
    mirror::clone(&paths.mirrors_dir, &parsed, raw_url)
        .map_err(|e| anyhow::anyhow!("cloning: {}", e))?;

    cfg.repos.insert(
        identity.clone(),
        RepoEntry {
            url: raw_url.clone(),
            added: Utc::now(),
        },
    );

    cfg.save_to(&paths.config_path)
        .map_err(|e| anyhow::anyhow!("saving config: {}", e))?;

    Ok(Output::Mutation(MutationOutput {
        ok: true,
        message: format!("Registered {}", identity),
    }))
}

pub fn run_list(_matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let cfg = config::Config::load_from(&paths.config_path)
        .map_err(|e| anyhow::anyhow!("loading config: {}", e))?;

    let mut identities: Vec<String> = cfg.repos.keys().cloned().collect();
    identities.sort();

    let shortnames = giturl::shortnames(&identities);

    let repos = identities
        .iter()
        .map(|id| {
            let entry = &cfg.repos[id];
            let short = shortnames.get(id).cloned().unwrap_or_else(|| id.clone());
            RepoListEntry {
                identity: id.clone(),
                shortname: short,
                url: entry.url.clone(),
            }
        })
        .collect();

    Ok(Output::RepoList(RepoListOutput { repos }))
}

pub fn run_remove(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let name = matches.get_one::<String>("name").unwrap();

    let mut cfg = config::Config::load_from(&paths.config_path)
        .map_err(|e| anyhow::anyhow!("loading config: {}", e))?;

    let identities: Vec<String> = cfg.repos.keys().cloned().collect();
    let identity = giturl::resolve(name, &identities)?;

    let entry = &cfg.repos[&identity];
    let parsed = giturl::parse(&entry.url)?;

    eprintln!("Removing mirror for {}...", identity);
    mirror::remove(&paths.mirrors_dir, &parsed)
        .map_err(|e| anyhow::anyhow!("removing mirror: {}", e))?;

    cfg.repos.remove(&identity);
    cfg.save_to(&paths.config_path)
        .map_err(|e| anyhow::anyhow!("saving config: {}", e))?;

    Ok(Output::Mutation(MutationOutput {
        ok: true,
        message: format!("Removed {}", identity),
    }))
}
