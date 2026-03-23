use anyhow::Result;
use clap::{Arg, ArgMatches, Command};
use clap_complete::engine::ArgValueCandidates;

use wsp_core::config::Paths;
use wsp_core::giturl;
use wsp_core::output::{MutationOutput, Output};
use wsp_core::template;
use wsp_core::workspace;

use super::completers;

pub fn cmd() -> Command {
    Command::new("setup")
        .about("Run setup commands for repos in the current workspace")
        .long_about(
            "Run setup commands for repos in the current workspace.\n\n\
             Repos can declare setup_commands in their .wsp.yaml to run after cloning \
             (e.g. installing git hooks, generating files). This command re-runs those \
             commands, prompting for approval if not already approved.\n\n\
             Filters to the given repos if specified; otherwise runs for all repos that \
             have setup_commands. Use --force to re-prompt even for already-approved repos.",
        )
        .arg(
            Arg::new("repos")
                .num_args(0..)
                .add(ArgValueCandidates::new(completers::complete_repos)),
        )
        .arg(
            Arg::new("force")
                .long("force")
                .action(clap::ArgAction::SetTrue)
                .help("Re-prompt even if commands are already approved"),
        )
}

pub fn run(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let filter: Vec<&String> = matches
        .get_many::<String>("repos")
        .map(|v| v.collect())
        .unwrap_or_default();
    let force = matches.get_flag("force");

    let cwd = std::env::current_dir()?;
    let ws_dir = workspace::detect(&cwd)?;
    let meta = workspace::load_metadata(&ws_dir)?;

    // Pre-resolve filter args to full identities so shortnames work.
    let cfg = wsp_core::config::Config::load_from(&paths.config_path)
        .map_err(|e| anyhow::anyhow!("loading config: {}", e))?;
    let identities: Vec<String> = cfg.repos.keys().cloned().collect();
    let resolved_filter: Vec<String> = filter
        .iter()
        .filter_map(|f| giturl::resolve(giturl::parse_repo_ref(f), &identities).ok())
        .collect();

    let mut ran = 0usize;
    let mut skipped = 0usize;

    for info in meta.repo_infos(&ws_dir) {
        if info.error.is_some() {
            continue;
        }
        if !filter.is_empty() && !resolved_filter.contains(&info.identity) {
            continue;
        }

        let Some(cmds) = template::read_setup_commands(&info.clone_dir.join(".wsp.yaml")) else {
            continue;
        };

        let result = if force {
            wsp_core::setup_runner::prompt_and_run_setup(
                paths.data_dir(),
                &info.clone_dir,
                &info.identity,
                &cmds,
            )
        } else {
            wsp_core::setup_runner::maybe_run_setup(
                paths.data_dir(),
                &info.clone_dir,
                &info.identity,
                &cmds,
            )
        };

        match result {
            Ok(true) => ran += 1,
            Ok(false) => skipped += 1,
            Err(e) => eprintln!("warning: setup for {}: {}", info.identity, e),
        }
    }

    Ok(Output::Mutation(MutationOutput::new(format!(
        "Setup complete: {} ran, {} skipped.",
        ran, skipped
    ))))
}
