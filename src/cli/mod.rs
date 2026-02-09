pub mod add;
pub mod completers;
pub mod completion;
pub mod exec;
pub mod group;
pub mod list;
pub mod new;
pub mod remove;
pub mod repo;
pub mod status;

use clap::{Arg, Command};

use crate::config;

pub fn build_cli() -> Command {
    let repo = Command::new("repo")
        .about("Manage registered repositories")
        .subcommand_required(true)
        .subcommand(repo::add_cmd())
        .subcommand(repo::list_cmd())
        .subcommand(repo::remove_cmd())
        .subcommand(repo::fetch_cmd());

    let group = Command::new("group")
        .about("Manage repo groups")
        .subcommand_required(true)
        .subcommand(group::new_cmd())
        .subcommand(group::list_cmd())
        .subcommand(group::show_cmd())
        .subcommand(group::delete_cmd());

    Command::new("ws")
        .about("Multi-repo workspace manager")
        .subcommand_required(true)
        .subcommand(repo)
        .subcommand(group)
        .subcommand(new::cmd())
        .subcommand(add::cmd())
        .subcommand(list::cmd())
        .subcommand(status::cmd())
        .subcommand(remove::cmd())
        .subcommand(exec::cmd())
        .subcommand(
            Command::new("completion")
                .about("Output shell integration (completions + wrapper function)")
                .hide(true)
                .arg(Arg::new("shell").required(true).value_parser(["zsh"])),
        )
}

pub fn run() -> anyhow::Result<()> {
    let paths = config::Paths::resolve()?;
    let app = build_cli();
    let matches = app.get_matches();

    match matches.subcommand() {
        Some(("repo", sub)) => match sub.subcommand() {
            Some(("add", m)) => repo::run_add(m, &paths),
            Some(("list", m)) => repo::run_list(m, &paths),
            Some(("remove", m)) => repo::run_remove(m, &paths),
            Some(("fetch", m)) => repo::run_fetch(m, &paths),
            _ => unreachable!(),
        },
        Some(("group", sub)) => match sub.subcommand() {
            Some(("new", m)) => group::run_new(m, &paths),
            Some(("list", m)) => group::run_list(m, &paths),
            Some(("show", m)) => group::run_show(m, &paths),
            Some(("delete", m)) => group::run_delete(m, &paths),
            _ => unreachable!(),
        },
        Some(("new", m)) => new::run(m, &paths),
        Some(("add", m)) => add::run(m, &paths),
        Some(("list", m)) => list::run(m, &paths),
        Some(("status", m)) => status::run(m, &paths),
        Some(("remove", m)) => remove::run(m, &paths),
        Some(("exec", m)) => exec::run(m, &paths),
        Some(("completion", m)) => completion::run(m, &paths),
        _ => unreachable!(),
    }
}
