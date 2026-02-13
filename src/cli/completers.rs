use std::collections::HashSet;

use clap_complete::engine::CompletionCandidate;

use crate::config::{Config, Paths};
use crate::giturl;
use crate::group;
use crate::workspace;

pub fn complete_groups() -> Vec<CompletionCandidate> {
    let Ok(paths) = Paths::resolve() else {
        return Vec::new();
    };
    let Ok(cfg) = Config::load_from(&paths.config_path) else {
        return Vec::new();
    };
    group::list(&cfg)
        .into_iter()
        .map(CompletionCandidate::new)
        .collect()
}

pub fn complete_repos() -> Vec<CompletionCandidate> {
    let Ok(paths) = Paths::resolve() else {
        return Vec::new();
    };
    let Ok(cfg) = Config::load_from(&paths.config_path) else {
        return Vec::new();
    };
    repos_to_candidates(cfg.repos.keys().cloned().collect())
}

/// Complete only repos that are IN the group being updated (for --remove).
pub fn complete_group_repos_remove() -> Vec<CompletionCandidate> {
    let Some(group_repos) = group_repos_from_args() else {
        return Vec::new();
    };
    repos_to_candidates(group_repos)
}

/// Complete only repos that are NOT in the group being updated (for --add).
pub fn complete_group_repos_add() -> Vec<CompletionCandidate> {
    let Ok(paths) = Paths::resolve() else {
        return Vec::new();
    };
    let Ok(cfg) = Config::load_from(&paths.config_path) else {
        return Vec::new();
    };
    let all: Vec<String> = cfg.repos.keys().cloned().collect();

    let Some(in_group) = group_repos_from_args() else {
        return repos_to_candidates(all);
    };
    let in_group_set: HashSet<&str> = in_group.iter().map(|s| s.as_str()).collect();
    let not_in_group: Vec<String> = all
        .into_iter()
        .filter(|r| !in_group_set.contains(r.as_str()))
        .collect();
    repos_to_candidates(not_in_group)
}

/// Complete only repos in the current workspace (for `ws repo rm`).
pub fn complete_workspace_repos() -> Vec<CompletionCandidate> {
    let Ok(cwd) = std::env::current_dir() else {
        return Vec::new();
    };
    let Ok(ws_dir) = workspace::detect(&cwd) else {
        return Vec::new();
    };
    let Ok(meta) = workspace::load_metadata(&ws_dir) else {
        return Vec::new();
    };
    repos_to_candidates(meta.repos.keys().cloned().collect())
}

pub fn complete_workspaces() -> Vec<CompletionCandidate> {
    let Ok(paths) = Paths::resolve() else {
        return Vec::new();
    };
    let Ok(names) = workspace::list_all(&paths.workspaces_dir) else {
        return Vec::new();
    };
    names.into_iter().map(CompletionCandidate::new).collect()
}

fn repos_to_candidates(identities: Vec<String>) -> Vec<CompletionCandidate> {
    let shortnames = giturl::shortnames(&identities);
    shortnames
        .into_iter()
        .map(|(identity, short)| CompletionCandidate::new(short).help(Some(identity.into())))
        .collect()
}

/// Context-aware completer: `ArgValueCandidates` closures receive no parsed
/// state, so we extract tokens from `std::env::args()` directly. This works
/// because the binary is re-invoked with the partial command line during
/// completion. See `extract_group_name_after_update` for the parsing pattern.
fn group_repos_from_args() -> Option<Vec<String>> {
    let args: Vec<String> = std::env::args().collect();
    let group_name = extract_group_name_after_update(&args)?;

    let paths = Paths::resolve().ok()?;
    let cfg = Config::load_from(&paths.config_path).ok()?;
    group::get(&cfg, group_name).ok()
}

/// Find the `["group", "update"]` window in args and return the next
/// non-flag token (the group name).
fn extract_group_name_after_update(args: &[String]) -> Option<&str> {
    let pos = args
        .windows(2)
        .position(|w| w[0] == "group" && w[1] == "update")?;
    args.get(pos + 2)
        .map(|s| s.as_str())
        .filter(|a| !a.starts_with('-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| a.to_string()).collect()
    }

    #[test]
    fn test_extract_group_name() {
        struct Case {
            name: &'static str,
            args: Vec<String>,
            want: Option<&'static str>,
        }

        let cases = vec![
            Case {
                name: "normal usage",
                args: s(&[
                    "ws", "setup", "group", "update", "backend", "--remove", "repo-a",
                ]),
                want: Some("backend"),
            },
            Case {
                name: "no update subcommand",
                args: s(&["ws", "setup", "group", "show", "backend"]),
                want: None,
            },
            Case {
                name: "update is last token",
                args: s(&["ws", "setup", "group", "update"]),
                want: None,
            },
            Case {
                name: "flag immediately after update",
                args: s(&["ws", "setup", "group", "update", "--help"]),
                want: None,
            },
            Case {
                name: "bare update without group prefix",
                args: s(&["ws", "update", "backend"]),
                want: None,
            },
            Case {
                name: "group named update",
                args: s(&["ws", "setup", "group", "update", "update", "--remove"]),
                want: Some("update"),
            },
        ];

        for tc in cases {
            let got = extract_group_name_after_update(&tc.args);
            assert_eq!(got, tc.want, "case: {}", tc.name);
        }
    }
}
