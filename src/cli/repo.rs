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

pub fn fetch_cmd() -> Command {
    Command::new("fetch")
        .about("Fetch updates for mirror(s)")
        .arg(Arg::new("name").add(ArgValueCandidates::new(completers::complete_repos)))
        .arg(
            Arg::new("all")
                .long("all")
                .action(clap::ArgAction::SetTrue)
                .help("Fetch all registered repos"),
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
            let short = shortnames.get(id).cloned().unwrap_or_default();
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

pub fn run_fetch(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let all = matches.get_flag("all");
    let name = matches.get_one::<String>("name");

    let cfg = config::Config::load_from(&paths.config_path)
        .map_err(|e| anyhow::anyhow!("loading config: {}", e))?;

    if cfg.repos.is_empty() {
        return Ok(Output::Mutation(MutationOutput {
            ok: true,
            message: "No repos registered.".into(),
        }));
    }

    let identities: Vec<String> = cfg.repos.keys().cloned().collect();

    let to_fetch = match name {
        Some(n) if !all => {
            let identity = giturl::resolve(n, &identities)?;
            vec![identity]
        }
        _ => identities.clone(),
    };

    let mut failed = 0;
    for identity in &to_fetch {
        let entry = &cfg.repos[identity];
        let parsed = match giturl::parse(&entry.url) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("  {}: error parsing URL: {}", identity, e);
                failed += 1;
                continue;
            }
        };

        eprintln!("Fetching {}...", identity);
        if let Err(e) = mirror::fetch(&paths.mirrors_dir, &parsed) {
            eprintln!("  {}: error: {}", identity, e);
            failed += 1;
        }
    }

    if failed > 0 {
        bail!("{} fetch(es) failed", failed);
    }

    Ok(Output::Mutation(MutationOutput {
        ok: true,
        message: format!("Fetched {} repo(s)", to_fetch.len()),
    }))
}
