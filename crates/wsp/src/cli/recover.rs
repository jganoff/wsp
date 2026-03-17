use anyhow::Result;
use clap::{Arg, ArgMatches, Command};

use wsp_core::config::Paths;
use wsp_core::gc;
use wsp_core::output::{MutationOutput, Output, RecoverListOutput, RecoverShowOutput};

pub fn cmd() -> Command {
    Command::new("recover")
        .about("List, inspect, or restore recently removed workspaces [read-only without args]")
        .long_about(
            "List, inspect, or restore recently removed workspaces.\n\n\
             Workspaces removed with `wsp rm` are held in a gc directory for 7 days \
             (configurable via gc.retention-days). Set gc.retention-days to 0 to keep \
             deleted workspaces indefinitely.\n\n\
             Without arguments, lists recoverable workspaces. Use `show` to inspect \
             a specific entry, or pass a name directly to restore it.",
        )
        .subcommand(
            Command::new("ls")
                .visible_alias("list")
                .about("List recoverable workspaces [read-only]"),
        )
        .subcommand(
            Command::new("show")
                .about("Inspect a recoverable workspace without restoring [read-only]")
                .arg(
                    Arg::new("workspace")
                        .required(true)
                        .help("Name of workspace to inspect"),
                ),
        )
        .arg(Arg::new("workspace").help("Name of workspace to restore"))
}

fn retention_days(paths: &Paths) -> u32 {
    wsp_core::config::Config::load_from(&paths.config_path)
        .ok()
        .and_then(|c| c.gc_retention_days)
        .unwrap_or(gc::DEFAULT_RETENTION_DAYS)
}

pub fn run(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    match matches.subcommand() {
        Some(("ls", _)) | Some(("list", _)) => {
            let entries = gc::list_enriched(&paths.gc_dir)?;
            let retention_days = retention_days(paths);
            Ok(Output::RecoverList(RecoverListOutput {
                entries,
                retention_days,
            }))
        }
        Some(("show", m)) => {
            let name = m.get_one::<String>("workspace").unwrap();
            let entry = gc::show(&paths.gc_dir, name)?;
            let retention_days = retention_days(paths);
            Ok(Output::RecoverShow(RecoverShowOutput {
                entry,
                retention_days,
            }))
        }
        _ => {
            // Bare positional arg = restore, no arg = list
            if let Some(name) = matches.get_one::<String>("workspace") {
                gc::restore(paths, name)?;
                Ok(Output::Mutation(MutationOutput::new(format!(
                    "Workspace {:?} restored.",
                    name
                ))))
            } else {
                let entries = gc::list_enriched(&paths.gc_dir)?;
                let retention_days = retention_days(paths);
                Ok(Output::RecoverList(RecoverListOutput {
                    entries,
                    retention_days,
                }))
            }
        }
    }
}
