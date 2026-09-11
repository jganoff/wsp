use std::fs;
use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::config::Paths;

/// Detect a cross-device rename failure.
///
/// Uses `ErrorKind::CrossesDevices` (stabilized Rust 1.82 / edition 2024) for
/// portability across macOS, Linux, and Windows. The old `raw_os_error() == 18`
/// approach was macOS/Linux-only; Windows uses a different errno for the same
/// condition.
fn is_cross_device(e: &std::io::Error) -> bool {
    e.kind() == std::io::ErrorKind::CrossesDevices
}

const GC_META_FILE: &str = ".wsp-gc.yaml";
pub const DEFAULT_RETENTION_DAYS: u32 = 7;
const GC_COOLDOWN_SECS: u64 = 3600; // 1 hour between auto-gc runs

/// A removed workspace: the metadata stored inside its gc directory, plus what
/// can only be known by reading that directory.
///
/// The last two fields are deliberately not persisted. `gc_path` *is* the
/// location of the file this was read from, and `repos` lives in the
/// workspace's own `.wsp.yaml`; writing either into `.wsp-gc.yaml` would be a
/// second copy of the truth that could disagree with the first. [`list`] fills
/// them in.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GcEntry {
    pub name: String,
    pub branch: String,
    pub trashed_at: DateTime<Utc>,
    pub original_path: String,
    /// Where the files sit now. `original_path` is where a restore puts them
    /// back.
    #[serde(skip)]
    pub gc_path: String,
    /// Repos in the removed workspace. Reading them is what produces a count
    /// anyway, and it is the detail `wsp recover show` existed to provide
    /// before `wsp ls --removed` absorbed it.
    #[serde(skip)]
    pub repos: Vec<String>,
    /// Disk usage, measured once when the workspace was removed.
    ///
    /// Persisted, unlike the two above, because nothing writes to a gc'd
    /// workspace after it lands here: the number cannot go stale, so listing it
    /// costs a metadata read instead of a walk over every file.
    ///
    /// `None` means nobody recorded it, which a reader answers by measuring.
    /// Distinct from `Some(0)`, which means the workspace really was empty.
    #[serde(default)]
    pub size_bytes: Option<u64>,
}

/// Copy `src` to `dest` recursively, then delete `src`.
///
/// This is the cross-filesystem fallback used by `move_dir`. Extracted as
/// `pub(crate)` so it can be tested independently of the EXDEV trigger.
///
/// If the copy fails, any partial `dest` is cleaned up before the error
/// propagates. If the copy succeeds but deleting `src` fails, `dest` is left
/// intact and the error is returned.
pub(crate) fn copy_then_delete(src: &Path, dest: &Path) -> Result<()> {
    copy_dir_recursive(src, dest).inspect_err(|_| {
        // Clean up partial copy before propagating the error
        let _ = fs::remove_dir_all(dest);
    })?;
    fs::remove_dir_all(src).map_err(Into::into)
}

/// Move a directory, falling back to recursive copy + delete if rename
/// fails with EXDEV (cross-filesystem). An incomplete copy is cleaned up
/// on failure so the gc area doesn't accumulate garbage.
fn move_dir(src: &Path, dest: &Path) -> Result<()> {
    match fs::rename(src, dest) {
        Ok(()) => Ok(()),
        Err(e) if is_cross_device(&e) => copy_then_delete(src, dest),
        Err(e) => Err(e.into()),
    }
}

fn copy_dir_recursive(src: &Path, dest: &Path) -> Result<()> {
    fs::create_dir_all(dest)?;
    for item in fs::read_dir(src)? {
        let item = item?;
        let ft = item.file_type()?;
        let src_path = item.path();
        let dest_path = dest.join(item.file_name());
        if ft.is_symlink() {
            let target = fs::read_link(&src_path)?;
            let res: std::io::Result<()> = {
                #[cfg(unix)]
                {
                    std::os::unix::fs::symlink(&target, &dest_path)
                }
                #[cfg(windows)]
                {
                    // Windows needs the file/dir variant chosen up front. Stat
                    // src_path (follows the link at its actual location, not
                    // &target which may resolve wrongly against the CWD).
                    if std::fs::metadata(&src_path)
                        .map(|m| m.is_dir())
                        .unwrap_or(false)
                    {
                        std::os::windows::fs::symlink_dir(&target, &dest_path)
                    } else {
                        std::os::windows::fs::symlink_file(&target, &dest_path)
                    }
                }
            };
            if let Err(e) = res {
                // Windows without Developer Mode can't create symlinks; skip this
                // entry rather than aborting the whole GC copy.
                if !crate::symlink::is_dev_mode_error(&e) {
                    return Err(e.into());
                }
            }
        } else if ft.is_dir() {
            copy_dir_recursive(&src_path, &dest_path)?;
        } else {
            fs::copy(&src_path, &dest_path)?;
        }
    }
    Ok(())
}

/// Load gc metadata from a workspace directory, if present.
/// Returns `Some(GcEntry)` when the workspace has been garbage-collected.
pub fn load_entry(ws_dir: &Path) -> Option<GcEntry> {
    let meta_path = ws_dir.join(GC_META_FILE);
    let data = crate::util::read_yaml_file(&meta_path).ok()?;
    serde_yaml_ng::from_str(&data).ok()
}

/// Check whether a workspace directory is gc'd and handle accordingly.
///
/// - `read_only = true`: returns `Ok(Some(warning))` — caller should display the warning
/// - `read_only = false`: returns an error (blocks mutating commands)
/// - No gc entry: returns `Ok(None)`
pub fn check_workspace(ws_dir: &Path, read_only: bool) -> Result<Option<String>> {
    if let Some(entry) = load_entry(ws_dir) {
        let date = entry.trashed_at.format("%Y-%m-%d %H:%M UTC");
        if read_only {
            Ok(Some(gc_workspace_warning(&entry.name, &date.to_string())))
        } else {
            anyhow::bail!(
                "this workspace was removed on {}. Use `wsp recover {}` to restore it.",
                date,
                entry.name
            );
        }
    } else {
        Ok(None)
    }
}

/// Build a prominent multi-line banner warning the user that the current
/// workspace has been moved to the GC area. Returns the warning as a plain
/// string; the caller is responsible for any ANSI styling.
pub fn gc_workspace_warning(name: &str, date: &str) -> String {
    // Inner width (between the ║ chars). Long enough for all fixed content plus
    // a workspace name up to ~30 chars before wrapping looks odd.
    const W: usize = 58;

    // Pad `content` with trailing spaces to fill the inner width, then wrap in ║…║.
    let row = |content: &str| -> String {
        let inner = format!(" {content}");
        let pad = W.saturating_sub(inner.chars().count());
        format!("║{}{}║", inner, " ".repeat(pad))
    };

    let mut lines = Vec::new();
    lines.push("╔══════════════════════════════════════════════════════════╗".to_string());
    lines.push(row(""));
    lines.push(row("  ⚠  WORKSPACE REMOVED"));
    lines.push(row(""));
    lines.push("╠══════════════════════════════════════════════════════════╣".to_string());
    lines.push(row(&format!("  Removed:  {date}")));
    lines.push(row(&format!("  Recover:  wsp recover {name}")));
    lines.push("╚══════════════════════════════════════════════════════════╝".to_string());
    lines.push(String::new());
    lines.join("\n")
}

/// Move a workspace directory to the gc area for deferred deletion.
///
/// Writes metadata inside the workspace dir first, then moves the whole
/// directory. Uses rename when possible, falls back to copy+delete for
/// cross-filesystem moves.
///
/// **Always use this function (not hand-written YAML) to create gc entries.**
/// `GcEntry` has no `#[serde(default)]` fields — missing fields cause silent
/// deserialization failure, so `check_workspace` returns `None` with no error.
pub fn move_to_gc(paths: &Paths, name: &str, branch: &str) -> Result<GcEntry> {
    // Returns the entry so callers can report the removal deadline from the
    // timestamp that was actually written, rather than calling `now()` again
    // and getting a different answer either side of local midnight.
    let ws_dir = crate::workspace::dir(&paths.workspaces_dir, name);
    // Capture once so the directory name and GcEntry.trashed_at are identical.
    let now = Utc::now();
    let timestamp = now.format("%Y%m%dT%H%M%S%.3f").to_string();
    let gc_name = format!("{}__{}", name, timestamp);
    let dest = paths.gc_dir.join(&gc_name);

    fs::create_dir_all(&paths.gc_dir)?;

    let mut entry = GcEntry {
        name: name.to_string(),
        branch: branch.to_string(),
        trashed_at: now,
        original_path: ws_dir.display().to_string(),
        gc_path: dest.display().to_string(),
        repos: read_repo_identities(&ws_dir),
        // Measured here, while the tree is still in one place and we are already
        // about to move every file in it, so the walk is marginal against the
        // removal itself. Doing it later would mean walking on every listing.
        size_bytes: Some(crate::dir_size(&ws_dir)),
    };
    let yaml = serde_yaml_ng::to_string(&entry)?;
    fs::write(ws_dir.join(GC_META_FILE), yaml)?;

    move_dir(&ws_dir, &dest)?;
    entry.gc_path = dest.display().to_string();
    Ok(entry)
}

/// Every recoverable workspace, with its repos and gc location.
///
/// Sorted by name, matching `wsp ls`: one command with one flag should not
/// change its default order depending on the flag. A name can appear more than
/// once (removed, recreated, removed again), so ties break by removal time,
/// newest first -- putting the entry `restore` would actually pick at the top
/// of its group.
///
/// Callers wanting "what expires next" must ask for it (`min_by_key`) rather
/// than read the first element. There is deliberately only one listing
/// function: a leaner variant that skipped the per-entry `.wsp.yaml` read
/// existed alongside this one and sorted the opposite way, so "the first entry"
/// meant different things depending on which a caller reached for. The saving
/// was a handful of small local reads.
pub fn list(gc_dir: &Path) -> Result<Vec<GcEntry>> {
    if !gc_dir.exists() {
        return Ok(vec![]);
    }

    let mut entries = Vec::new();
    for item in fs::read_dir(gc_dir)? {
        let item = item?;
        let path = item.path();
        if !path.is_dir() {
            continue;
        }
        if let Some(entry) = read_entry(&path) {
            entries.push(entry);
        }
    }

    entries.sort_by(|a, b| {
        a.name
            .cmp(&b.name)
            .then_with(|| b.trashed_at.cmp(&a.trashed_at))
    });
    Ok(entries)
}

/// Read one gc'd workspace's metadata, filling in what only its location knows.
///
/// `None` if the directory has no readable `.wsp-gc.yaml` -- something else's
/// directory, or a half-written one.
fn read_entry(dir: &Path) -> Option<GcEntry> {
    let data = crate::util::read_yaml_file(&dir.join(GC_META_FILE)).ok()?;
    let mut entry = serde_yaml_ng::from_str::<GcEntry>(&data).ok()?;
    entry.gc_path = dir.display().to_string();
    entry.repos = read_repo_identities(dir);
    // `size_bytes` is deliberately not recomputed: it was measured at removal
    // and a gc'd workspace does not change.
    Some(entry)
}

/// When a workspace removed at `trashed_at` expires, or `None` if it never
/// will because retention is disabled.
///
/// The one place the deadline is computed. Formatting it is
/// `wsp::output::format_expiry`.
pub fn expires_at(trashed_at: &DateTime<Utc>, retention_days: u32) -> Option<DateTime<Utc>> {
    if retention_days == 0 {
        return None;
    }
    Some(*trashed_at + chrono::Duration::days(retention_days as i64))
}

/// The retention window in effect for `paths`, in days; 0 means never expire.
pub fn retention_days(paths: &Paths) -> u32 {
    crate::config::Config::load_from(&paths.config_path)
        .unwrap_or_default()
        .retention_days()
}

/// Read repo identities from a gc'd workspace's .wsp.yaml.
fn read_repo_identities(ws_dir: &Path) -> Vec<String> {
    let meta_path = ws_dir.join(".wsp.yaml");
    let data = match crate::util::read_yaml_file(&meta_path) {
        Ok(d) => d,
        Err(_) => return vec![],
    };
    match serde_yaml_ng::from_str::<crate::workspace::Metadata>(&data) {
        Ok(meta) => meta.repos.keys().cloned().collect(),
        Err(_) => vec![],
    }
}

/// Restore a workspace from the gc area back to the workspaces directory.
pub fn restore(paths: &Paths, name: &str) -> Result<()> {
    let entries = find_entries(&paths.gc_dir, name)?;
    if entries.is_empty() {
        anyhow::bail!("no recoverable workspace named {:?}", name);
    }

    // Use the most recent entry
    let (gc_name, entry) = &entries[0];

    // Validate the deserialized name to prevent path traversal from tampered metadata
    crate::workspace::validate_name(&entry.name)?;

    let dest = crate::workspace::dir(&paths.workspaces_dir, &entry.name);
    // fs::rename on Unix fails atomically if dest is a non-empty directory,
    // so this check is a courtesy error message, not a security gate.
    if dest.exists() {
        // A partial workspace (directory exists but no .wsp.yaml) can be cleared
        // to make way for the recovered workspace — there is no metadata or repo
        // data to lose.
        let meta_path = dest.join(crate::workspace::METADATA_FILE);
        if !meta_path.exists() {
            fs::remove_dir_all(&dest).map_err(|e| {
                anyhow::anyhow!(
                    "removing partial workspace directory {:?}: {}",
                    entry.name,
                    e
                )
            })?;
        } else {
            anyhow::bail!(
                "workspace {:?} already exists; remove or rename it first",
                entry.name
            );
        }
    }

    let src = paths.gc_dir.join(gc_name);
    move_dir(&src, &dest)?;

    // Clean up gc metadata from the restored workspace
    let _ = fs::remove_file(dest.join(GC_META_FILE));

    Ok(())
}

/// Purge gc entries older than the retention period.
/// A `retention_days` of 0 means "never purge" — all entries are kept indefinitely.
/// Permanently delete gc entries past the retention window.
///
/// Returns the names removed rather than a count, so callers can say *what*
/// they deleted. A purge that reports only a number cannot be audited by the
/// person whose work it deleted.
pub fn purge(gc_dir: &Path, retention_days: u32) -> Result<Vec<String>> {
    if retention_days == 0 || !gc_dir.exists() {
        return Ok(Vec::new());
    }

    let cutoff = Utc::now() - chrono::Duration::days(retention_days as i64);
    let mut removed = Vec::new();
    let mut expired = Vec::new();

    for item in fs::read_dir(gc_dir)? {
        let item = item?;
        let path = item.path();
        if !path.is_dir() {
            continue;
        }

        let meta_path = path.join(GC_META_FILE);
        let data = match crate::util::read_yaml_file(&meta_path) {
            Ok(d) => d,
            Err(_) => continue,
        };
        let entry = match serde_yaml_ng::from_str::<GcEntry>(&data) {
            Ok(e) => e,
            Err(_) => continue,
        };

        if entry.trashed_at < cutoff {
            expired.push((path, entry));
        }
    }

    let total = expired.len();
    for (index, (path, entry)) in expired.into_iter().enumerate() {
        eprintln!("gc: [{}/{}] purging {}...", index + 1, total, entry.name);
        // Best-effort: continue purging others if one fails.
        if let Err(e) = fs::remove_dir_all(&path) {
            eprintln!("  warning: gc purge failed for {}: {}", entry.name, e);
        } else {
            removed.push(entry.name.clone());
        }
    }

    Ok(removed)
}

/// Run gc if enough time has passed since the last run.
/// Called opportunistically from hot paths (new, rm, sync, ls).
pub fn maybe_run(paths: &Paths, retention_days: u32) {
    let marker = paths.gc_dir.join(".gc-last");

    // Skip if gc dir doesn't exist (nothing to gc)
    if !paths.gc_dir.exists() {
        return;
    }

    // Skip if we ran recently
    if let Ok(meta) = fs::metadata(&marker)
        && let Ok(modified) = meta.modified()
        && modified.elapsed().unwrap_or(Duration::ZERO) < Duration::from_secs(GC_COOLDOWN_SECS)
    {
        return;
    }

    if retention_days == 0 {
        return; // never purge
    }
    // Say what was deleted. git is not silent about auto-gc either, and unlike
    // git's — which mostly repacks — every byte this does is destructive, so
    // silence would leave no record that recoverable work is gone.
    match purge(&paths.gc_dir, retention_days) {
        Ok(removed) if !removed.is_empty() => {
            eprintln!(
                "gc: purged {} expired workspace{} ({})",
                removed.len(),
                if removed.len() == 1 { "" } else { "s" },
                removed.join(", ")
            );
        }
        Ok(_) => {}
        Err(e) => eprintln!("  warning: gc failed: {}", e),
    }

    // Touch the marker file
    let _ = fs::write(&marker, "");
}

/// Find gc entries matching a workspace name, most recent first.
fn find_entries(gc_dir: &Path, name: &str) -> Result<Vec<(String, GcEntry)>> {
    if !gc_dir.exists() {
        return Ok(vec![]);
    }

    let mut matches = Vec::new();
    for item in fs::read_dir(gc_dir)? {
        let item = item?;
        let path = item.path();
        if !path.is_dir() {
            continue;
        }

        let meta_path = path.join(GC_META_FILE);
        let data = match crate::util::read_yaml_file(&meta_path) {
            Ok(d) => d,
            Err(_) => continue,
        };
        if let Ok(entry) = serde_yaml_ng::from_str::<GcEntry>(&data)
            && entry.name == name
        {
            let gc_name = path.file_name().unwrap().to_string_lossy().to_string();
            matches.push((gc_name, entry));
        }
    }

    matches.sort_by_key(|m| std::cmp::Reverse(m.1.trashed_at));
    Ok(matches)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Paths;

    fn test_paths(tmp: &Path) -> Paths {
        Paths::from_dirs(tmp, &tmp.join("workspaces"))
    }

    fn create_workspace(paths: &Paths, name: &str) {
        let ws_dir = paths.workspaces_dir.join(name);
        fs::create_dir_all(&ws_dir).unwrap();
        let meta = crate::workspace::Metadata {
            version: 0,
            name: name.to_string(),
            branch: format!("test/{}", name),
            repos: std::collections::BTreeMap::new(),
            created: Utc::now(),
            description: None,
            last_used: None,
            created_from: None,
            dirs: std::collections::BTreeMap::new(),
            config: None,
            setup_commands: std::collections::BTreeMap::new(),
        };
        let yaml = serde_yaml_ng::to_string(&meta).unwrap();
        fs::write(ws_dir.join(".wsp.yaml"), yaml).unwrap();
    }

    #[test]
    fn test_move_and_restore() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = test_paths(tmp.path());
        create_workspace(&paths, "my-feature");

        assert!(paths.workspaces_dir.join("my-feature").exists());

        move_to_gc(&paths, "my-feature", "test/my-feature").unwrap();
        assert!(!paths.workspaces_dir.join("my-feature").exists());

        let entries = list(&paths.gc_dir).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "my-feature");
        assert_eq!(entries[0].branch, "test/my-feature");

        restore(&paths, "my-feature").unwrap();
        assert!(paths.workspaces_dir.join("my-feature").exists());
        // gc metadata should be cleaned up after restore
        assert!(
            !paths
                .workspaces_dir
                .join("my-feature")
                .join(GC_META_FILE)
                .exists()
        );

        let entries = list(&paths.gc_dir).unwrap();
        assert_eq!(entries.len(), 0);
    }

    #[test]
    fn test_purge_expired() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = test_paths(tmp.path());
        create_workspace(&paths, "old-ws");

        move_to_gc(&paths, "old-ws", "test/old-ws").unwrap();

        // Backdate the entry to 10 days ago
        backdate_gc_entries(&paths.gc_dir, 10);

        let removed = purge(&paths.gc_dir, 7).unwrap();
        assert_eq!(removed.len(), 1);

        let entries = list(&paths.gc_dir).unwrap();
        assert_eq!(entries.len(), 0);
    }

    #[test]
    fn test_purge_keeps_recent() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = test_paths(tmp.path());
        create_workspace(&paths, "new-ws");

        move_to_gc(&paths, "new-ws", "test/new-ws").unwrap();

        let removed = purge(&paths.gc_dir, 7).unwrap();
        assert_eq!(removed.len(), 0);

        let entries = list(&paths.gc_dir).unwrap();
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn test_restore_conflict() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = test_paths(tmp.path());
        create_workspace(&paths, "conflict");

        move_to_gc(&paths, "conflict", "test/conflict").unwrap();
        create_workspace(&paths, "conflict"); // recreate

        let err = restore(&paths, "conflict").unwrap_err();
        assert!(err.to_string().contains("already exists"));
    }

    #[test]
    fn test_restore_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = test_paths(tmp.path());

        let err = restore(&paths, "nonexistent").unwrap_err();
        assert!(err.to_string().contains("no recoverable workspace"));
    }

    #[test]
    fn test_maybe_run_cooldown() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = test_paths(tmp.path());
        fs::create_dir_all(&paths.gc_dir).unwrap();

        // First run should touch the marker
        maybe_run(&paths, 7);
        assert!(paths.gc_dir.join(".gc-last").exists());

        // Create and gc an entry, then backdate it
        create_workspace(&paths, "ws1");
        move_to_gc(&paths, "ws1", "test/ws1").unwrap();
        backdate_gc_entries(&paths.gc_dir, 10);

        // Second run within cooldown should skip gc
        maybe_run(&paths, 7);
        assert_eq!(
            list(&paths.gc_dir).unwrap().len(),
            1,
            "gc should be skipped within cooldown"
        );
    }

    #[test]
    fn test_maybe_run_after_cooldown_expiry() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = test_paths(tmp.path());
        fs::create_dir_all(&paths.gc_dir).unwrap();

        // Create and gc a workspace, then backdate it past retention
        create_workspace(&paths, "ws-expired");
        move_to_gc(&paths, "ws-expired", "test/ws-expired").unwrap();
        backdate_gc_entries(&paths.gc_dir, 10); // 10 days old, retention is 7

        // Simulate cooldown expiry: no marker means elapsed check fails → GC runs
        let marker = paths.gc_dir.join(".gc-last");
        let _ = fs::remove_file(&marker);

        // maybe_run should now run gc and purge the expired entry
        maybe_run(&paths, 7);

        assert_eq!(
            list(&paths.gc_dir).unwrap().len(),
            0,
            "expired gc entry should be removed after cooldown expires"
        );
    }

    #[test]
    fn test_soft_delete_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = test_paths(tmp.path());
        create_workspace(&paths, "soft-del");

        crate::workspace::remove(&paths, "soft-del", true).unwrap();
        assert!(!paths.workspaces_dir.join("soft-del").exists());

        let entries = list(&paths.gc_dir).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "soft-del");

        // restore should bring it back
        restore(&paths, "soft-del").unwrap();
        assert!(paths.workspaces_dir.join("soft-del").exists());
    }

    #[test]
    fn test_load_entry_present() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = test_paths(tmp.path());
        create_workspace(&paths, "gc-test");

        move_to_gc(&paths, "gc-test", "test/gc-test").unwrap();

        // Find the gc'd directory
        let gc_dirs: Vec<_> = fs::read_dir(&paths.gc_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .collect();
        assert_eq!(gc_dirs.len(), 1);

        let entry = load_entry(&gc_dirs[0].path());
        assert!(entry.is_some());
        let entry = entry.unwrap();
        assert_eq!(entry.name, "gc-test");
        assert_eq!(entry.branch, "test/gc-test");
    }

    #[test]
    fn test_load_entry_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = test_paths(tmp.path());
        create_workspace(&paths, "normal-ws");

        let ws_dir = paths.workspaces_dir.join("normal-ws");
        assert!(load_entry(&ws_dir).is_none());
    }

    #[test]
    fn test_check_workspace_read_only_warns() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = test_paths(tmp.path());
        create_workspace(&paths, "warn-test");
        move_to_gc(&paths, "warn-test", "test/warn-test").unwrap();

        let gc_dirs: Vec<_> = fs::read_dir(&paths.gc_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .collect();

        // Read-only check should succeed and return a warning
        let result = check_workspace(&gc_dirs[0].path(), true).unwrap();
        assert!(result.is_some());
        let warning = result.unwrap();
        assert!(warning.contains("WORKSPACE REMOVED"));
        assert!(warning.contains("warn-test"));
    }

    #[test]
    fn test_check_workspace_mutating_blocks() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = test_paths(tmp.path());
        create_workspace(&paths, "block-test");
        move_to_gc(&paths, "block-test", "test/block-test").unwrap();

        let gc_dirs: Vec<_> = fs::read_dir(&paths.gc_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .collect();

        // Mutating check should fail
        let err = check_workspace(&gc_dirs[0].path(), false).unwrap_err();
        assert!(err.to_string().contains("was removed on"));
        assert!(err.to_string().contains("wsp recover"));
    }

    #[test]
    fn test_check_workspace_normal_passes() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = test_paths(tmp.path());
        create_workspace(&paths, "normal");

        let ws_dir = paths.workspaces_dir.join("normal");
        assert!(check_workspace(&ws_dir, true).unwrap().is_none());
        assert!(check_workspace(&ws_dir, false).unwrap().is_none());
    }

    #[test]
    #[cfg(unix)]
    fn test_copy_dir_recursive_preserves_symlinks() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        let dest = tmp.path().join("dest");
        fs::create_dir_all(&src).unwrap();

        // Regular file
        fs::write(src.join("file.txt"), "hello").unwrap();

        // Relative symlink
        std::os::unix::fs::symlink("file.txt", src.join("link.txt")).unwrap();

        // Subdirectory with a file
        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(src.join("sub/nested.txt"), "nested").unwrap();

        copy_dir_recursive(&src, &dest).unwrap();

        // Regular file copied
        assert_eq!(fs::read_to_string(dest.join("file.txt")).unwrap(), "hello");

        // Symlink preserved (not resolved to regular file)
        let link_meta = dest.join("link.txt").symlink_metadata().unwrap();
        assert!(link_meta.file_type().is_symlink());
        assert_eq!(
            fs::read_link(dest.join("link.txt"))
                .unwrap()
                .to_str()
                .unwrap(),
            "file.txt"
        );

        // Subdirectory recursed
        assert_eq!(
            fs::read_to_string(dest.join("sub/nested.txt")).unwrap(),
            "nested"
        );
    }

    /// Helper: create a workspace with repos in its metadata.
    fn create_workspace_with_repos(paths: &Paths, name: &str, repos: &[&str]) {
        let ws_dir = paths.workspaces_dir.join(name);
        fs::create_dir_all(&ws_dir).unwrap();
        let mut repo_map = std::collections::BTreeMap::new();
        for r in repos {
            repo_map.insert(r.to_string(), None);
        }
        let meta = crate::workspace::Metadata {
            version: 0,
            name: name.to_string(),
            branch: format!("test/{}", name),
            repos: repo_map,
            created: Utc::now(),
            description: None,
            last_used: None,
            created_from: None,
            dirs: std::collections::BTreeMap::new(),
            config: None,
            setup_commands: std::collections::BTreeMap::new(),
        };
        let yaml = serde_yaml_ng::to_string(&meta).unwrap();
        fs::write(ws_dir.join(".wsp.yaml"), yaml).unwrap();
    }

    #[test]
    fn test_purge_zero_retention_never_purges() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = test_paths(tmp.path());
        create_workspace(&paths, "keep-forever");

        move_to_gc(&paths, "keep-forever", "test/keep-forever").unwrap();
        backdate_gc_entries(&paths.gc_dir, 365); // backdate to a year ago

        let removed = purge(&paths.gc_dir, 0).unwrap();
        assert!(removed.is_empty(), "retention_days=0 should never purge");

        let entries = list(&paths.gc_dir).unwrap();
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn test_maybe_run_zero_retention_skips() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = test_paths(tmp.path());
        create_workspace(&paths, "keep-me");
        move_to_gc(&paths, "keep-me", "test/keep-me").unwrap();
        backdate_gc_entries(&paths.gc_dir, 365);

        maybe_run(&paths, 0);

        let entries = list(&paths.gc_dir).unwrap();
        assert_eq!(
            entries.len(),
            1,
            "maybe_run with 0 retention should skip gc"
        );
    }

    #[test]
    fn test_list_carries_repos_and_gc_path() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = test_paths(tmp.path());

        create_workspace_with_repos(
            &paths,
            "multi-repo",
            &["github.com/acme/api", "github.com/acme/web"],
        );
        move_to_gc(&paths, "multi-repo", "test/multi-repo").unwrap();

        create_workspace(&paths, "empty-ws");
        move_to_gc(&paths, "empty-ws", "test/empty-ws").unwrap();

        let entries = list(&paths.gc_dir).unwrap();
        assert_eq!(entries.len(), 2);

        // Find by name rather than index: the sort is not this test's subject.
        let multi = entries.iter().find(|e| e.name == "multi-repo").unwrap();
        let empty = entries.iter().find(|e| e.name == "empty-ws").unwrap();

        assert_eq!(multi.repos.len(), 2);
        assert!(multi.repos.contains(&"github.com/acme/api".to_string()));
        assert!(multi.repos.contains(&"github.com/acme/web".to_string()));
        assert_eq!(multi.branch, "test/multi-repo");
        // Points at the gc copy, not where it came from.
        assert!(multi.gc_path.contains("multi-repo"));
        assert!(Path::new(&multi.gc_path).is_dir());
        assert_ne!(multi.gc_path, multi.original_path);

        assert!(empty.repos.is_empty());
    }

    #[test]
    fn test_expires_at_none_when_retention_disabled() {
        let trashed_at: DateTime<Utc> = "2026-01-01T00:00:00Z".parse().unwrap();
        assert_eq!(expires_at(&trashed_at, 0), None);
        assert_eq!(
            expires_at(&trashed_at, 7),
            Some("2026-01-08T00:00:00Z".parse::<DateTime<Utc>>().unwrap())
        );
    }

    #[test]
    fn test_move_dir_rename_path() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        let dest = tmp.path().join("dest");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("file.txt"), "data").unwrap();

        move_dir(&src, &dest).unwrap();

        assert!(!src.exists(), "src should be gone after rename");
        assert_eq!(fs::read_to_string(dest.join("file.txt")).unwrap(), "data");
    }

    #[test]
    fn test_copy_then_delete_moves_content() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        let dest = tmp.path().join("dest");
        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(src.join("a.txt"), "hello").unwrap();
        fs::write(src.join("sub/b.txt"), "world").unwrap();

        copy_then_delete(&src, &dest).unwrap();

        assert!(
            !src.exists(),
            "src should be deleted after copy_then_delete"
        );
        assert_eq!(fs::read_to_string(dest.join("a.txt")).unwrap(), "hello");
        assert_eq!(fs::read_to_string(dest.join("sub/b.txt")).unwrap(), "world");
    }

    /// When the copy step fails, any partial dest must be cleaned up.
    #[test]
    #[cfg(unix)]
    fn test_copy_then_delete_cleans_up_dest_on_copy_failure() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        let dest = tmp.path().join("dest");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("file.txt"), "data").unwrap();

        // Make src unreadable so copy_dir_recursive fails on read_dir(src).
        fs::set_permissions(&src, fs::Permissions::from_mode(0o000)).unwrap();

        let result = copy_then_delete(&src, &dest);

        // Restore permissions so tempdir cleanup succeeds.
        fs::set_permissions(&src, fs::Permissions::from_mode(0o755)).unwrap();

        assert!(result.is_err(), "should fail when src is unreadable");
        assert!(!dest.exists(), "partial dest must be cleaned up on failure");
    }

    /// When the copy succeeds but deleting `src` fails, `dest` must be intact.
    #[test]
    #[cfg(unix)]
    fn test_copy_then_delete_preserves_dest_when_src_delete_fails() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        // Use a subdirectory as the "parent" so we can chmod just it.
        let parent = tmp.path().join("parent");
        fs::create_dir_all(&parent).unwrap();
        let src = parent.join("src");
        let dest = tmp.path().join("dest");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("file.txt"), "data").unwrap();

        // Remove write permission from parent so fs::remove_dir_all(src) fails.
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o555)).unwrap();

        let result = copy_then_delete(&src, &dest);

        // Restore before assertions so tempdir cleanup succeeds.
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o755)).unwrap();

        assert!(result.is_err(), "should fail when src cannot be deleted");
        assert!(
            dest.exists(),
            "dest should be intact when only the delete step fails"
        );
        assert_eq!(fs::read_to_string(dest.join("file.txt")).unwrap(), "data");
    }

    #[test]
    fn test_restore_clears_partial_workspace_at_dest() {
        // If a workspace directory exists at the destination but has no .wsp.yaml
        // (partial creation), restore should clear it and proceed rather than bailing.
        let tmp = tempfile::tempdir().unwrap();
        let paths = test_paths(tmp.path());

        // Set up a gc'd workspace to restore
        create_workspace(&paths, "partial-restore");
        crate::workspace::remove(&paths, "partial-restore", true).unwrap();
        assert!(!paths.workspaces_dir.join("partial-restore").exists());

        // Simulate a partial workspace at the restore destination
        let dest = paths.workspaces_dir.join("partial-restore");
        fs::create_dir_all(dest.join("some-partial-content")).unwrap();
        // No .wsp.yaml written — this is the partial state

        // restore() should clear the partial dir and succeed
        restore(&paths, "partial-restore").unwrap();

        let restored = paths.workspaces_dir.join("partial-restore");
        assert!(restored.exists(), "workspace should be restored");
        assert!(
            restored.join(crate::workspace::METADATA_FILE).exists(),
            "restored workspace should have .wsp.yaml"
        );
    }

    /// Backdate all gc entries by the given number of days.
    fn backdate_gc_entries(gc_dir: &Path, days: i64) {
        for item in fs::read_dir(gc_dir).unwrap() {
            let path = item.unwrap().path();
            if !path.is_dir() {
                continue;
            }
            let meta_path = path.join(GC_META_FILE);
            if let Ok(data) = fs::read_to_string(&meta_path) {
                let mut entry: GcEntry = serde_yaml_ng::from_str(&data).unwrap();
                entry.trashed_at = Utc::now() - chrono::Duration::days(days);
                fs::write(&meta_path, serde_yaml_ng::to_string(&entry).unwrap()).unwrap();
            }
        }
    }
}
