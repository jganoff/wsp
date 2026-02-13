use anyhow::Result;
use clap::{Arg, ArgMatches, Command};
use clap_complete::engine::ArgValueCandidates;

use crate::config::{self, Paths};
use crate::giturl;
use crate::group as grp;
use crate::output::{GroupListEntry, GroupListOutput, GroupShowOutput, MutationOutput, Output};

use super::completers;

pub fn new_cmd() -> Command {
    Command::new("new")
        .about("Create a new repo group")
        .arg(Arg::new("name").required(true))
        .arg(
            Arg::new("repos")
                .required(true)
                .num_args(1..)
                .add(ArgValueCandidates::new(completers::complete_repos)),
        )
}

pub fn list_cmd() -> Command {
    Command::new("list").about("List all groups")
}

pub fn show_cmd() -> Command {
    Command::new("show").about("Show repos in a group").arg(
        Arg::new("name")
            .required(true)
            .add(ArgValueCandidates::new(completers::complete_groups)),
    )
}

pub fn delete_cmd() -> Command {
    Command::new("delete").about("Delete a group").arg(
        Arg::new("name")
            .required(true)
            .add(ArgValueCandidates::new(completers::complete_groups)),
    )
}

pub fn update_cmd() -> Command {
    Command::new("update")
        .about("Add or remove repos from a group")
        .arg(
            Arg::new("name")
                .required(true)
                .add(ArgValueCandidates::new(completers::complete_groups)),
        )
        .arg(
            Arg::new("add")
                .long("add")
                .num_args(1..)
                .add(ArgValueCandidates::new(
                    completers::complete_group_repos_add,
                )),
        )
        .arg(
            Arg::new("remove")
                .long("remove")
                .num_args(1..)
                .add(ArgValueCandidates::new(
                    completers::complete_group_repos_remove,
                )),
        )
}

pub fn run_new(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let name = matches.get_one::<String>("name").unwrap();
    let repo_names: Vec<&String> = matches.get_many::<String>("repos").unwrap().collect();

    let mut cfg = config::Config::load_from(&paths.config_path)
        .map_err(|e| anyhow::anyhow!("loading config: {}", e))?;

    let identities: Vec<String> = cfg.repos.keys().cloned().collect();

    let mut resolved = Vec::new();
    for rn in &repo_names {
        let id = giturl::resolve(rn, &identities)?;
        resolved.push(id);
    }

    grp::create(&mut cfg, name, resolved.clone())?;

    cfg.save_to(&paths.config_path)
        .map_err(|e| anyhow::anyhow!("saving config: {}", e))?;

    Ok(Output::Mutation(MutationOutput {
        ok: true,
        message: format!("Created group {:?} with {} repos", name, resolved.len()),
    }))
}

pub fn run_list(_matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let cfg = config::Config::load_from(&paths.config_path)
        .map_err(|e| anyhow::anyhow!("loading config: {}", e))?;

    let names = grp::list(&cfg);
    let mut sorted_names = names;
    sorted_names.sort();

    let mut groups = Vec::new();
    for name in &sorted_names {
        let repos = grp::get(&cfg, name)?;
        groups.push(GroupListEntry {
            name: name.clone(),
            repo_count: repos.len(),
        });
    }

    Ok(Output::GroupList(GroupListOutput { groups }))
}

pub fn run_show(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let name = matches.get_one::<String>("name").unwrap();

    let cfg = config::Config::load_from(&paths.config_path)
        .map_err(|e| anyhow::anyhow!("loading config: {}", e))?;

    let repos = grp::get(&cfg, name)?;

    Ok(Output::GroupShow(GroupShowOutput {
        name: name.clone(),
        repos,
    }))
}

pub fn run_delete(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let name = matches.get_one::<String>("name").unwrap();

    let mut cfg = config::Config::load_from(&paths.config_path)
        .map_err(|e| anyhow::anyhow!("loading config: {}", e))?;

    grp::delete(&mut cfg, name)?;

    cfg.save_to(&paths.config_path)
        .map_err(|e| anyhow::anyhow!("saving config: {}", e))?;

    Ok(Output::Mutation(MutationOutput {
        ok: true,
        message: format!("Deleted group {:?}", name),
    }))
}

pub fn run_update(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let name = matches.get_one::<String>("name").unwrap();
    let to_add: Vec<&String> = matches
        .get_many::<String>("add")
        .map(|v| v.collect())
        .unwrap_or_default();
    let to_remove: Vec<&String> = matches
        .get_many::<String>("remove")
        .map(|v| v.collect())
        .unwrap_or_default();

    if to_add.is_empty() && to_remove.is_empty() {
        anyhow::bail!("at least one of --add or --remove is required");
    }

    let mut cfg = config::Config::load_from(&paths.config_path)
        .map_err(|e| anyhow::anyhow!("loading config: {}", e))?;

    let identities: Vec<String> = cfg.repos.keys().cloned().collect();

    let resolved_add: Vec<String> = to_add
        .iter()
        .map(|rn| giturl::resolve(rn, &identities))
        .collect::<Result<_>>()?;
    let resolved_remove: Vec<String> = to_remove
        .iter()
        .map(|rn| giturl::resolve(rn, &identities))
        .collect::<Result<_>>()?;

    let add_set: std::collections::HashSet<&str> =
        resolved_add.iter().map(|s| s.as_str()).collect();
    let overlap: Vec<&str> = resolved_remove
        .iter()
        .filter(|r| add_set.contains(r.as_str()))
        .map(|r| r.as_str())
        .collect();
    if !overlap.is_empty() {
        anyhow::bail!("repos appear in both --add and --remove: {:?}", overlap);
    }

    if !resolved_add.is_empty() {
        grp::add_repos(&mut cfg, name, resolved_add)?;
    }

    if !resolved_remove.is_empty() {
        grp::remove_repos(&mut cfg, name, resolved_remove)?;
    }

    cfg.save_to(&paths.config_path)
        .map_err(|e| anyhow::anyhow!("saving config: {}", e))?;

    let mut parts = Vec::new();
    if !to_add.is_empty() {
        parts.push(format!("added {}", to_add.len()));
    }
    if !to_remove.is_empty() {
        parts.push(format!("removed {}", to_remove.len()));
    }

    Ok(Output::Mutation(MutationOutput {
        ok: true,
        message: format!("Updated group {:?}: {}", name, parts.join(", ")),
    }))
}
