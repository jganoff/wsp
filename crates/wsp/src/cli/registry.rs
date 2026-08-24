use anyhow::Result;
use clap::{ArgMatches, Command};

use wsp_core::config::Paths;
use wsp_core::output::Output;

use super::repo;

pub fn cmd() -> Command {
    Command::new("registry")
        .add(crate::shellnav::ShellNav::none())
        .about("Manage the global repo registry")
        .long_about(
            "Manage the global repo registry.\n\n\
             The registry tracks known repositories and their upstream URLs. Repos must be \
             registered before they can be added to workspaces. Registration also creates a \
             local bare mirror used to speed up cloning and fetching.",
        )
        .subcommand(repo::add_cmd())
        .subcommand(repo::list_cmd())
        .subcommand(repo::rm_cmd())
}

pub fn dispatch(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    match matches.subcommand() {
        Some(("add", m)) => repo::run_add(m, paths),
        Some(("ls", m)) => repo::run_list(m, paths),
        Some(("rm", m)) => repo::run_remove(m, paths),
        None => repo::run_list(matches, paths),
        _ => unreachable!(),
    }
}
