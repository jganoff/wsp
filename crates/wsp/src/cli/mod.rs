pub mod add;
pub mod cd;
pub mod cfg;
pub mod completers;
pub mod completion;
pub mod delete;
pub mod describe;
pub mod diff;
pub mod doctor;
pub mod exec;
pub mod fetch;
pub mod help;
pub mod init;
pub mod list;
pub mod log;
pub mod new;
pub mod recover;
pub mod registry;
pub mod remove;
pub mod rename;
pub mod repo;
pub mod repo_list;
pub mod repo_setup;
pub mod repo_setup_commands;
pub mod setup;
pub mod skill;
pub mod status;
pub mod sync;
pub mod template;

use clap::{Arg, ArgMatches, Command};

use wsp_core::config::{self, Paths};
use wsp_core::output::Output;
use wsp_core::workspace;

/// Command categories for `--help` output. Each entry is (heading, [command_names]).
/// Command categories for `--help`, ordered by workflow stage.
const HELP_CATEGORIES: &[(&str, &[&str])] = &[
    (
        "Workspace",
        &[
            "new", "repo", "cd", "ls", "rename", "describe", "rm", "recover",
        ],
    ),
    ("Workflow", &["st", "diff", "log", "sync", "exec"]),
    (
        "Admin",
        &[
            "setup",
            "init",
            "registry",
            "template",
            "config",
            "doctor",
            "completion",
            "help",
        ],
    ),
];

pub fn build_cli() -> Command {
    let repo_ws = Command::new("repo")
        .about("Manage repos in the current workspace")
        .long_about(
            "Manage repos in the current workspace.\n\n\
             Add, remove, list, and fetch repos within the current workspace. Must be run \
             from inside a workspace directory.",
        )
        .subcommand(add::cmd())
        .subcommand(remove::cmd())
        .subcommand(fetch::cmd())
        .subcommand(repo_list::cmd())
        .subcommand(repo_setup::cmd())
        .subcommand(repo_setup_commands::cmd());

    #[allow(unused_mut)]
    let mut cli = Command::new("wsp")
        .disable_help_subcommand(true)
        .about("Multi-repo workspace manager")
        .long_about(
            "Multi-repo workspace manager.\n\n\
             wsp creates workspaces that span multiple git repositories, sharing a single \
             branch name across repos. Each repo is cloned from a local bare mirror, so \
             bootstrapping is fast and works offline once mirrors are populated.\n\n\
             Workspaces live in ~/dev/workspaces/<name>/ with a .wsp.yaml metadata file. \
             Inside a workspace, each repo is a normal git clone — no wsp-specific remotes \
             or config leak into .git/.",
        )
        .version(env!("WSP_VERSION_STRING"))
        .arg(
            Arg::new("json")
                .long("json")
                .global(true)
                .action(clap::ArgAction::SetTrue)
                .help("Output as JSON"),
        )
        // Workspace commands
        .subcommand(new::cmd())
        .subcommand(delete::cmd())
        .subcommand(list::cmd())
        .subcommand(status::cmd())
        .subcommand(diff::cmd())
        .subcommand(log::cmd())
        .subcommand(sync::cmd())
        .subcommand(exec::cmd())
        .subcommand(cd::cmd())
        .subcommand(recover::cmd())
        .subcommand(rename::cmd())
        .subcommand(describe::cmd())
        // Workspace-scoped repo commands
        .subcommand(repo_ws)
        // Admin commands
        .subcommand(setup::cmd())
        .subcommand(init::cmd())
        .subcommand(registry::cmd())
        .subcommand(template::cmd())
        .subcommand(cfg::cmd())
        .subcommand(doctor::cmd())
        .subcommand(completion::cmd())
        // Help with topic support
        .subcommand(help::cmd());

    #[cfg(feature = "codegen")]
    {
        cli = cli.subcommand(skill::generate_cmd().hide(true));
    }

    // Build categorized help from the command definitions, then set
    // a custom help_template that replaces clap's flat subcommand list.
    let categorized = build_categorized_help(&cli);
    cli.help_template("{about-with-newline}\n{usage-heading} {usage}\n\n{options}\n{after-help}")
        .after_help(categorized)
}

/// Build categorized help text by introspecting subcommand about strings.
fn build_categorized_help(cli: &Command) -> String {
    use std::fmt::Write;

    let mut out = String::new();

    for (heading, names) in HELP_CATEGORIES {
        writeln!(out, "{}:", heading).unwrap();
        for name in *names {
            if let Some(sub) = cli.find_subcommand(name) {
                let about = sub.get_about().map(|a| a.to_string()).unwrap_or_default();
                let aliases: Vec<&str> = sub.get_visible_aliases().collect();
                let alias_suffix = if aliases.is_empty() {
                    String::new()
                } else {
                    format!(" [aliases: {}]", aliases.join(", "))
                };
                writeln!(out, "  {:12}{}{}", name, about, alias_suffix).unwrap();
            }
        }
        out.push('\n');
    }

    // Trim trailing newline
    while out.ends_with('\n') {
        out.pop();
    }

    out
}

pub fn dispatch(matches: &ArgMatches, paths: &Paths) -> anyhow::Result<Output> {
    match matches.subcommand() {
        // --- Workspace-scoped repo commands ---
        Some(("repo", sub)) => match sub.subcommand() {
            Some(("add", m)) => add::run(m, paths),
            Some(("rm", m)) => remove::run(m, paths),
            Some(("fetch", m)) => fetch::run(m, paths),
            Some(("ls", m)) => repo_list::run(m, paths),
            Some(("setup", m)) => repo_setup::run(m, paths),
            Some(("setup-commands", m)) => repo_setup_commands::run(m, paths),
            None => repo_list::run(sub, paths),
            _ => unreachable!(),
        },

        // --- Workspace commands ---
        Some(("new", m)) => new::run(m, paths),
        Some(("rm", m)) => delete::run(m, paths),
        Some(("cd", m)) => cd::run(m, paths),
        Some(("ls", m)) => list::run(m, paths),
        Some(("st", m)) => status::run(m, paths),
        Some(("diff", m)) => diff::run(m, paths),
        Some(("log", m)) => log::run(m, paths),
        Some(("sync", m)) => sync::run(m, paths),
        Some(("exec", m)) => exec::run(m, paths),
        Some(("recover", m)) => recover::run(m, paths),
        Some(("rename", m)) => rename::run(m, paths),
        Some(("describe", m)) => describe::run(m, paths),

        // --- Admin commands (promoted from setup) ---
        Some(("registry", sub)) => registry::dispatch(sub, paths),
        Some(("template", sub)) => template::dispatch(sub, paths),
        Some(("config", sub)) => cfg::dispatch(sub, paths),
        Some(("doctor", m)) => doctor::run(m, paths),
        Some(("completion", m)) => completion::run(m, paths),
        Some(("setup", m)) => setup::run(m, paths),
        Some(("init", m)) => init::run(m, paths),

        // --- Dev-only codegen ---
        #[cfg(feature = "codegen")]
        Some(("generate", m)) => skill::run_generate(m, paths),
        // --- No subcommand: default behavior ---
        None => {
            let cwd = std::env::current_dir()?;
            if workspace::detect(&cwd).is_ok() {
                status::run(matches, paths)
            } else {
                let mut output = list::run(matches, paths)?;
                if let Output::WorkspaceList(ref mut wl) = output {
                    // Cheap first-run check: no config file means wsp has never been configured.
                    // If the file exists, load it to check if anything is actually set.
                    let is_first_run = if !paths.config_path.exists() {
                        true
                    } else {
                        let cfg = config::Config::load_from(&paths.config_path)?;
                        cfg.branch_prefix.is_none() && cfg.repos.is_empty()
                    };
                    wl.hint = Some(if is_first_run {
                        "New to wsp? Run `wsp setup` to get started.".to_string()
                    } else {
                        "Not in a workspace. Use `wsp cd <name>` to enter one.".to_string()
                    });
                }
                Ok(output)
            }
        }
        _ => unreachable!(),
    }
}
