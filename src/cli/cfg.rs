use anyhow::{Result, bail};
use clap::{Arg, ArgMatches, Command};

use crate::config::{self, Paths};
use crate::output::{ConfigGetOutput, MutationOutput, Output};

pub fn get_cmd() -> Command {
    Command::new("get")
        .about("Get a config value")
        .arg(Arg::new("key").required(true))
}

pub fn set_cmd() -> Command {
    Command::new("set")
        .about("Set a config value")
        .arg(Arg::new("key").required(true))
        .arg(Arg::new("value").required(true))
}

pub fn unset_cmd() -> Command {
    Command::new("unset")
        .about("Unset a config value")
        .arg(Arg::new("key").required(true))
}

pub fn run_get(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let key = matches.get_one::<String>("key").unwrap();
    let cfg = config::Config::load_from(&paths.config_path)?;

    match key.as_str() {
        "branch-prefix" => Ok(Output::ConfigGet(ConfigGetOutput {
            key: key.clone(),
            value: cfg.branch_prefix,
        })),
        _ => bail!("unknown config key: {}", key),
    }
}

pub fn run_set(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let key = matches.get_one::<String>("key").unwrap();
    let value = matches.get_one::<String>("value").unwrap();
    let mut cfg = config::Config::load_from(&paths.config_path)?;

    match key.as_str() {
        "branch-prefix" => {
            cfg.branch_prefix = Some(value.clone());
            cfg.save_to(&paths.config_path)?;
            Ok(Output::Mutation(MutationOutput {
                ok: true,
                message: format!("branch-prefix = {}", value),
            }))
        }
        _ => bail!("unknown config key: {}", key),
    }
}

pub fn run_unset(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let key = matches.get_one::<String>("key").unwrap();
    let mut cfg = config::Config::load_from(&paths.config_path)?;

    match key.as_str() {
        "branch-prefix" => {
            cfg.branch_prefix = None;
            cfg.save_to(&paths.config_path)?;
            Ok(Output::Mutation(MutationOutput {
                ok: true,
                message: "branch-prefix unset".into(),
            }))
        }
        _ => bail!("unknown config key: {}", key),
    }
}
