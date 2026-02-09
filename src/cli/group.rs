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
