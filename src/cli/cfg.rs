use std::collections::BTreeMap;

use anyhow::{Result, bail};
use clap::{Arg, ArgMatches, Command};

use crate::config::{self, Paths};
use crate::output::{ConfigGetOutput, ConfigListEntry, ConfigListOutput, MutationOutput, Output};

pub fn list_cmd() -> Command {
    Command::new("list").about("List all config values")
}

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

pub fn run_list(_matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let cfg = config::Config::load_from(&paths.config_path)?;
    let mut entries = Vec::new();

    // branch-prefix: show value or (not set)
    entries.push(ConfigListEntry {
        key: "branch-prefix".into(),
        value: cfg
            .branch_prefix
            .as_deref()
            .unwrap_or("(not set)")
            .to_string(),
    });

    // language integrations: show effective value for all known integrations
    for name in crate::lang::integration_names() {
        let enabled = cfg
            .language_integrations
            .as_ref()
            .and_then(|m| m.get(name.as_str()))
            .copied()
            .unwrap_or(true);
        entries.push(ConfigListEntry {
            key: format!("language-integrations.{}", name),
            value: enabled.to_string(),
        });
    }

    Ok(Output::ConfigList(ConfigListOutput { entries }))
}

pub fn run_get(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let key = matches.get_one::<String>("key").unwrap();
    let cfg = config::Config::load_from(&paths.config_path)?;

    match key.as_str() {
        "branch-prefix" => Ok(Output::ConfigGet(ConfigGetOutput {
            key: key.clone(),
            value: cfg.branch_prefix,
        })),
        k if k.starts_with("language-integrations.") => {
            let lang = &k["language-integrations.".len()..];
            let enabled = cfg
                .language_integrations
                .as_ref()
                .and_then(|m| m.get(lang))
                .copied()
                .unwrap_or(true);
            Ok(Output::ConfigGet(ConfigGetOutput {
                key: key.clone(),
                value: Some(enabled.to_string()),
            }))
        }
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
        k if k.starts_with("language-integrations.") => {
            let lang = &k["language-integrations.".len()..];
            let known = crate::lang::integration_names();
            if !known.iter().any(|n| n == lang) {
                bail!("unknown language integration: {}", lang);
            }
            let enabled: bool = value
                .parse()
                .map_err(|_| anyhow::anyhow!("value must be true or false"))?;
            let integrations = cfg.language_integrations.get_or_insert_with(BTreeMap::new);
            integrations.insert(lang.to_string(), enabled);
            cfg.save_to(&paths.config_path)?;
            Ok(Output::Mutation(MutationOutput {
                ok: true,
                message: format!("language-integrations.{} = {}", lang, enabled),
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
        k if k.starts_with("language-integrations.") => {
            let lang = &k["language-integrations.".len()..];
            let known = crate::lang::integration_names();
            if !known.iter().any(|n| n == lang) {
                bail!("unknown language integration: {}", lang);
            }
            if let Some(ref mut m) = cfg.language_integrations {
                m.remove(lang);
                if m.is_empty() {
                    cfg.language_integrations = None;
                }
            }
            cfg.save_to(&paths.config_path)?;
            Ok(Output::Mutation(MutationOutput {
                ok: true,
                message: format!("language-integrations.{} unset (default: true)", lang),
            }))
        }
        _ => bail!("unknown config key: {}", key),
    }
}
