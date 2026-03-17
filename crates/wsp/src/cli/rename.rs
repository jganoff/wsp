use anyhow::Result;
use clap::{Arg, ArgMatches, Command};
use clap_complete::engine::ArgValueCandidates;

use wsp_core::config::Paths;
use wsp_core::output::{MutationOutput, Output};
use wsp_core::workspace;

use super::completers;

pub fn cmd() -> Command {
    Command::new("rename")
        .about("Rename a workspace, its directory, and git branches")
        .long_about(
            "Rename a workspace, its directory, and git branches.\n\n\
             Atomically renames the workspace directory, updates .wsp.yaml metadata, and \
             renames the workspace branch in every repo clone. Remote tracking branches \
             are not affected — push the renamed branch manually if needed.",
        )
        .arg(
            Arg::new("old")
                .required(true)
                .add(ArgValueCandidates::new(completers::complete_workspaces)),
        )
        .arg(Arg::new("new").required(true))
}

pub fn run(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let old_name = matches.get_one::<String>("old").unwrap();
    let new_name = matches.get_one::<String>("new").unwrap();

    let results = workspace::rename(paths, old_name, new_name)?;

    let mut lines = vec![format!(
        "Renamed workspace {:?} -> {:?}",
        old_name, new_name
    )];
    for r in &results {
        lines.push(format!(
            "  {}    branch: {} -> {}",
            r.name, r.old_branch, r.new_branch,
        ));
    }

    let new_dir = workspace::dir(&paths.workspaces_dir, new_name);
    let new_branch = results
        .first()
        .map(|r| r.new_branch.as_str())
        .unwrap_or(new_name);
    Ok(Output::Mutation(
        MutationOutput::new(lines.join("\n")).with_workspace(
            new_name,
            new_dir.display().to_string(),
            new_branch,
        ),
    ))
}
