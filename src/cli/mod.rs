pub mod add;
pub mod cd;
pub mod cfg;
pub mod completers;
pub mod completion;
pub mod delete;
pub mod diff;
pub mod exec;
pub mod fetch;
pub mod group;
pub mod list;
pub mod new;
pub mod remove;
pub mod repo;
pub mod repo_list;
pub mod skill;
pub mod status;

use clap::{Arg, ArgMatches, Command};

use crate::config::Paths;
use crate::output::Output;
use crate::workspace;

pub fn build_cli() -> Command {
    let repo_admin = Command::new("repo")
        .about("Manage registered repositories")
        .subcommand_required(true)
        .subcommand(repo::add_cmd())
        .subcommand(repo::list_cmd())
        .subcommand(repo::remove_cmd());

    let group = Command::new("group")
        .about("Manage repo groups")
        .subcommand_required(true)
        .subcommand(group::new_cmd())
        .subcommand(group::list_cmd())
        .subcommand(group::show_cmd())
        .subcommand(group::delete_cmd())
        .subcommand(group::update_cmd());

    let config = Command::new("config")
        .about("Manage global configuration")
        .subcommand_required(true)
        .subcommand(cfg::list_cmd())
        .subcommand(cfg::get_cmd())
        .subcommand(cfg::set_cmd())
        .subcommand(cfg::unset_cmd());

    let skill_cmd = Command::new("skill")
        .about("Manage Claude Code skills")
        .subcommand_required(true)
        .subcommand(skill::install_cmd());

    let setup = Command::new("setup")
        .about("Configure repos, groups, and settings")
        .subcommand_required(true)
        .subcommand(repo_admin)
        .subcommand(group)
        .subcommand(config)
        .subcommand(skill_cmd)
        .subcommand(
            Command::new("completion")
                .about("Output shell integration (completions + wrapper function)")
                .arg(
                    Arg::new("shell")
                        .required(true)
                        .value_parser(["zsh", "bash", "fish"]),
                ),
        );

    let repo_ws = Command::new("repo")
        .about("Manage repos in the current workspace")
        .subcommand_required(true)
        .subcommand(add::cmd())
        .subcommand(remove::cmd())
        .subcommand(fetch::cmd())
        .subcommand(repo_list::cmd());

    Command::new("wsp")
        .about("Multi-repo workspace manager")
        .version(env!("WSP_VERSION_STRING"))
        .arg(
            Arg::new("json")
                .long("json")
                .global(true)
                .action(clap::ArgAction::SetTrue)
                .help("Output as JSON"),
        )
        .subcommand(new::cmd())
        .subcommand(delete::cmd())
        .subcommand(repo_ws)
        .subcommand(list::cmd())
        .subcommand(status::cmd())
        .subcommand(diff::cmd())
        .subcommand(exec::cmd())
        .subcommand(cd::cmd())
        .subcommand(setup)
}

pub fn dispatch(matches: &ArgMatches, paths: &Paths) -> anyhow::Result<Output> {
    match matches.subcommand() {
        Some(("setup", sub)) => match sub.subcommand() {
            Some(("repo", sub2)) => match sub2.subcommand() {
                Some(("add", m)) => repo::run_add(m, paths),
                Some(("list", m)) => repo::run_list(m, paths),
                Some(("remove", m)) => repo::run_remove(m, paths),
                _ => unreachable!(),
            },
            Some(("group", sub2)) => match sub2.subcommand() {
                Some(("new", m)) => group::run_new(m, paths),
                Some(("list", m)) => group::run_list(m, paths),
                Some(("show", m)) => group::run_show(m, paths),
                Some(("delete", m)) => group::run_delete(m, paths),
                Some(("update", m)) => group::run_update(m, paths),
                _ => unreachable!(),
            },
            Some(("config", sub2)) => match sub2.subcommand() {
                Some(("list", m)) => cfg::run_list(m, paths),
                Some(("get", m)) => cfg::run_get(m, paths),
                Some(("set", m)) => cfg::run_set(m, paths),
                Some(("unset", m)) => cfg::run_unset(m, paths),
                _ => unreachable!(),
            },
            Some(("skill", sub2)) => match sub2.subcommand() {
                Some(("install", m)) => skill::run_install(m, paths),
                _ => unreachable!(),
            },
            Some(("completion", m)) => completion::run(m, paths),
            _ => unreachable!(),
        },
        Some(("repo", sub)) => match sub.subcommand() {
            Some(("add", m)) => add::run(m, paths),
            Some(("rm", m)) => remove::run(m, paths),
            Some(("fetch", m)) => fetch::run(m, paths),
            Some(("ls", m)) => repo_list::run(m, paths),
            _ => unreachable!(),
        },
        Some(("new", m)) => new::run(m, paths),
        Some(("rm", m)) => delete::run(m, paths),
        Some(("cd", m)) => cd::run(m, paths),
        Some(("ls", m)) => list::run(m, paths),
        Some(("st", m)) => status::run(m, paths),
        Some(("diff", m)) => diff::run(m, paths),
        Some(("exec", m)) => exec::run(m, paths),
        None => {
            let cwd = std::env::current_dir()?;
            if workspace::detect(&cwd).is_ok() {
                status::run(matches, paths)
            } else {
                let mut output = list::run(matches, paths)?;
                if let Output::WorkspaceList(ref mut wl) = output {
                    wl.hint =
                        Some("Not in a workspace. Use `wsp cd <name>` to enter one.".to_string());
                }
                Ok(output)
            }
        }
        _ => unreachable!(),
    }
}
