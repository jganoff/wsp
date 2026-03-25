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
             are not affected — push the renamed branch manually if needed.\n\n\
             Use '.' as <old> to rename the current workspace.",
        )
        .arg(
            Arg::new("old")
                .required(true)
                .add(ArgValueCandidates::new(completers::complete_workspaces)),
        )
        .arg(Arg::new("new").required(true))
}

/// Resolve the `old` argument: `"."` → current workspace name, anything else → as-is.
fn resolve_old_name(old_raw: &str, cwd: &std::path::Path) -> Result<String> {
    if old_raw == "." {
        let ws_dir = workspace::detect(cwd)
            .map_err(|_| anyhow::anyhow!("not inside a workspace (cannot resolve '.')"))?;
        ws_dir
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| anyhow::anyhow!("could not determine workspace name from path"))
            .map(|s| s.to_owned())
    } else {
        Ok(old_raw.to_owned())
    }
}

pub fn run(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let old_raw = matches.get_one::<String>("old").unwrap();
    let cwd = std::env::current_dir()?;
    let old_name = resolve_old_name(old_raw, &cwd)?;
    let new_name = matches.get_one::<String>("new").unwrap();

    let results = workspace::rename(paths, &old_name, new_name)?;

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
}
