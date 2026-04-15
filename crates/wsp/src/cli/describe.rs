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
             The workspace name is optional when running from inside a workspace directory.\n\n\
             Use -- to pass multi-word descriptions without quoting:\n\
             \x20 wsp describe -- claude --resume abc123\n\
             \x20 wsp describe my-ws -- some long description here",
        )
        .override_usage("wsp describe <text>\n       wsp describe [workspace] -- <text>...")
        .arg(
            Arg::new("workspace")
                .required(false)
                .add(ArgValueCandidates::new(completers::complete_workspaces)),
        )
        .arg(
            Arg::new("text")
                .num_args(1..)
                .last(true)
                .allow_hyphen_values(true)
                .help("Description text; use -- to pass multiple words without quoting"),
        )
}

/// Resolve workspace name and description text from the positional args.
///
/// Three forms:
/// - `wsp describe <text>`: single positional is text, workspace from CWD.
/// - `wsp describe <ws> -- <text>...`: explicit workspace, trailing tokens joined.
/// - `wsp describe -- <text>...`: no workspace before `--`, workspace from CWD.
fn resolve_args(matches: &ArgMatches) -> Result<(String, String)> {
    let ws_arg = matches.get_one::<String>("workspace");
    let text_args: Option<Vec<String>> = matches
        .get_many::<String>("text")
        .map(|vals| vals.cloned().collect());

    match (ws_arg, text_args) {
        (Some(ws), Some(parts)) => Ok((ws.clone(), parts.join(" "))),
        (Some(text), None) => {
            let cwd = std::env::current_dir()?;
            let ws_dir = workspace::detect(&cwd)?;
            let meta = workspace::load_metadata(&ws_dir)?;
            Ok((meta.name, text.clone()))
        }
        (None, Some(parts)) => {
            let cwd = std::env::current_dir()?;
            let ws_dir = workspace::detect(&cwd)?;
            let meta = workspace::load_metadata(&ws_dir)?;
            Ok((meta.name, parts.join(" ")))
        }
        (None, None) => bail!("description text is required"),
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
    fn parse_one_arg_text_only() {
        let m = cmd().get_matches_from(["describe", "some description"]);
        assert_eq!(
            m.get_one::<String>("workspace").map(|s| s.as_str()),
            Some("some description")
        );
        assert!(m.get_many::<String>("text").is_none());
    }

    #[test]
    fn parse_workspace_with_trailing_text() {
        let m = cmd().get_matches_from(["describe", "my-ws", "--", "some", "long", "description"]);
        assert_eq!(
            m.get_one::<String>("workspace").map(|s| s.as_str()),
            Some("my-ws")
        );
        let text: Vec<&str> = m
            .get_many::<String>("text")
            .unwrap()
            .map(|s| s.as_str())
            .collect();
        assert_eq!(text, vec!["some", "long", "description"]);
    }

    #[test]
    fn parse_trailing_text_without_workspace() {
        let m = cmd().get_matches_from(["describe", "--", "claude", "--resume", "120701c6"]);
        assert!(m.get_one::<String>("workspace").is_none());
        let text: Vec<&str> = m
            .get_many::<String>("text")
            .unwrap()
            .map(|s| s.as_str())
            .collect();
        assert_eq!(text, vec!["claude", "--resume", "120701c6"]);
    }

    #[test]
    fn parse_trailing_text_with_hyphen_values() {
        let m =
            cmd().get_matches_from(["describe", "my-ws", "--", "claude", "--resume", "120701c6"]);
        let text: Vec<&str> = m
            .get_many::<String>("text")
            .unwrap()
            .map(|s| s.as_str())
            .collect();
        assert_eq!(text, vec!["claude", "--resume", "120701c6"]);
    }

    #[test]
    fn resolve_with_trailing_args_joins_text() {
        let m =
            cmd().get_matches_from(["describe", "my-ws", "--", "claude", "--resume", "120701c6"]);
        let (name, text) = resolve_args(&m).unwrap();
        assert_eq!(name, "my-ws");
        assert_eq!(text, "claude --resume 120701c6");
    }

    #[test]
    fn resolve_with_single_trailing_arg() {
        let m = cmd().get_matches_from(["describe", "my-ws", "--", "simple"]);
        let (name, text) = resolve_args(&m).unwrap();
        assert_eq!(name, "my-ws");
        assert_eq!(text, "simple");
    }

    #[test]
    fn two_bare_positionals_rejected() {
        // With last(true) on text, the old two-positional form requires --.
        let result = cmd().try_get_matches_from(["describe", "my-ws", "a description"]);
        assert!(result.is_err());
    }

    #[test]
    fn no_args_at_all() {
        let m = cmd().try_get_matches_from(["describe"]);
        // Clap allows it (both args optional), but resolve_args will bail.
        if let Ok(m) = m {
            assert!(resolve_args(&m).is_err());
        }
    }
}
