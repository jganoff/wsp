use std::collections::BTreeMap;

use anyhow::{Result, bail};
use clap::{Arg, ArgMatches, Command};

use crate::config::{self, Paths};
use crate::giturl;
use crate::group;
use crate::workspace;

pub fn cmd() -> Command {
    Command::new("new")
        .about("Create a new workspace with worktrees")
        .arg(Arg::new("workspace").required(true))
        .arg(Arg::new("repos").num_args(0..))
        .arg(
            Arg::new("group")
                .short('g')
                .long("group")
                .help("Add repos from a group"),
        )
}

pub fn run(matches: &ArgMatches, paths: &Paths) -> Result<()> {
    let ws_name = matches.get_one::<String>("workspace").unwrap();
    let repo_args: Vec<&String> = matches
        .get_many::<String>("repos")
        .map(|v| v.collect())
        .unwrap_or_default();
    let group_name = matches.get_one::<String>("group");

    let cfg = config::Config::load_from(&paths.config_path)
        .map_err(|e| anyhow::anyhow!("loading config: {}", e))?;

    let identities: Vec<String> = cfg.repos.keys().cloned().collect();

    let mut repo_refs: BTreeMap<String, String> = BTreeMap::new();

    // Add repos from group (active, no ref)
    if let Some(gn) = group_name {
        let group_repos = group::get(&cfg, gn)?;
        for id in group_repos {
            repo_refs.insert(id, String::new());
        }
    }

    // Add individual repos (may have @ref)
    for rn in &repo_args {
        let (name, r) = giturl::parse_repo_ref(rn);
        let id = giturl::resolve(name, &identities)?;
        repo_refs.insert(id, r.to_string());
    }

    if repo_refs.is_empty() {
        bail!("no repos specified (use repo args or --group)");
    }

    println!(
        "Creating workspace {:?} with {} repos...",
        ws_name,
        repo_refs.len()
    );
    workspace::create(paths, ws_name, &repo_refs)?;

    let ws_dir = workspace::dir(&paths.workspaces_dir, ws_name);
    println!("Workspace created: {}", ws_dir.display());
    Ok(())
}
