use anyhow::Result;
use clap::{Arg, ArgMatches, Command};
use clap_complete::engine::ArgValueCandidates;

use wsp_core::config::Paths;
use wsp_core::output::{MutationOutput, Output};
use wsp_core::workspace;

use super::completers;

pub fn cmd() -> Command {
    Command::new("rm")
        .visible_alias("remove")
        .about("Remove a workspace")
        .long_about(
            "Remove a workspace.\n\n\
             Fetches from upstream, checks whether the workspace branch has been merged \
             (regular, squash, or rebase merge), and removes the workspace if safe. \
             Unmerged or pushed-but-unmerged branches block removal unless --force is used.\n\n\
             By default, workspaces are moved to a gc directory and can be recovered with \
             `wsp recover`. Use --permanent to skip gc and delete immediately.",
        )
        .arg(Arg::new("workspace").add(ArgValueCandidates::new(completers::complete_workspaces)))
        .arg(
            Arg::new("force")
                .short('f')
                .long("force")
                .action(clap::ArgAction::SetTrue)
                .help("Remove even if repos have pending changes, unmerged branches, or workspace root has user content"),
        )
        .arg(
            Arg::new("permanent")
                .long("permanent")
                .action(clap::ArgAction::SetTrue)
                .help("Permanently delete instead of deferring for gc"),
        )
}

pub fn run(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let force = matches.get_flag("force");
    let permanent = matches.get_flag("permanent");

    let name = if let Some(n) = matches.get_one::<String>("workspace") {
        n.clone()
    } else {
        let cwd = std::env::current_dir()?;
        let ws_dir = workspace::detect(&cwd)?;
        let meta = workspace::load_metadata(&ws_dir)
            .map_err(|e| anyhow::anyhow!("reading workspace: {}", e))?;
        meta.name
    };

    eprintln!("Removing workspace {:?}...", name);
    workspace::remove(paths, &name, force, permanent)?;

    let mut out = MutationOutput::new(format!("Workspace {:?} removed.", name));
    if !permanent {
        let cfg = wsp_core::config::Config::load_from(&paths.config_path).unwrap_or_default();
        let days = cfg
            .gc_retention_days
            .unwrap_or(wsp_core::gc::DEFAULT_RETENTION_DAYS);
        let hint = if days == 0 {
            "recoverable via `wsp recover` (gc disabled, kept indefinitely)".into()
        } else {
            format!(
                "recoverable via `wsp recover` for {} day{}",
                days,
                if days == 1 { "" } else { "s" }
            )
        };
        out = out.with_hint(hint);
    }
    Ok(Output::Mutation(out))
}
