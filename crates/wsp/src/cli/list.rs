use anyhow::Result;
use clap::{Arg, ArgMatches, Command};

use wsp_core::config::Paths;
use wsp_core::output::{Output, WorkspaceListEntry, WorkspaceListOutput};
use wsp_core::workspace;

pub fn cmd() -> Command {
    Command::new("ls")
        .add(crate::shellnav::ShellNav::none())
        .visible_alias("list")
        .about("List active workspaces [read-only]")
        .long_about(
            "List active workspaces [read-only].\n\n\
             Shows all workspaces under the workspaces directory, with their branch, repo \
             count, and description. Supports sorting by name (default), last-used time, \
             or creation date.",
        )
        .arg(
            Arg::new("time")
                .short('t')
                .action(clap::ArgAction::SetTrue)
                .help("Sort by last used, newest first (falls back to created)"),
        )
        .arg(
            Arg::new("creation")
                .short('U')
                .action(clap::ArgAction::SetTrue)
                .help("Sort by creation date, newest first"),
        )
        .arg(
            Arg::new("reverse")
                .short('r')
                .action(clap::ArgAction::SetTrue)
                .help("Reverse sort order"),
        )
        .group(
            clap::ArgGroup::new("sort")
                .args(["time", "creation"])
                .required(false),
        )
}

pub fn run(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let sort_time = matches
        .try_get_one::<bool>("time")
        .ok()
        .flatten()
        .copied()
        .unwrap_or(false);
    let sort_created = matches
        .try_get_one::<bool>("creation")
        .ok()
        .flatten()
        .copied()
        .unwrap_or(false);
    let reverse = matches
        .try_get_one::<bool>("reverse")
        .ok()
        .flatten()
        .copied()
        .unwrap_or(false);

    let names = workspace::list_all(&paths.workspaces_dir)?;

    let mut workspaces = Vec::new();
    for name in &names {
        let ws_dir = workspace::dir(&paths.workspaces_dir, name);
        let meta = match workspace::load_metadata(&ws_dir) {
            Ok(m) => m,
            Err(_) => {
                workspaces.push(WorkspaceListEntry {
                    name: name.clone(),
                    branch: "ERROR".to_string(),
                    repo_count: 0,
                    path: ws_dir.display().to_string(),
                    description: None,
                    created: String::new(),
                    last_used: None,
                    created_from: None,
                });
                continue;
            }
        };
        workspaces.push(WorkspaceListEntry {
            name: name.clone(),
            branch: meta.branch,
            repo_count: meta.repos.len(),
            path: ws_dir.display().to_string(),
            description: meta.description,
            created: meta.created.to_rfc3339(),
            last_used: None,
            created_from: meta.created_from,
        });
    }

    // Sort by requested criteria
    if sort_time || sort_created {
        // Both -t and -U sort by timestamp. -t uses last_used with created fallback;
        // -U uses created directly. Since last_used is not yet tracked, both
        // currently sort by created.
        workspaces.sort_by(|a, b| {
            let ts_a = if sort_time {
                a.last_used.as_deref().unwrap_or(&a.created)
            } else {
                &a.created
            };
            let ts_b = if sort_time {
                b.last_used.as_deref().unwrap_or(&b.created)
            } else {
                &b.created
            };
            // Newest first (reverse chronological)
            ts_b.cmp(ts_a)
        });
    }

    if reverse {
        workspaces.reverse();
    }

    Ok(Output::WorkspaceList(WorkspaceListOutput {
        hint: None,
        workspaces,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sort_by_created() {
        let mut entries = [
            WorkspaceListEntry {
                name: "old".into(),
                branch: "old".into(),
                repo_count: 1,
                path: "/ws/old".into(),
                description: None,
                created: "2026-01-01T00:00:00+00:00".into(),
                last_used: None,
                created_from: None,
            },
            WorkspaceListEntry {
                name: "new".into(),
                branch: "new".into(),
                repo_count: 1,
                path: "/ws/new".into(),
                description: None,
                created: "2026-03-01T00:00:00+00:00".into(),
                last_used: None,
                created_from: None,
            },
            WorkspaceListEntry {
                name: "mid".into(),
                branch: "mid".into(),
                repo_count: 1,
                path: "/ws/mid".into(),
                description: None,
                created: "2026-02-01T00:00:00+00:00".into(),
                last_used: None,
                created_from: None,
            },
        ];

        // Sort by created (newest first)
        entries.sort_by(|a, b| b.created.cmp(&a.created));
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["new", "mid", "old"]);

        // Reverse: oldest first
        entries.reverse();
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["old", "mid", "new"]);
    }

    #[test]
    fn test_sort_empty_created_sorts_last() {
        let mut entries = [
            WorkspaceListEntry {
                name: "error-ws".into(),
                branch: "ERROR".into(),
                repo_count: 0,
                path: "/ws/error".into(),
                description: None,
                created: String::new(),
                last_used: None,
                created_from: None,
            },
            WorkspaceListEntry {
                name: "good".into(),
                branch: "good".into(),
                repo_count: 1,
                path: "/ws/good".into(),
                description: None,
                created: "2026-03-01T00:00:00+00:00".into(),
                last_used: None,
                created_from: None,
            },
        ];

        // Newest first — empty string sorts last (less than any RFC3339 timestamp)
        entries.sort_by(|a, b| b.created.cmp(&a.created));
        assert_eq!(entries[0].name, "good");
        assert_eq!(entries[1].name, "error-ws");
    }
}
