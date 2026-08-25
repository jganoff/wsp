use anyhow::Result;
use clap::{Arg, ArgMatches, Command};

use wsp_core::config::Paths;
use wsp_core::gc;
use wsp_core::gc::GcEntry;
use wsp_core::output::{ListState, Output, WorkspaceListEntry, WorkspaceListOutput};
use wsp_core::workspace;

pub fn cmd() -> Command {
    Command::new("ls")
        .add(crate::shellnav::ShellNav::none())
        .visible_alias("list")
        .about("List workspaces [read-only]")
        .long_about(
            "List workspaces [read-only].\n\n\
             Shows all workspaces under the workspaces directory, with their branch, repo \
             count, and description, sorted by name. -t and -U sort by time instead.\n\n\
             With --removed, lists removed workspaces that `wsp recover <name>` can still \
             restore, soonest to expire first, showing when each one went and how \
             long is left. -t and -U sort those by removal time instead, since they \
             have no creation date; add -r to flip either.",
        )
        .arg(
            Arg::new("removed")
                .long("removed")
                .action(clap::ArgAction::SetTrue)
                .help("List removed workspaces that can still be recovered"),
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
    let sort_time = flag(matches, "time");
    let sort_created = flag(matches, "creation");
    let reverse = flag(matches, "reverse");
    let removed = flag(matches, "removed");

    let mut workspaces = if removed {
        removed_entries(paths)?
    } else {
        active_entries(paths)?
    };
    sort_entries(&mut workspaces, sort_time || sort_created, reverse);

    Ok(Output::WorkspaceList(WorkspaceListOutput {
        // Only the active listing gets the footer: under `--removed` the table
        // already is the removed workspaces, so pointing at them is noise.
        hint: if removed {
            None
        } else {
            removed_footer(
                &gc::list(&paths.gc_dir).unwrap_or_default(),
                gc::retention_days(paths),
            )
        },
        state: if removed {
            ListState::Removed
        } else {
            ListState::Active
        },
        workspaces,
    }))
}

fn flag(matches: &ArgMatches, name: &str) -> bool {
    matches
        .try_get_one::<bool>(name)
        .ok()
        .flatten()
        .copied()
        .unwrap_or(false)
}

fn sort_entries(entries: &mut [WorkspaceListEntry], by_time: bool, reverse: bool) {
    if by_time {
        // Newest first (reverse chronological).
        entries.sort_by(|a, b| sort_key(b).cmp(sort_key(a)));
    }
    if reverse {
        entries.reverse();
    }
}

/// The timestamp `-t` and `-U` sort on.
///
/// One key for both flags: `-t` is meant to prefer last use, but nothing writes
/// `Metadata::last_used` yet, so there is no second key to prefer. Give `-t` its
/// own branch here when there is.
fn sort_key(e: &WorkspaceListEntry) -> &str {
    if !e.created.is_empty() {
        return &e.created;
    }
    // A removed workspace keeps no creation date, so it sorts by when it was
    // removed -- the only time it carries. Entries with neither (unreadable
    // metadata) sort last.
    e.removed_at.as_deref().unwrap_or("")
}

/// Footer for the active listing.
///
/// Removed workspaces are invisible in `wsp ls` and they expire, so the count
/// is worth a line. The soonest one is named only when it is close enough to
/// act on -- otherwise the count and the command are enough.
fn removed_footer(entries: &[gc::GcEntry], retention_days: u32) -> Option<String> {
    // Asked for, not assumed: `gc::list` sorts by name, like `wsp ls`.
    let soonest = entries.iter().min_by_key(|e| e.trashed_at)?;
    let count = format!(
        "{} removed workspace{} recoverable",
        entries.len(),
        if entries.len() == 1 { "" } else { "s" }
    );
    let deadline = gc::expires_at(&soonest.trashed_at, retention_days);
    let urgent = deadline.is_some_and(|at| at - chrono::Utc::now() < IMMINENT);
    Some(if urgent {
        format!(
            "{}; {:?} expires {} (wsp ls --removed)",
            count,
            soonest.name,
            crate::output::format_expiry(deadline)
        )
    } else {
        format!("{} (wsp ls --removed)", count)
    })
}

/// How close an expiry has to be before the footer names the workspace instead
/// of just counting. A day is enough notice to act and rare enough not to nag.
const IMMINENT: chrono::TimeDelta = chrono::TimeDelta::hours(24);

fn active_entries(paths: &Paths) -> Result<Vec<WorkspaceListEntry>> {
    let mut entries = Vec::new();
    for name in &workspace::list_all(&paths.workspaces_dir)? {
        let ws_dir = workspace::dir(&paths.workspaces_dir, name);
        let path = ws_dir.display().to_string();
        let Ok(meta) = workspace::load_metadata(&ws_dir) else {
            entries.push(WorkspaceListEntry {
                name: name.clone(),
                branch: "ERROR".to_string(),
                repo_count: 0,
                repos: vec![],
                path,
                removed_at: None,
                expires_at: None,
                description: None,
                created: String::new(),
                last_used: None,
                created_from: None,
            });
            continue;
        };
        let mut repos: Vec<String> = meta.repos.keys().cloned().collect();
        repos.sort();
        entries.push(WorkspaceListEntry {
            name: name.clone(),
            branch: meta.branch,
            repo_count: repos.len(),
            repos,
            path,
            removed_at: None,
            expires_at: None,
            description: meta.description,
            created: meta.created.to_rfc3339(),
            last_used: None,
            created_from: meta.created_from,
        });
    }
    Ok(entries)
}

fn removed_entries(paths: &Paths) -> Result<Vec<WorkspaceListEntry>> {
    let retention_days = gc::retention_days(paths);
    let mut entries: Vec<GcEntry> = gc::list(&paths.gc_dir)?;
    // Soonest to expire first. The active listing defaults to name order,
    // because a name is a stable index into work in progress; this listing
    // answers "what am I about to lose", so the deadline leads. `-t`/`-U`/`-r`
    // override it either way.
    //
    // Presentation lives here rather than in `gc::list`, which sorts by name so
    // that no caller inherits an order it did not ask for.
    entries.sort_by_key(|e| e.trashed_at);
    Ok(entries
        .into_iter()
        .map(|e| {
            let mut repos = e.repos;
            repos.sort();
            WorkspaceListEntry {
                name: e.name,
                branch: e.branch,
                repo_count: repos.len(),
                repos,
                path: e.gc_path,
                removed_at: Some(e.trashed_at.to_rfc3339()),
                // Absent when retention is disabled -- the entry never expires,
                // and a null is a truer answer than a date that will not happen.
                expires_at: gc::expires_at(&e.trashed_at, retention_days).map(|t| t.to_rfc3339()),
                description: None,
                // A removed workspace's creation time is not kept; its removal
                // time is the timestamp that matters for what you do next.
                created: String::new(),
                last_used: None,
                created_from: None,
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An entry with only the fields the sort looks at.
    fn entry(name: &str, created: &str, removed_at: Option<&str>) -> WorkspaceListEntry {
        WorkspaceListEntry {
            name: name.into(),
            branch: name.into(),
            repo_count: 0,
            repos: vec![],
            path: format!("/ws/{}", name),
            removed_at: removed_at.map(String::from),
            expires_at: None,
            description: None,
            created: created.into(),
            last_used: None,
            created_from: None,
        }
    }

    fn names(entries: &[WorkspaceListEntry]) -> Vec<&str> {
        entries.iter().map(|e| e.name.as_str()).collect()
    }

    #[test]
    fn sorts_newest_first_and_reverses() {
        let mut entries = [
            entry("old", "2026-01-01T00:00:00+00:00", None),
            entry("new", "2026-03-01T00:00:00+00:00", None),
            entry("mid", "2026-02-01T00:00:00+00:00", None),
        ];

        sort_entries(&mut entries, true, false);
        assert_eq!(names(&entries), vec!["new", "mid", "old"]);

        sort_entries(&mut entries, true, true);
        assert_eq!(names(&entries), vec!["old", "mid", "new"]);
    }

    #[test]
    fn entries_with_no_timestamp_sort_last() {
        let mut entries = [
            entry("error-ws", "", None),
            entry("good", "2026-03-01T00:00:00+00:00", None),
        ];

        sort_entries(&mut entries, true, false);
        assert_eq!(names(&entries), vec!["good", "error-ws"]);
    }

    #[test]
    fn removed_entries_sort_by_when_they_were_removed() {
        // A removed workspace has no `created`, so sorting on that alone would
        // leave `wsp ls --removed -U` in arbitrary order.
        let mut entries = [
            entry("first-out", "", Some("2026-01-01T00:00:00+00:00")),
            entry("last-out", "", Some("2026-03-01T00:00:00+00:00")),
        ];

        sort_entries(&mut entries, true, false);
        assert_eq!(names(&entries), vec!["last-out", "first-out"]);
    }

    /// A gc'd workspace on disk, so `removed_entries` reads real state.
    fn gc_fixture(tmp: &std::path::Path, name: &str) -> Paths {
        let paths = wsp_core::config::Paths::from_dirs(&tmp.join("data"), &tmp.join("ws"));
        let ws_dir = paths.workspaces_dir.join(name);
        std::fs::create_dir_all(&ws_dir).unwrap();
        std::fs::write(
            ws_dir.join(".wsp.yaml"),
            format!(
                "version: 0\nname: {name}\nbranch: {name}\nrepos: {{}}\n\
                 created: 2026-01-01T00:00:00Z\ndirs: {{}}\nsetup_commands: {{}}\n"
            ),
        )
        .unwrap();
        wsp_core::gc::move_to_gc(&paths, name, name).unwrap();
        paths
    }

    fn removed(name: &str, days_ago: i64) -> gc::GcEntry {
        gc::GcEntry {
            name: name.into(),
            branch: name.into(),
            trashed_at: chrono::Utc::now() - chrono::Duration::days(days_ago),
            original_path: format!("/ws/{}", name),
            gc_path: format!("/gc/{}", name),
            repos: vec![],
        }
    }

    #[test]
    fn removed_entries_map_gc_state_onto_the_listing_shape() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = gc_fixture(tmp.path(), "gone");

        let entries = removed_entries(&paths).unwrap();
        assert_eq!(entries.len(), 1);
        let e = &entries[0];

        assert_eq!(e.name, "gone");
        // `path` is where the files are now, not where a restore would put
        // them -- the listing describes the present.
        assert!(
            e.path.starts_with(&paths.gc_dir.display().to_string()),
            "path should point into the gc dir, got {}",
            e.path
        );
        assert!(
            e.removed_at.is_some(),
            "removed_at is what marks it removed"
        );
        assert!(e.expires_at.is_some(), "default retention expires");
        assert!(
            e.created.is_empty(),
            "a removed workspace keeps no creation date"
        );
        assert_eq!(e.repo_count, e.repos.len());
    }

    #[test]
    fn removed_entries_never_expire_when_gc_is_disabled() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = gc_fixture(tmp.path(), "gone");
        std::fs::write(&paths.config_path, "gc_retention_days: 0\n").unwrap();

        let entries = removed_entries(&paths).unwrap();
        assert_eq!(
            entries[0].expires_at, None,
            "a null is truer than a date that will never arrive"
        );
    }

    #[test]
    fn footer_is_absent_when_nothing_is_recoverable() {
        assert_eq!(removed_footer(&[], 7), None);
    }

    #[test]
    fn footer_counts_and_pluralizes() {
        assert_eq!(
            removed_footer(&[removed("a", 1)], 7).unwrap(),
            "1 removed workspace recoverable (wsp ls --removed)"
        );
        assert_eq!(
            removed_footer(&[removed("a", 1), removed("b", 1)], 7).unwrap(),
            "2 removed workspaces recoverable (wsp ls --removed)"
        );
    }

    #[test]
    fn removed_listing_leads_with_the_soonest_to_expire() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = gc_fixture(tmp.path(), "zzz-oldest");
        // Same gc dir, a second workspace removed later. Name order would put
        // "aaa" first; expiry order must not.
        let ws_dir = paths.workspaces_dir.join("aaa-newest");
        std::fs::create_dir_all(&ws_dir).unwrap();
        std::fs::write(
            ws_dir.join(".wsp.yaml"),
            "version: 0\nname: aaa-newest\nbranch: aaa-newest\nrepos: {}\n\
             created: 2026-01-01T00:00:00Z\ndirs: {}\nsetup_commands: {}\n",
        )
        .unwrap();
        wsp_core::gc::move_to_gc(&paths, "aaa-newest", "aaa-newest").unwrap();

        let names: Vec<String> = removed_entries(&paths)
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(
            names,
            vec!["zzz-oldest", "aaa-newest"],
            "the workspace closest to being purged must come first"
        );
    }

    #[test]
    fn footer_finds_the_soonest_regardless_of_input_order() {
        // Newest first, the order `gc::list` would give.
        let footer = removed_footer(&[removed("fresh", 0), removed("nearly-gone", 7)], 7).unwrap();
        assert!(
            footer.contains("\"nearly-gone\""),
            "must name the soonest to expire, not the first in the slice: {footer}"
        );
    }

    #[test]
    fn footer_names_the_workspace_about_to_expire() {
        // 7-day retention, removed 7 days ago: hours left, so this is the last
        // chance to act and the name is worth the extra words.
        let footer = removed_footer(&[removed("nearly-gone", 7)], 7).unwrap();
        assert!(
            footer.contains("\"nearly-gone\"") && footer.contains("expires"),
            "expected the imminent workspace to be named: {footer}"
        );
    }

    #[test]
    fn footer_stays_quiet_about_expiry_when_gc_is_disabled() {
        // Retention 0 means nothing ever expires, however old the entry is.
        let footer = removed_footer(&[removed("ancient", 900)], 0).unwrap();
        assert_eq!(
            footer, "1 removed workspace recoverable (wsp ls --removed)",
            "nothing expires with gc disabled, so nothing is urgent"
        );
    }

    #[test]
    fn no_sort_flag_leaves_the_order_alone() {
        let mut entries = [
            entry("b", "2026-01-01T00:00:00+00:00", None),
            entry("a", "2026-03-01T00:00:00+00:00", None),
        ];

        sort_entries(&mut entries, false, false);
        assert_eq!(names(&entries), vec!["b", "a"]);
    }
}
