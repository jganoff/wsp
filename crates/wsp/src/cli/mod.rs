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
pub mod whatsnew;

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
            "whatsnew",
            "completion",
            "help",
        ],
    ),
];

pub fn build_cli() -> Command {
    let repo_ws = Command::new("repo")
        // `repo rm` deletes a repo directory, so the wrapper steps out before
        // running the binary and returns afterwards. Which subcommand ran does
        // not matter to the wrapper: it goes back to where it was if that still
        // exists, and otherwise to whatever destination the binary reported.
        .add(crate::shellnav::ShellNav::vacates_and_follows())
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
        .subcommand(whatsnew::cmd())
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
        Some(("whatsnew", m)) => whatsnew::run(m, paths),

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
                    // Do not clobber a footer `list::run` already set. It
                    // reports workspaces that are recoverable but expiring,
                    // which outranks navigation advice: the advice is always
                    // re-derivable, the deadline is not. Bare `wsp` is the
                    // likely next keystroke after `wsp rm`, since the wrapper
                    // has just moved you out of the workspace.
                    if wl.hint.is_none() {
                        wl.hint = Some(if is_first_run {
                            "New to wsp? Run `wsp setup` to get started.".to_string()
                        } else if wl.workspaces.is_empty() {
                            // Nothing to cd into, so don't suggest it.
                            "No workspaces yet. Run `wsp new <name>` to make one.".to_string()
                        } else {
                            "Not in a workspace. Use `wsp cd <name>` to enter one.".to_string()
                        });
                    }
                }
                Ok(output)
            }
        }
        _ => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Commands where a required workspace positional is intentional.
    ///
    /// cd: navigates TO a named workspace; CWD detection is not meaningful.
    const WORKSPACE_REQUIRED_ALLOWLIST: &[&str] = &["cd"];

    /// Every command with a `workspace` positional arg must make it optional
    /// (CWD detection fallback) unless explicitly allowlisted.
    ///
    /// See design-tenets.md "Workspace from context" tenet.
    ///
    /// For multi-positional commands (e.g. `describe [workspace] <text>`),
    /// the first positional may be clap-required since at least one arg is
    /// needed, but the runtime treats the single-arg case as
    /// "workspace from CWD + arg is payload." Those are verified separately
    /// in `test_multi_positional_workspace_parses_without_name`.
    #[test]
    fn test_workspace_arg_is_optional() {
        let cli = build_cli();
        check_subcommands(cli.get_subcommands(), &[]);
    }

    fn check_subcommands<'a>(commands: impl Iterator<Item = &'a Command>, parent_path: &[&str]) {
        for sub in commands {
            let name = sub.get_name();
            let mut path = parent_path.to_vec();
            path.push(name);
            let full_name = path.join(" ");

            for arg in sub.get_arguments() {
                if arg.get_id() != "workspace" || !arg.is_positional() {
                    continue;
                }

                // Multi-positional commands handle the optional workspace via
                // runtime dispatch, so clap may still mark the first positional
                // as required. Skip the introspection check for those and rely
                // on the parse-based test below.
                let has_other_positionals = sub
                    .get_arguments()
                    .any(|a| a.is_positional() && a.get_id() != "workspace");

                if arg.is_required_set() && !has_other_positionals {
                    assert!(
                        WORKSPACE_REQUIRED_ALLOWLIST.contains(&name),
                        "command '{}' has a required workspace positional with no \
                         other positionals; workspace should be optional with CWD \
                         fallback (see design-tenets.md 'Workspace from context'). \
                         If requiring it is intentional, add '{}' to \
                         WORKSPACE_REQUIRED_ALLOWLIST in cli/mod.rs with a comment.",
                        full_name,
                        name,
                    );
                }
            }

            check_subcommands(sub.get_subcommands(), &path);
        }
    }

    /// Multi-positional commands must parse successfully with just the
    /// non-workspace arg(s), proving workspace is truly optional at the
    /// parsing level.
    #[test]
    fn test_multi_positional_workspace_parses_without_name() {
        let cases: &[(&str, &[&str])] = &[
            ("describe", &["some description"]),
            ("rename", &["new-name"]),
            ("exec", &["--", "echo", "hello"]),
        ];

        for (name, args) in cases {
            let mut argv = vec!["wsp", name];
            argv.extend_from_slice(args);
            let result = build_cli().try_get_matches_from(argv);
            assert!(
                result.is_ok(),
                "command '{}' should parse with workspace omitted (error: {})",
                name,
                result.unwrap_err(),
            );
        }
    }

    #[test]
    fn test_new_command_accepts_create_alias() {
        let matches = build_cli()
            .try_get_matches_from(["wsp", "create", "my-workspace"])
            .expect("create alias should parse");

        assert_eq!(matches.subcommand_name(), Some("new"));
    }
}
