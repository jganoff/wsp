use anyhow::{Result, bail};
use clap::{Arg, ArgMatches, Command};
use clap_complete::engine::ArgValueCandidates;

use wsp_core::config::Paths;
use wsp_core::filelock;
use wsp_core::output::{MutationOutput, Output};
use wsp_core::workspace;

use super::completers;

pub fn cmd() -> Command {
    Command::new("describe")
        .about("Set or update a workspace description")
        .long_about(
            "Set or update a workspace description.\n\n\
             Stores a short purpose string in the workspace metadata. The description \
             appears in `wsp ls` output to help identify workspaces at a glance.\n\n\
             The workspace name is optional when running from inside a workspace directory.",
        )
        .override_usage("wsp describe [workspace] <text>")
        .arg(
            Arg::new("workspace")
                .required(true)
                .add(ArgValueCandidates::new(completers::complete_workspaces)),
        )
        .arg(Arg::new("text"))
}

/// Resolve workspace name and description text from the positional args.
///
/// Two args: first is workspace name, second is text.
/// One arg: detect workspace from CWD, the single arg is text.
fn resolve_args(matches: &ArgMatches) -> Result<(String, String)> {
    let first = matches.get_one::<String>("workspace").unwrap();
    let second = matches.get_one::<String>("text");

    match second {
        Some(text) => Ok((first.clone(), text.clone())),
        None => {
            let cwd = std::env::current_dir()?;
            let ws_dir = workspace::detect(&cwd)?;
            let meta = workspace::load_metadata(&ws_dir)?;
            Ok((meta.name, first.clone()))
        }
    }
}

pub fn run(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let (ws_name, text) = resolve_args(matches)?;

    workspace::validate_name(&ws_name)?;
    let ws_dir = workspace::dir(&paths.workspaces_dir, &ws_name);
    if !ws_dir.join(workspace::METADATA_FILE).exists() {
        bail!("workspace '{}' not found", ws_name);
    }

    let desc = if text.is_empty() {
        None
    } else {
        Some(text.clone())
    };

    filelock::with_metadata(&ws_dir, |meta| {
        meta.description = desc;
        Ok(())
    })?;

    if text.is_empty() {
        Ok(Output::Mutation(MutationOutput::new(format!(
            "Description cleared for {}",
            ws_name
        ))))
    } else {
        Ok(Output::Mutation(MutationOutput::new(format!(
            "Description set for {}",
            ws_name
        ))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_two_args() {
        let m = cmd().get_matches_from(["describe", "my-ws", "some description"]);
        assert_eq!(
            m.get_one::<String>("workspace").map(|s| s.as_str()),
            Some("my-ws")
        );
        assert_eq!(
            m.get_one::<String>("text").map(|s| s.as_str()),
            Some("some description")
        );
    }

    #[test]
    fn parse_one_arg_text_only() {
        let m = cmd().get_matches_from(["describe", "some description"]);
        assert_eq!(
            m.get_one::<String>("workspace").map(|s| s.as_str()),
            Some("some description")
        );
        assert!(m.get_one::<String>("text").is_none());
    }

    #[test]
    fn resolve_with_two_args_uses_explicit_workspace() {
        let m = cmd().get_matches_from(["describe", "my-ws", "a description"]);
        let (name, text) = resolve_args(&m).unwrap();
        assert_eq!(name, "my-ws");
        assert_eq!(text, "a description");
    }
}
