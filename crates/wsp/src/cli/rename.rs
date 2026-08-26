use crate::usage::UsageExt;
use anyhow::Result;
use clap::{Arg, ArgMatches, Command};
use clap_complete::engine::ArgValueCandidates;

use wsp_core::config::Paths;
use wsp_core::output::{MutationOutput, Output};
use wsp_core::workspace;

use super::completers;

pub fn cmd() -> Command {
    Command::new("rename")
        // Moves the workspace directory: the shell must step aside first (Windows
        // cannot rename a live cwd) and then follow it to the new location.
        .add(crate::shellnav::ShellNav::vacates_and_follows())
        .about("Rename a workspace, its directory, and git branches")
        .long_about(
            "Rename a workspace, its directory, and git branches.\n\n\
             Atomically renames the workspace directory, updates .wsp.yaml metadata, and \
             renames the workspace branch in every repo clone. Remote tracking branches \
             are not affected — push the renamed branch manually if needed.\n\n\
             The old workspace name is optional when running from inside a workspace \
             directory. Use '.' as <old> to explicitly name the current workspace.",
        )
        .usage("wsp rename [old] <new>")
        .arg(
            Arg::new("old")
                .required(true)
                .add(ArgValueCandidates::new(completers::complete_workspaces)),
        )
        .arg(Arg::new("new"))
}

/// Resolve the `old` argument: `"."` → current workspace name, anything else → as-is.
fn resolve_old_name(old_raw: &str, cwd: &std::path::Path) -> Result<String> {
    if old_raw == "." {
        detect_workspace_name(cwd)
            .map_err(|_| anyhow::anyhow!("not inside a workspace (cannot resolve '.')"))
    } else {
        Ok(old_raw.to_owned())
    }
}

/// Detect the workspace name from the current working directory.
fn detect_workspace_name(cwd: &std::path::Path) -> Result<String> {
    let ws_dir = workspace::detect(cwd)?;
    let meta = workspace::load_metadata(&ws_dir)?;
    Ok(meta.name)
}

/// Resolve old and new names from the positional args.
///
/// Two args: first is old name (supports "."), second is new name.
/// One arg: detect old name from CWD, the single arg is the new name.
fn resolve_names(matches: &ArgMatches) -> Result<(String, String)> {
    let first = matches.get_one::<String>("old").unwrap();
    let second = matches.get_one::<String>("new");
    // Via invocation_dir: the wrapper vacates before running rename, so the
    // process cwd is the workspaces root rather than where the user was. Both
    // the bare form and `.` resolve the old name from here.
    let cwd = crate::shellcd::invocation_dir()?;

    match second {
        Some(new_name) => {
            let old_name = resolve_old_name(first, &cwd)?;
            Ok((old_name, new_name.clone()))
        }
        None => {
            let old_name = detect_workspace_name(&cwd)?;
            Ok((old_name, first.clone()))
        }
    }
}

pub fn run(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let (old_name, new_name) = resolve_names(matches)?;

    let results = workspace::rename(paths, &old_name, &new_name)?;

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

    let new_dir = workspace::dir(&paths.workspaces_dir, &new_name);

    // Tell the shell wrapper where the workspace went. Without this a shell
    // standing inside the renamed workspace is left with a $PWD naming a path
    // that no longer exists.
    crate::shellcd::request(&new_dir);

    let new_branch = results
        .first()
        .map(|r| r.new_branch.as_str())
        .unwrap_or(&new_name);
    Ok(Output::Mutation(
        MutationOutput::new(lines.join("\n")).with_workspace(
            &new_name,
            new_dir.display().to_string(),
            new_branch,
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_workspace_dir(parent: &std::path::Path, name: &str) -> std::path::PathBuf {
        let ws_dir = parent.join(name);
        std::fs::create_dir_all(&ws_dir).unwrap();
        let meta = wsp_core::workspace::Metadata {
            version: 0,
            name: name.to_owned(),
            branch: format!("test/{}", name),
            repos: std::collections::BTreeMap::new(),
            created: chrono::Utc::now(),
            description: None,
            last_used: None,
            created_from: None,
            dirs: std::collections::BTreeMap::new(),
            config: None,
            setup_commands: std::collections::BTreeMap::new(),
        };
        wsp_core::workspace::save_metadata(&ws_dir, &meta).unwrap();
        ws_dir
    }

    #[test]
    fn resolve_dot_returns_workspace_name() {
        let tmp = tempfile::tempdir().unwrap();
        let ws_dir = make_workspace_dir(tmp.path(), "my-feature");
        // '.' resolves to the workspace name when cwd is inside the workspace dir
        let name = resolve_old_name(".", &ws_dir).unwrap();
        assert_eq!(name, "my-feature");
    }

    #[test]
    fn resolve_dot_errors_outside_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        // tmp dir has no .wsp.yaml, so detect() will fail
        let err = resolve_old_name(".", tmp.path()).unwrap_err();
        assert!(
            err.to_string().contains("not inside a workspace"),
            "expected 'not inside a workspace' error, got: {}",
            err
        );
    }

    #[test]
    fn resolve_literal_name_passthrough() {
        let tmp = tempfile::tempdir().unwrap();
        let name = resolve_old_name("my-workspace", tmp.path()).unwrap();
        assert_eq!(name, "my-workspace");
    }

    #[test]
    fn parse_two_args() {
        let m = cmd().get_matches_from(["rename", "old-ws", "new-ws"]);
        assert_eq!(
            m.get_one::<String>("old").map(|s| s.as_str()),
            Some("old-ws")
        );
        assert_eq!(
            m.get_one::<String>("new").map(|s| s.as_str()),
            Some("new-ws")
        );
    }

    #[test]
    fn parse_one_arg_new_name_only() {
        let m = cmd().get_matches_from(["rename", "new-ws"]);
        assert_eq!(
            m.get_one::<String>("old").map(|s| s.as_str()),
            Some("new-ws")
        );
        assert!(m.get_one::<String>("new").is_none());
    }
}
