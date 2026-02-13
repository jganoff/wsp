use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::config::Paths;
use crate::git;
use crate::giturl;
use crate::mirror;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkspaceRepoRef {
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub r#ref: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Metadata {
    pub name: String,
    pub branch: String,
    pub repos: BTreeMap<String, Option<WorkspaceRepoRef>>,
    pub created: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dirs: BTreeMap<String, String>,
}

impl Metadata {
    /// Returns the clone directory name for an identity.
    /// Uses the dirs map if an override exists, otherwise falls back to parsed.repo.
    pub fn dir_name(&self, identity: &str) -> Result<String> {
        if let Some(dir) = self.dirs.get(identity) {
            return Ok(dir.clone());
        }
        let parsed = parse_identity(identity)?;
        Ok(parsed.repo)
    }
}

/// Detects repo-name collisions and returns a dirs map with `owner-repo` entries
/// for all identities that share the same repo short name.
/// Only colliding identities appear in the returned map.
fn compute_dir_names(identities: &[&str]) -> Result<BTreeMap<String, String>> {
    let mut by_repo: BTreeMap<String, Vec<(&str, String)>> = BTreeMap::new();
    for &id in identities {
        let parsed = parse_identity(id)?;
        by_repo
            .entry(parsed.repo.clone())
            .or_default()
            .push((id, parsed.owner.replace('/', "-")));
    }

    let mut dirs = BTreeMap::new();
    for entries in by_repo.values() {
        if entries.len() > 1 {
            for (id, owner) in entries {
                let parsed = parse_identity(id)?;
                dirs.insert(id.to_string(), format!("{}-{}", owner, parsed.repo));
            }
        }
    }
    Ok(dirs)
}

pub const METADATA_FILE: &str = ".wsp.yaml";

pub fn dir(workspaces_dir: &Path, name: &str) -> PathBuf {
    workspaces_dir.join(name)
}

pub fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("workspace name cannot be empty");
    }
    if name.contains('\0') {
        bail!("workspace name cannot contain null bytes");
    }
    if name.contains('/') || name.contains('\\') {
        bail!("workspace name {:?} cannot contain path separators", name);
    }
    if name.starts_with('-') {
        bail!("workspace name {:?} cannot start with a dash", name);
    }
    if name.starts_with('.') {
        bail!("workspace name {:?} cannot start with a dot", name);
    }
    Ok(())
}

pub fn load_metadata(ws_dir: &Path) -> Result<Metadata> {
    let data = fs::read_to_string(ws_dir.join(METADATA_FILE))?;
    let m: Metadata = serde_yaml_ng::from_str(&data)?;
    Ok(m)
}

pub fn save_metadata(ws_dir: &Path, m: &Metadata) -> Result<()> {
    let data = serde_yaml_ng::to_string(m)?;
    let mut tmp =
        tempfile::NamedTempFile::new_in(ws_dir).context("creating temp file for atomic save")?;
    tmp.write_all(data.as_bytes())
        .context("writing metadata to temp file")?;
    tmp.persist(ws_dir.join(METADATA_FILE))
        .context("renaming temp file to metadata")?;
    Ok(())
}

pub fn detect(start_dir: &Path) -> Result<PathBuf> {
    let mut dir = start_dir.to_path_buf();
    loop {
        if dir.join(METADATA_FILE).exists() {
            return Ok(dir);
        }
        match dir.parent() {
            Some(parent) if parent != dir => {
                dir = parent.to_path_buf();
            }
            _ => bail!("not in a workspace (no {} found)", METADATA_FILE),
        }
    }
}

pub fn create(
    paths: &Paths,
    name: &str,
    repo_refs: &BTreeMap<String, String>,
    branch_prefix: Option<&str>,
    upstream_urls: &BTreeMap<String, String>,
) -> Result<()> {
    validate_name(name)?;

    let ws_dir = dir(&paths.workspaces_dir, name);
    if ws_dir.exists() {
        bail!("workspace {:?} already exists", name);
    }

    fs::create_dir_all(&ws_dir)?;

    let branch = match branch_prefix.filter(|p| !p.is_empty()) {
        Some(prefix) => format!("{}/{}", prefix, name),
        None => name.to_string(),
    };

    match create_inner(
        &paths.mirrors_dir,
        &branch,
        &ws_dir,
        name,
        repo_refs,
        upstream_urls,
    ) {
        Ok(()) => Ok(()),
        Err(e) => {
            // Clean up workspace dir on failure (best-effort)
            let _ = fs::remove_dir_all(&ws_dir);
            Err(e)
        }
    }
}

fn create_inner(
    mirrors_dir: &Path,
    branch: &str,
    ws_dir: &Path,
    name: &str,
    repo_refs: &BTreeMap<String, String>,
    upstream_urls: &BTreeMap<String, String>,
) -> Result<()> {
    let mut repos: BTreeMap<String, Option<WorkspaceRepoRef>> = BTreeMap::new();
    for (identity, r) in repo_refs {
        if r.is_empty() {
            repos.insert(identity.clone(), None);
        } else {
            repos.insert(
                identity.clone(),
                Some(WorkspaceRepoRef { r#ref: r.clone() }),
            );
        }
    }

    let identities: Vec<&str> = repo_refs.keys().map(|s| s.as_str()).collect();
    let dirs = compute_dir_names(&identities)?;

    let meta = Metadata {
        name: name.to_string(),
        branch: branch.to_string(),
        repos,
        created: Utc::now(),
        dirs: dirs.clone(),
    };

    for (identity, r) in repo_refs {
        let dn = meta.dir_name(identity)?;
        let upstream = upstream_urls
            .get(identity)
            .map(|s| s.as_str())
            .unwrap_or("");
        clone_from_mirror(mirrors_dir, ws_dir, identity, &dn, branch, r, upstream)
            .map_err(|e| anyhow::anyhow!("cloning repo {}: {}", identity, e))?;
    }

    save_metadata(ws_dir, &meta)?;
    Ok(())
}

pub fn add_repos(
    mirrors_dir: &Path,
    ws_dir: &Path,
    repo_refs: &BTreeMap<String, String>,
    upstream_urls: &BTreeMap<String, String>,
) -> Result<()> {
    let mut meta = load_metadata(ws_dir)?;

    for (identity, r) in repo_refs {
        if meta.repos.contains_key(identity) {
            eprintln!("  {} already in workspace, skipping", identity);
            continue;
        }

        let new_parsed = parse_identity(identity)?;
        let new_default_dir = new_parsed.repo.clone();

        // Check for collision with existing repos
        let mut collision_identity: Option<String> = None;
        for existing_id in meta.repos.keys() {
            let existing_dir = meta.dir_name(existing_id)?;
            if existing_dir == new_default_dir {
                collision_identity = Some(existing_id.clone());
                break;
            }
        }

        let upstream = upstream_urls
            .get(identity)
            .map(|s| s.as_str())
            .unwrap_or("");

        if let Some(existing_id) = collision_identity {
            // Rename existing clone directory to owner-repo
            let existing_parsed = parse_identity(&existing_id)?;
            let old_dir = meta.dir_name(&existing_id)?;
            let new_existing_dir = format!(
                "{}-{}",
                existing_parsed.owner.replace('/', "-"),
                existing_parsed.repo
            );
            fs::rename(ws_dir.join(&old_dir), ws_dir.join(&new_existing_dir))
                .map_err(|e| anyhow::anyhow!("renaming directory for {}: {}", existing_id, e))?;
            meta.dirs.insert(existing_id.clone(), new_existing_dir);

            // Create new clone as owner-repo
            let new_dir = format!("{}-{}", new_parsed.owner.replace('/', "-"), new_parsed.repo);
            clone_from_mirror(
                mirrors_dir,
                ws_dir,
                identity,
                &new_dir,
                &meta.branch,
                r,
                upstream,
            )
            .map_err(|e| anyhow::anyhow!("cloning repo {}: {}", identity, e))?;
            meta.dirs.insert(identity.clone(), new_dir);
        } else {
            let dn = meta.dir_name(identity)?;
            clone_from_mirror(
                mirrors_dir,
                ws_dir,
                identity,
                &dn,
                &meta.branch,
                r,
                upstream,
            )
            .map_err(|e| anyhow::anyhow!("cloning repo {}: {}", identity, e))?;
        }

        if r.is_empty() {
            meta.repos.insert(identity.clone(), None);
        } else {
            meta.repos.insert(
                identity.clone(),
                Some(WorkspaceRepoRef { r#ref: r.clone() }),
            );
        }
    }

    save_metadata(ws_dir, &meta)
}

pub fn remove_repos(ws_dir: &Path, identities_to_remove: &[String], force: bool) -> Result<()> {
    let mut meta = load_metadata(ws_dir)?;

    // Validate all identities exist in the workspace
    for identity in identities_to_remove {
        if !meta.repos.contains_key(identity) {
            bail!("repo {} is not in this workspace", identity);
        }
    }

    // Safety check: for active repos, check pending changes + unmerged branches
    if !force {
        let mut problems: Vec<String> = Vec::new();
        for identity in identities_to_remove {
            let entry = &meta.repos[identity];
            let is_active = match entry {
                None => true,
                Some(re) => re.r#ref.is_empty(),
            };
            if !is_active {
                continue;
            }

            let dn = meta.dir_name(identity)?;
            let clone_dir = ws_dir.join(&dn);

            let changed = git::changed_file_count(&clone_dir).unwrap_or(0);
            let ahead = git::ahead_count(&clone_dir).unwrap_or(0);
            if changed > 0 || ahead > 0 {
                problems.push(format!("{} (pending changes)", identity));
                continue;
            }

            // Fetch origin in the clone for up-to-date merge detection
            let _ = git::fetch_remote(&clone_dir, "origin");

            if git::branch_exists(&clone_dir, &meta.branch) {
                let default_branch = git::default_branch_for_remote(&clone_dir, "origin")
                    .or_else(|_| git::default_branch(&clone_dir))
                    .unwrap_or_default();
                if !default_branch.is_empty() {
                    let merge_target = format!("origin/{}", default_branch);
                    let target = if git::ref_exists(&clone_dir, &merge_target) {
                        merge_target
                    } else {
                        default_branch
                    };
                    match git::branch_safety(&clone_dir, &meta.branch, &target) {
                        git::BranchSafety::Merged | git::BranchSafety::SquashMerged => {}
                        git::BranchSafety::PushedToRemote => {
                            problems.push(format!(
                                "{} (unmerged branch, but pushed to remote)",
                                identity
                            ));
                        }
                        git::BranchSafety::Unmerged => {
                            problems.push(format!("{} (unmerged branch)", identity));
                        }
                    }
                }
            }
        }

        if !problems.is_empty() {
            let mut list = String::new();
            for p in &problems {
                list.push_str(&format!("\n  - {}", p));
            }
            bail!(
                "cannot remove repos:{}\n\nUse --force to remove anyway",
                list
            );
        }
    }

    // Remove clone directories
    for identity in identities_to_remove {
        let dn = meta.dir_name(identity)?;
        let clone_path = ws_dir.join(&dn);

        if let Err(e) = fs::remove_dir_all(&clone_path) {
            eprintln!("  warning: removing clone for {}: {}", identity, e);
        }

        meta.repos.remove(identity);
        meta.dirs.remove(identity);
    }

    // Recalculate dir names for remaining repos
    let remaining_ids: Vec<&str> = meta.repos.keys().map(|s| s.as_str()).collect();
    let new_dirs = compute_dir_names(&remaining_ids)?;

    // Check if any collision disambiguations can be undone
    for (identity, new_dir) in &new_dirs {
        if let Some(old_dir) = meta.dirs.get(identity)
            && old_dir != new_dir
            && let Err(e) = fs::rename(ws_dir.join(old_dir), ws_dir.join(new_dir))
        {
            eprintln!("  warning: renaming directory for {}: {}", identity, e);
        }
    }

    // Check if repos that were disambiguated can now use their short name
    for identity in meta.repos.keys() {
        if let Some(old_dir) = meta.dirs.get(identity).cloned()
            && !new_dirs.contains_key(identity)
        {
            let parsed = parse_identity(identity)?;
            let short_name = parsed.repo.clone();
            if let Err(e) = fs::rename(ws_dir.join(&old_dir), ws_dir.join(&short_name)) {
                eprintln!("  warning: renaming directory for {}: {}", identity, e);
            }
        }
    }

    // Update dirs map
    meta.dirs = new_dirs;

    save_metadata(ws_dir, &meta)
}

/// Fetch wsp-mirror in each clone (parallel, best-effort).
/// Propagates refs fetched into mirrors down to workspace clones.
pub fn propagate_mirror_to_clones(ws_dir: &Path, meta: &Metadata) {
    let clones: Vec<(String, PathBuf)> = meta
        .repos
        .keys()
        .filter_map(|id| {
            meta.dir_name(id)
                .ok()
                .map(|dn| (id.clone(), ws_dir.join(dn)))
        })
        .collect();

    if clones.is_empty() {
        return;
    }

    std::thread::scope(|s| {
        let handles: Vec<_> = clones
            .iter()
            .map(|(id, clone_dir)| {
                s.spawn(move || {
                    if let Err(e) = git::fetch_remote(clone_dir, "wsp-mirror") {
                        eprintln!("  warning: propagate wsp-mirror for {}: {}", id, e);
                    }
                })
            })
            .collect();
        for h in handles {
            let _ = h.join();
        }
    });
}

pub fn has_pending_changes(ws_dir: &Path) -> Result<Vec<String>> {
    let meta = load_metadata(ws_dir)?;
    let mut dirty = Vec::new();

    for identity in meta.repos.keys() {
        let dn = match meta.dir_name(identity) {
            Ok(d) => d,
            Err(_) => continue,
        };
        let repo_dir = ws_dir.join(&dn);

        let changed = git::changed_file_count(&repo_dir).unwrap_or(0);
        let ahead = git::ahead_count(&repo_dir).unwrap_or(0);

        if changed > 0 || ahead > 0 {
            dirty.push(dn);
        }
    }

    Ok(dirty)
}

pub fn remove(paths: &Paths, name: &str, force: bool) -> Result<()> {
    let ws_dir = dir(&paths.workspaces_dir, name);
    let meta =
        load_metadata(&ws_dir).map_err(|e| anyhow::anyhow!("reading workspace metadata: {}", e))?;

    struct ActiveRepo {
        identity: String,
        dir_name: String,
        fetch_failed: bool,
    }

    let mut active_repos: Vec<ActiveRepo> = Vec::new();

    for (identity, entry) in &meta.repos {
        let dn = meta.dir_name(identity)?;

        let is_active = match entry {
            None => true,
            Some(re) => re.r#ref.is_empty(),
        };

        if is_active {
            let clone_dir = ws_dir.join(&dn);
            // Best-effort fetch origin to detect remote merges
            let fetch_failed = git::fetch_remote(&clone_dir, "origin").is_err();
            if fetch_failed {
                eprintln!("  warning: fetch failed for {}, using local data", identity);
            }
            active_repos.push(ActiveRepo {
                identity: identity.clone(),
                dir_name: dn,
                fetch_failed,
            });
        }
    }

    // Pre-flight: check if all active branches are merged (on clone, not mirror)
    if !force {
        let mut unmerged: Vec<(String, bool)> = Vec::new();
        for ar in &active_repos {
            let clone_dir = ws_dir.join(&ar.dir_name);
            if !git::branch_exists(&clone_dir, &meta.branch) {
                continue;
            }
            let default_branch = match git::default_branch_for_remote(&clone_dir, "origin") {
                Ok(b) => b,
                Err(_) => match git::default_branch(&clone_dir) {
                    Ok(b) => b,
                    Err(e) => {
                        eprintln!(
                            "  warning: cannot detect default branch for {}: {}",
                            ar.identity, e
                        );
                        continue;
                    }
                },
            };
            let merge_target = format!("origin/{}", default_branch);
            let target = if git::ref_exists(&clone_dir, &merge_target) {
                merge_target
            } else {
                default_branch
            };
            match git::branch_safety(&clone_dir, &meta.branch, &target) {
                git::BranchSafety::Merged | git::BranchSafety::SquashMerged => {}
                git::BranchSafety::PushedToRemote => {
                    unmerged.push((
                        format!("{} (unmerged, but pushed to remote)", ar.identity),
                        ar.fetch_failed,
                    ));
                }
                git::BranchSafety::Unmerged => {
                    unmerged.push((ar.identity.clone(), ar.fetch_failed));
                }
            }
        }

        if !unmerged.is_empty() {
            let mut list = String::new();
            let mut any_fetch_failed = false;
            for (repo, fetch_failed) in &unmerged {
                list.push_str(&format!("\n  - {}", repo));
                if *fetch_failed {
                    list.push_str(" (fetch failed, local data may be stale)");
                    any_fetch_failed = true;
                }
            }
            let mut msg = format!(
                "workspace {:?} has unmerged branches ({}):{}\n\nUse --force to remove anyway",
                name, meta.branch, list
            );
            if any_fetch_failed {
                msg.push_str(
                    "\n\nNote: some fetches failed; the branch may already be merged remotely",
                );
            }
            bail!("{}", msg);
        }
    }

    fs::remove_dir_all(&ws_dir)?;
    Ok(())
}

pub fn list_all(workspaces_dir: &Path) -> Result<Vec<String>> {
    if !workspaces_dir.exists() {
        return Ok(Vec::new());
    }

    let mut names = Vec::new();
    for entry in fs::read_dir(workspaces_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let meta_path = entry.path().join(METADATA_FILE);
        if meta_path.exists()
            && let Some(name) = entry.file_name().to_str()
        {
            names.push(name.to_string());
        }
    }
    names.sort();
    Ok(names)
}

fn clone_from_mirror(
    mirrors_dir: &Path,
    ws_dir: &Path,
    identity: &str,
    dir_name: &str,
    branch: &str,
    git_ref: &str,
    upstream_url: &str,
) -> Result<()> {
    let parsed = parse_identity(identity)?;
    let mirror_dir = mirror::dir(mirrors_dir, &parsed);
    let dest = ws_dir.join(dir_name);

    // 1. Clone from mirror (hardlinks, creates wsp-mirror remote)
    git::clone_local(&mirror_dir, &dest)?;

    // 2. Configure wsp-mirror to fetch from mirror's refs/remotes/origin/*
    git::configure_wsp_mirror_refspec(&dest)?;
    git::fetch_remote(&dest, "wsp-mirror")?;

    // 3. Set origin to real upstream URL
    if !upstream_url.is_empty() {
        git::remote_set_origin(&dest, upstream_url)?;
    }

    // 4. Copy default branch info from wsp-mirror to origin
    if let Ok(default_br) = git::default_branch_for_remote(&dest, "wsp-mirror") {
        let _ = git::remote_set_head(&dest, "origin", &default_br);
    }

    // 4b. Fetch origin so remote tracking branches (origin/main etc.) exist
    if !upstream_url.is_empty() {
        git::fetch_remote(&dest, "origin")?;
    }

    // 5. Checkout the right ref/branch
    // Context repo: check out at the specified ref
    if !git_ref.is_empty() {
        let ws_mirror_ref = format!("wsp-mirror/{}", git_ref);
        if git::branch_exists(&dest, git_ref) {
            // Local branch already exists
            git::checkout(&dest, git_ref)?;
        } else if git::ref_exists(&dest, &format!("refs/remotes/wsp-mirror/{}", git_ref)) {
            // Create branch from wsp-mirror/<ref>, track origin/<ref>
            git::checkout_new_branch(&dest, git_ref, &ws_mirror_ref)?;
            let origin_ref = format!("origin/{}", git_ref);
            if git::ref_exists(&dest, &format!("refs/remotes/origin/{}", git_ref)) {
                git::set_upstream(&dest, &origin_ref)?;
            }
        } else {
            // Tag or SHA: detached HEAD
            git::checkout_detached(&dest, git_ref)?;
        }
        return Ok(());
    }

    // Active repo: create/checkout workspace branch
    if git::branch_exists(&dest, branch) {
        git::checkout(&dest, branch)?;
        return Ok(());
    }

    let default_branch = git::default_branch_for_remote(&dest, "wsp-mirror")?;
    let start_point = format!("wsp-mirror/{}", default_branch);
    git::checkout_new_branch(&dest, branch, &start_point)?;

    // Track origin/<default_branch> so ahead/behind info is meaningful
    let origin_ref = format!("origin/{}", default_branch);
    if git::ref_exists(&dest, &format!("refs/remotes/origin/{}", default_branch)) {
        let _ = git::set_upstream(&dest, &origin_ref);
    }

    Ok(())
}

fn parse_identity(identity: &str) -> Result<giturl::Parsed> {
    giturl::Parsed::from_identity(identity)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    /// Sets up a test environment using tempdirs.
    /// Returns Paths, TempDirs (keep alive!), identity, and upstream URL map.
    fn setup_test_env() -> (
        Paths,
        tempfile::TempDir,
        tempfile::TempDir,
        String,
        BTreeMap<String, String>,
    ) {
        let tmp_data = tempfile::tempdir().unwrap();
        let tmp_home = tempfile::tempdir().unwrap();

        let data_dir = tmp_data.path().join("wsp");
        let workspaces_dir = tmp_home.path().join("dev").join("workspaces");
        fs::create_dir_all(&workspaces_dir).unwrap();

        let paths = Paths::from_dirs(&data_dir, &workspaces_dir);

        // Create a source repo
        let repo_dir = tempfile::tempdir().unwrap();
        let cmds: Vec<Vec<&str>> = vec![
            vec!["git", "init", "--initial-branch=main"],
            vec!["git", "config", "user.email", "test@test.com"],
            vec!["git", "config", "user.name", "Test"],
            vec!["git", "config", "commit.gpgsign", "false"],
            vec!["git", "commit", "--allow-empty", "-m", "initial"],
        ];
        for args in &cmds {
            let output = Command::new(args[0])
                .args(&args[1..])
                .current_dir(repo_dir.path())
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "command {:?} failed: {}",
                args,
                String::from_utf8_lossy(&output.stderr)
            );
        }

        // Bare clone into mirrors
        let parsed = giturl::Parsed {
            host: "test.local".into(),
            owner: "user".into(),
            repo: "test-repo".into(),
        };
        mirror::clone(
            &paths.mirrors_dir,
            &parsed,
            repo_dir.path().to_str().unwrap(),
        )
        .unwrap();

        // Set up HEAD ref so DefaultBranch works
        let mirror_dir = mirror::dir(&paths.mirrors_dir, &parsed);
        let output = Command::new("git")
            .args([
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/heads/main",
            ])
            .current_dir(&mirror_dir)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "setting HEAD ref: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let identity = parsed.identity();
        let upstream_urls = BTreeMap::from([(
            identity.clone(),
            repo_dir.path().to_str().unwrap().to_string(),
        )]);

        (paths, tmp_data, repo_dir, identity, upstream_urls)
    }

    #[test]
    fn test_create_and_load_metadata() {
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(&paths, "test-ws", &refs, None, &upstream_urls).unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "test-ws");
        let meta = load_metadata(&ws_dir).unwrap();

        assert_eq!(meta.name, "test-ws");
        assert_eq!(meta.branch, "test-ws");
        assert!(meta.repos.contains_key(&identity));

        // Clone directory should exist and be a regular git repo
        let clone_dir = ws_dir.join("test-repo");
        assert!(clone_dir.exists());
        assert!(
            clone_dir.join(".git").is_dir(),
            ".git should be a directory, not a worktree file"
        );
    }

    #[test]
    fn test_create_with_branch_prefix() {
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(&paths, "my-feature", &refs, Some("jganoff"), &upstream_urls).unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "my-feature");
        let meta = load_metadata(&ws_dir).unwrap();

        assert_eq!(meta.name, "my-feature");
        assert_eq!(meta.branch, "jganoff/my-feature");
        assert!(meta.repos.contains_key(&identity));
        assert!(ws_dir.join("test-repo").exists());
    }

    #[test]
    fn test_create_with_empty_branch_prefix() {
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(&paths, "empty-prefix", &refs, Some(""), &upstream_urls).unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "empty-prefix");
        let meta = load_metadata(&ws_dir).unwrap();

        assert_eq!(meta.branch, "empty-prefix");
    }

    #[test]
    fn test_create_duplicate() {
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(&paths, "test-ws-dup", &refs, None, &upstream_urls).unwrap();
        assert!(create(&paths, "test-ws-dup", &refs, None, &upstream_urls).is_err());
    }

    #[test]
    fn test_detect() {
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity, String::new())]);
        create(&paths, "test-ws-detect", &refs, None, &upstream_urls).unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "test-ws-detect");

        // From workspace root
        let found = detect(&ws_dir).unwrap();
        assert_eq!(found, ws_dir);

        // From a repo subdirectory
        let repo_dir = ws_dir.join("test-repo");
        let found = detect(&repo_dir).unwrap();
        assert_eq!(found, ws_dir);
    }

    #[test]
    fn test_detect_not_in_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(detect(tmp.path()).is_err());
    }

    #[test]
    fn test_remove_merged_workspace() {
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(&paths, "rm-merged", &refs, None, &upstream_urls).unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "rm-merged");
        assert!(ws_dir.exists());

        // Branch was created from main with no extra commits, so it's merged
        remove(&paths, "rm-merged", false).unwrap();
        assert!(!ws_dir.exists());
    }

    #[test]
    fn test_remove_merged_when_origin_ahead_of_local_main() {
        let (paths, _d, source_repo, identity, upstream_urls) = setup_test_env();

        let parsed = parse_identity(&identity).unwrap();
        let mirror_dir = mirror::dir(&paths.mirrors_dir, &parsed);

        // Advance the source repo so origin/main moves ahead
        let cmds: Vec<Vec<&str>> = vec![vec![
            "git",
            "commit",
            "--allow-empty",
            "-m",
            "upstream advance",
        ]];
        for args in &cmds {
            let output = Command::new(args[0])
                .args(&args[1..])
                .current_dir(source_repo.path())
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "command {:?} failed: {}",
                args,
                String::from_utf8_lossy(&output.stderr)
            );
        }

        // Fetch to update mirror
        git::fetch(&mirror_dir, true).unwrap();

        // Create workspace
        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(&paths, "rm-origin-ahead", &refs, None, &upstream_urls).unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "rm-origin-ahead");
        assert!(ws_dir.exists());

        // Remove should succeed — the workspace branch has no extra commits
        remove(&paths, "rm-origin-ahead", false).unwrap();
        assert!(!ws_dir.exists());
    }

    #[test]
    fn test_remove_blocks_unmerged_branch() {
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(&paths, "rm-unmerged", &refs, None, &upstream_urls).unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "rm-unmerged");
        let repo_dir = ws_dir.join("test-repo");

        // Add a commit to the workspace branch so it diverges from main
        let cmds: Vec<Vec<&str>> = vec![
            vec!["git", "config", "user.email", "test@test.com"],
            vec!["git", "config", "user.name", "Test"],
            vec!["git", "config", "commit.gpgsign", "false"],
            vec!["git", "commit", "--allow-empty", "-m", "diverge"],
        ];
        for args in &cmds {
            let output = Command::new(args[0])
                .args(&args[1..])
                .current_dir(&repo_dir)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "command {:?} failed: {}",
                args,
                String::from_utf8_lossy(&output.stderr)
            );
        }

        let result = remove(&paths, "rm-unmerged", false);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("unmerged branches"),
            "expected 'unmerged branches' in error: {}",
            err
        );

        // Workspace should still exist
        assert!(ws_dir.exists());
    }

    #[test]
    fn test_remove_force_deletes_unmerged() {
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(&paths, "rm-force", &refs, None, &upstream_urls).unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "rm-force");
        let repo_dir = ws_dir.join("test-repo");

        // Add a commit to the workspace branch so it diverges from main
        let cmds: Vec<Vec<&str>> = vec![
            vec!["git", "config", "user.email", "test@test.com"],
            vec!["git", "config", "user.name", "Test"],
            vec!["git", "config", "commit.gpgsign", "false"],
            vec!["git", "commit", "--allow-empty", "-m", "diverge"],
        ];
        for args in &cmds {
            let output = Command::new(args[0])
                .args(&args[1..])
                .current_dir(&repo_dir)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "command {:?} failed: {}",
                args,
                String::from_utf8_lossy(&output.stderr)
            );
        }

        // Force remove should succeed despite unmerged branch
        remove(&paths, "rm-force", true).unwrap();
        assert!(!ws_dir.exists());
    }

    #[test]
    fn test_list_all() {
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();

        // Initially empty
        let names = list_all(&paths.workspaces_dir).unwrap();
        assert!(names.is_empty());

        // Create a workspace
        let refs = BTreeMap::from([(identity, String::new())]);
        create(&paths, "ws-1-list", &refs, None, &upstream_urls).unwrap();

        let names = list_all(&paths.workspaces_dir).unwrap();
        assert_eq!(names, vec!["ws-1-list"]);
    }

    #[test]
    fn test_save_and_load_metadata_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let meta = Metadata {
            name: "my-ws".into(),
            branch: "my-ws".into(),
            repos: BTreeMap::from([
                ("github.com/user/repo-a".into(), None),
                ("github.com/user/repo-b".into(), None),
            ]),
            created: Utc::now(),
            dirs: BTreeMap::new(),
        };

        save_metadata(tmp.path(), &meta).unwrap();
        let loaded = load_metadata(tmp.path()).unwrap();

        assert_eq!(loaded.name, meta.name);
        assert_eq!(loaded.branch, meta.branch);
        assert_eq!(loaded.repos.len(), meta.repos.len());
        for k in meta.repos.keys() {
            assert!(loaded.repos.contains_key(k));
        }
    }

    #[test]
    fn test_save_and_load_metadata_round_trip_with_refs() {
        let tmp = tempfile::tempdir().unwrap();
        let meta = Metadata {
            name: "my-ws".into(),
            branch: "my-ws".into(),
            repos: BTreeMap::from([
                ("github.com/acme/api-gateway".into(), None),
                (
                    "github.com/acme/user-service".into(),
                    Some(WorkspaceRepoRef {
                        r#ref: "main".into(),
                    }),
                ),
                (
                    "github.com/acme/proto".into(),
                    Some(WorkspaceRepoRef {
                        r#ref: "v1.0".into(),
                    }),
                ),
            ]),
            created: Utc::now(),
            dirs: BTreeMap::new(),
        };

        save_metadata(tmp.path(), &meta).unwrap();
        let loaded = load_metadata(tmp.path()).unwrap();

        assert_eq!(loaded.name, meta.name);
        assert_eq!(loaded.repos.len(), 3);
        assert!(loaded.repos["github.com/acme/api-gateway"].is_none());
        assert_eq!(
            loaded.repos["github.com/acme/user-service"]
                .as_ref()
                .unwrap()
                .r#ref,
            "main"
        );
        assert_eq!(
            loaded.repos["github.com/acme/proto"]
                .as_ref()
                .unwrap()
                .r#ref,
            "v1.0"
        );
    }

    #[test]
    fn test_validate_name() {
        let cases = vec![
            ("valid", "my-feature", false),
            ("valid with dots", "fix.bug", false),
            ("empty", "", true),
            ("forward slash", "a/b", true),
            ("backslash", "a\\b", true),
            ("dash prefix", "-bad", true),
            ("double dash prefix", "--also-bad", true),
            ("dot", ".", true),
            ("dotdot", "..", true),
            ("dot prefix", ".hidden", true),
            ("dot prefix config", ".config", true),
            ("null byte", "bad\0name", true),
        ];
        for (name, input, want_err) in cases {
            let result = validate_name(input);
            if want_err {
                assert!(result.is_err(), "{}: expected error", name);
            } else {
                assert!(result.is_ok(), "{}: unexpected error: {:?}", name, result);
            }
        }
    }

    #[test]
    fn test_create_cleans_up_on_failure() {
        let tmp_data = tempfile::tempdir().unwrap();
        let tmp_home = tempfile::tempdir().unwrap();

        let data_dir = tmp_data.path().join("wsp");
        let workspaces_dir = tmp_home.path().join("dev").join("workspaces");
        fs::create_dir_all(&workspaces_dir).unwrap();

        let paths = Paths::from_dirs(&data_dir, &workspaces_dir);

        // Try to create with a nonexistent repo identity — will fail
        let refs = BTreeMap::from([("nonexistent.local/user/nope".into(), String::new())]);
        let upstream_urls = BTreeMap::new();
        let result = create(&paths, "fail-ws", &refs, None, &upstream_urls);
        assert!(result.is_err());

        // Workspace dir should have been cleaned up
        let ws_dir = workspaces_dir.join("fail-ws");
        assert!(
            !ws_dir.exists(),
            "workspace dir should be cleaned up on failure"
        );
    }

    #[test]
    fn test_create_with_context_repo() {
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();

        // Create workspace with the repo as context (ref = "main")
        let refs = BTreeMap::from([(identity.clone(), "main".into())]);
        create(&paths, "ctx-ws", &refs, None, &upstream_urls).unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "ctx-ws");
        let meta = load_metadata(&ws_dir).unwrap();

        assert!(meta.repos[&identity].is_some());
        assert_eq!(meta.repos[&identity].as_ref().unwrap().r#ref, "main");
        assert!(ws_dir.join("test-repo").exists());
    }

    #[test]
    fn test_add_repos_to_existing_workspace() {
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();

        // Create workspace with active repo
        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(&paths, "add-ws", &refs, None, &upstream_urls).unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "add-ws");

        // Try adding the same repo again — should skip
        add_repos(&paths.mirrors_dir, &ws_dir, &refs, &upstream_urls).unwrap();

        let meta = load_metadata(&ws_dir).unwrap();
        assert_eq!(meta.repos.len(), 1);
    }

    #[test]
    fn test_has_pending_changes_clean() {
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity, String::new())]);
        create(&paths, "pending-clean", &refs, None, &upstream_urls).unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "pending-clean");
        let dirty = has_pending_changes(&ws_dir).unwrap();
        assert!(dirty.is_empty());
    }

    #[test]
    fn test_has_pending_changes_uncommitted() {
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity, String::new())]);
        create(&paths, "pending-dirty", &refs, None, &upstream_urls).unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "pending-dirty");
        let repo_dir = ws_dir.join("test-repo");
        fs::write(repo_dir.join("dirty.txt"), "x").unwrap();

        let dirty = has_pending_changes(&ws_dir).unwrap();
        assert!(dirty.contains(&"test-repo".to_string()));
    }

    #[test]
    fn test_remove_context_repo() {
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();

        // Create workspace with context repo (pinned to "main")
        let refs = BTreeMap::from([(identity, "main".into())]);
        create(&paths, "rm-ws-ctx", &refs, None, &upstream_urls).unwrap();

        // Remove should succeed without touching context repo branches
        remove(&paths, "rm-ws-ctx", false).unwrap();
    }

    /// Creates a second mirror with a different owner but same repo name.
    /// Returns (identity, upstream_urls entry).
    fn add_mirror_with_owner(
        paths: &Paths,
        source_repo: &Path,
        host: &str,
        owner: &str,
        repo: &str,
    ) -> (String, BTreeMap<String, String>) {
        let parsed = giturl::Parsed {
            host: host.into(),
            owner: owner.into(),
            repo: repo.into(),
        };
        mirror::clone(&paths.mirrors_dir, &parsed, source_repo.to_str().unwrap()).unwrap();

        let mirror_dir = mirror::dir(&paths.mirrors_dir, &parsed);
        let output = Command::new("git")
            .args([
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/heads/main",
            ])
            .current_dir(&mirror_dir)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "setting HEAD ref: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let id = parsed.identity();
        let urls = BTreeMap::from([(id.clone(), source_repo.to_str().unwrap().to_string())]);
        (id, urls)
    }

    #[test]
    fn test_compute_dir_names_no_collision() {
        let ids = vec!["github.com/acme/api", "github.com/acme/web"];
        let dirs = compute_dir_names(&ids).unwrap();
        assert!(dirs.is_empty(), "no collision means empty map");
    }

    #[test]
    fn test_compute_dir_names_with_collision() {
        let ids = vec!["github.com/acme/utils", "github.com/other/utils"];
        let dirs = compute_dir_names(&ids).unwrap();
        assert_eq!(dirs.len(), 2);
        assert_eq!(dirs["github.com/acme/utils"], "acme-utils");
        assert_eq!(dirs["github.com/other/utils"], "other-utils");
    }

    #[test]
    fn test_compute_dir_names_nested_owner() {
        let ids = vec!["gitlab.com/org/sub/utils", "gitlab.com/other/utils"];
        let dirs = compute_dir_names(&ids).unwrap();
        assert_eq!(dirs.len(), 2);
        assert_eq!(dirs["gitlab.com/org/sub/utils"], "org-sub-utils");
        assert_eq!(dirs["gitlab.com/other/utils"], "other-utils");
    }

    #[test]
    fn test_dir_name_with_override() {
        let meta = Metadata {
            name: "test".into(),
            branch: "test".into(),
            repos: BTreeMap::from([("github.com/acme/utils".into(), None)]),
            created: Utc::now(),
            dirs: BTreeMap::from([("github.com/acme/utils".into(), "acme-utils".into())]),
        };
        assert_eq!(
            meta.dir_name("github.com/acme/utils").unwrap(),
            "acme-utils"
        );
    }

    #[test]
    fn test_dir_name_without_override() {
        let meta = Metadata {
            name: "test".into(),
            branch: "test".into(),
            repos: BTreeMap::from([("github.com/acme/utils".into(), None)]),
            created: Utc::now(),
            dirs: BTreeMap::new(),
        };
        assert_eq!(meta.dir_name("github.com/acme/utils").unwrap(), "utils");
    }

    #[test]
    fn test_backward_compat_no_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let yaml = "name: old-ws\nbranch: old-ws\nrepos:\n  github.com/acme/api:\ncreated: '2024-01-01T00:00:00Z'\n";
        fs::write(tmp.path().join(METADATA_FILE), yaml).unwrap();

        let meta = load_metadata(tmp.path()).unwrap();
        assert_eq!(meta.name, "old-ws");
        assert!(meta.dirs.is_empty());
        assert_eq!(meta.dir_name("github.com/acme/api").unwrap(), "api");
    }

    #[test]
    fn test_create_with_colliding_repo_names() {
        let (paths, _d, source_repo, identity1, mut upstream_urls) = setup_test_env();

        let (identity2, urls2) = add_mirror_with_owner(
            &paths,
            source_repo.path(),
            "test.local",
            "other",
            "test-repo",
        );
        upstream_urls.extend(urls2);

        let refs = BTreeMap::from([
            (identity1.clone(), String::new()),
            (identity2.clone(), String::new()),
        ]);
        create(&paths, "collide-ws", &refs, None, &upstream_urls).unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "collide-ws");
        let meta = load_metadata(&ws_dir).unwrap();

        assert_eq!(meta.dir_name(&identity1).unwrap(), "user-test-repo");
        assert_eq!(meta.dir_name(&identity2).unwrap(), "other-test-repo");
        assert!(ws_dir.join("user-test-repo").exists());
        assert!(ws_dir.join("other-test-repo").exists());
    }

    #[test]
    fn test_add_repo_causing_collision() {
        let (paths, _d, source_repo, identity1, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity1.clone(), String::new())]);
        create(&paths, "add-collide", &refs, None, &upstream_urls).unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "add-collide");
        assert!(ws_dir.join("test-repo").exists());

        let (identity2, urls2) = add_mirror_with_owner(
            &paths,
            source_repo.path(),
            "test.local",
            "other",
            "test-repo",
        );
        let new_refs = BTreeMap::from([(identity2.clone(), String::new())]);
        add_repos(&paths.mirrors_dir, &ws_dir, &new_refs, &urls2).unwrap();

        let meta = load_metadata(&ws_dir).unwrap();
        assert_eq!(meta.dir_name(&identity1).unwrap(), "user-test-repo");
        assert_eq!(meta.dir_name(&identity2).unwrap(), "other-test-repo");
        assert!(!ws_dir.join("test-repo").exists());
        assert!(ws_dir.join("user-test-repo").exists());
        assert!(ws_dir.join("other-test-repo").exists());
    }

    #[test]
    fn test_remove_repos_basic() {
        let (paths, _d, source_repo, identity1, mut upstream_urls) = setup_test_env();

        let (identity2, urls2) = add_mirror_with_owner(
            &paths,
            source_repo.path(),
            "test.local",
            "other",
            "other-repo",
        );
        upstream_urls.extend(urls2);

        let refs = BTreeMap::from([
            (identity1.clone(), String::new()),
            (identity2.clone(), String::new()),
        ]);
        create(&paths, "rm-repo-ws", &refs, None, &upstream_urls).unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "rm-repo-ws");
        assert!(ws_dir.join("test-repo").exists());
        assert!(ws_dir.join("other-repo").exists());

        remove_repos(&ws_dir, &[identity2.clone()], false).unwrap();

        let meta = load_metadata(&ws_dir).unwrap();
        assert_eq!(meta.repos.len(), 1);
        assert!(meta.repos.contains_key(&identity1));
        assert!(!meta.repos.contains_key(&identity2));
        assert!(ws_dir.join("test-repo").exists());
        assert!(!ws_dir.join("other-repo").exists());
    }

    #[test]
    fn test_remove_repos_not_in_workspace() {
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity, String::new())]);
        create(&paths, "rm-repo-nf", &refs, None, &upstream_urls).unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "rm-repo-nf");
        let result = remove_repos(&ws_dir, &["test.local/nobody/fake".to_string()], false);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("not in this workspace")
        );
    }

    #[test]
    fn test_remove_repos_blocks_pending_changes() {
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(&paths, "rm-repo-dirty", &refs, None, &upstream_urls).unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "rm-repo-dirty");
        let repo_dir = ws_dir.join("test-repo");
        fs::write(repo_dir.join("dirty.txt"), "x").unwrap();

        let result = remove_repos(&ws_dir, &[identity.clone()], false);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("pending changes"));
    }

    #[test]
    fn test_remove_repos_force_with_pending_changes() {
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(&paths, "rm-repo-force", &refs, None, &upstream_urls).unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "rm-repo-force");
        let repo_dir = ws_dir.join("test-repo");
        fs::write(repo_dir.join("dirty.txt"), "x").unwrap();

        remove_repos(&ws_dir, &[identity.clone()], true).unwrap();

        let meta = load_metadata(&ws_dir).unwrap();
        assert!(meta.repos.is_empty());
        assert!(!ws_dir.join("test-repo").exists());
    }

    #[test]
    fn test_remove_repos_undoes_collision() {
        let (paths, _d, source_repo, identity1, mut upstream_urls) = setup_test_env();

        let (identity2, urls2) = add_mirror_with_owner(
            &paths,
            source_repo.path(),
            "test.local",
            "other",
            "test-repo",
        );
        upstream_urls.extend(urls2);

        let refs = BTreeMap::from([
            (identity1.clone(), String::new()),
            (identity2.clone(), String::new()),
        ]);
        create(&paths, "rm-repo-col", &refs, None, &upstream_urls).unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "rm-repo-col");
        assert!(ws_dir.join("user-test-repo").exists());
        assert!(ws_dir.join("other-test-repo").exists());

        remove_repos(&ws_dir, &[identity2.clone()], false).unwrap();

        let meta = load_metadata(&ws_dir).unwrap();
        assert_eq!(meta.repos.len(), 1);
        assert!(meta.dirs.is_empty(), "no collisions, dirs should be empty");
        assert_eq!(meta.dir_name(&identity1).unwrap(), "test-repo");
        assert!(ws_dir.join("test-repo").exists());
        assert!(!ws_dir.join("user-test-repo").exists());
        assert!(!ws_dir.join("other-test-repo").exists());
    }

    #[test]
    fn test_remove_repos_context_repo() {
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity.clone(), "main".into())]);
        create(&paths, "rm-repo-ctx", &refs, None, &upstream_urls).unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "rm-repo-ctx");
        remove_repos(&ws_dir, &[identity.clone()], false).unwrap();

        let meta = load_metadata(&ws_dir).unwrap();
        assert!(meta.repos.is_empty());
    }

    /// Helper: squash-merge a branch into target in the source repo.
    fn squash_merge_branch(dir: &Path, branch: &str, target: &str) {
        for args in &[
            vec!["git", "checkout", target],
            vec!["git", "merge", "--squash", branch],
            vec!["git", "commit", "-m", &format!("squash-merge {}", branch)],
        ] {
            let output = Command::new(args[0])
                .args(&args[1..])
                .current_dir(dir)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{:?}: {}",
                args,
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    /// Helper: commit a file, push to origin, fetch, and set up tracking in a clone.
    fn commit_push_and_track(repo_dir: &Path, branch: &str, file: &str, content: &str) {
        for args in &[
            vec!["git", "config", "user.email", "test@test.com"],
            vec!["git", "config", "user.name", "Test"],
            vec!["git", "config", "commit.gpgsign", "false"],
        ] {
            let output = Command::new(args[0])
                .args(&args[1..])
                .current_dir(repo_dir)
                .output()
                .unwrap();
            assert!(output.status.success());
        }
        fs::write(repo_dir.join(file), content).unwrap();
        let output = Command::new("git")
            .args(["add", file])
            .current_dir(repo_dir)
            .output()
            .unwrap();
        assert!(output.status.success());
        let output = Command::new("git")
            .args(["commit", "-m", &format!("add {}", file)])
            .current_dir(repo_dir)
            .output()
            .unwrap();
        assert!(output.status.success());

        // Push to origin (source repo)
        let output = Command::new("git")
            .args(["push", "origin", branch])
            .current_dir(repo_dir)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "push: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        // Fetch so origin/<branch> appears locally
        let output = Command::new("git")
            .args(["fetch", "origin"])
            .current_dir(repo_dir)
            .output()
            .unwrap();
        assert!(output.status.success());

        // Set tracking so ahead_count returns 0
        let upstream = format!("origin/{}", branch);
        let output = Command::new("git")
            .args(["branch", "--set-upstream-to", &upstream])
            .current_dir(repo_dir)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "set-upstream: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn test_remove_allows_squash_merged_branch() {
        let (paths, _d, source_repo, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(&paths, "rm-squash", &refs, None, &upstream_urls).unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "rm-squash");
        let repo_dir = ws_dir.join("test-repo");

        commit_push_and_track(&repo_dir, "rm-squash", "feat.txt", "feature");
        squash_merge_branch(source_repo.path(), "rm-squash", "main");

        // Remove should succeed without --force since branch is squash-merged
        remove(&paths, "rm-squash", false).unwrap();
        assert!(!ws_dir.exists());
    }

    #[test]
    fn test_remove_blocks_pushed_but_unmerged() {
        let (paths, _d, _source_repo, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(&paths, "rm-pushed", &refs, None, &upstream_urls).unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "rm-pushed");
        let repo_dir = ws_dir.join("test-repo");

        commit_push_and_track(&repo_dir, "rm-pushed", "wip.txt", "wip");

        let result = remove(&paths, "rm-pushed", false);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("pushed to remote"),
            "expected 'pushed to remote' in error: {}",
            err
        );
        assert!(ws_dir.exists());
    }

    #[test]
    fn test_remove_repos_allows_squash_merged() {
        let (paths, _d, source_repo, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(&paths, "rmr-squash", &refs, None, &upstream_urls).unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "rmr-squash");
        let repo_dir = ws_dir.join("test-repo");

        commit_push_and_track(&repo_dir, "rmr-squash", "feat.txt", "feature");
        squash_merge_branch(source_repo.path(), "rmr-squash", "main");

        remove_repos(&ws_dir, &[identity.clone()], false).unwrap();
        let meta = load_metadata(&ws_dir).unwrap();
        assert!(meta.repos.is_empty());
    }

    #[test]
    fn test_remove_repos_blocks_pushed_but_unmerged() {
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(&paths, "rmr-pushed", &refs, None, &upstream_urls).unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "rmr-pushed");
        let repo_dir = ws_dir.join("test-repo");

        commit_push_and_track(&repo_dir, "rmr-pushed", "wip.txt", "wip");

        let result = remove_repos(&ws_dir, &[identity.clone()], false);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("pushed to remote"),
            "expected 'pushed to remote' in error: {}",
            err
        );
    }

    #[test]
    fn test_clone_has_two_remotes() {
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(&paths, "two-remotes", &refs, None, &upstream_urls).unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "two-remotes");
        let clone_dir = ws_dir.join("test-repo");

        // Verify both remotes exist
        let remotes = git::run(Some(&clone_dir), &["remote"]).unwrap();
        assert!(remotes.contains("origin"), "should have origin remote");
        assert!(
            remotes.contains("wsp-mirror"),
            "should have wsp-mirror remote"
        );

        // origin should point to source repo (upstream URL)
        let origin_url = git::run(Some(&clone_dir), &["remote", "get-url", "origin"]).unwrap();
        assert_eq!(origin_url, upstream_urls[&identity]);

        // wsp-mirror should point to the mirror
        let parsed = parse_identity(&identity).unwrap();
        let mirror_dir = mirror::dir(&paths.mirrors_dir, &parsed);
        let ws_mirror_url =
            git::run(Some(&clone_dir), &["remote", "get-url", "wsp-mirror"]).unwrap();
        assert_eq!(
            PathBuf::from(&ws_mirror_url).canonicalize().unwrap(),
            mirror_dir.canonicalize().unwrap()
        );
    }

    #[test]
    fn test_remove_does_not_touch_mirror_branches() {
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(&paths, "rm-no-mirror", &refs, None, &upstream_urls).unwrap();

        // The workspace branch should NOT exist in the mirror (clones are independent)
        let parsed = parse_identity(&identity).unwrap();
        let mirror_dir = mirror::dir(&paths.mirrors_dir, &parsed);

        remove(&paths, "rm-no-mirror", false).unwrap();

        // Mirror should still exist and be intact
        assert!(mirror_dir.exists());
    }

    #[test]
    fn test_propagate_mirror_to_clones() {
        let (paths, _d, source_repo, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(&paths, "prop-ws", &refs, None, &upstream_urls).unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "prop-ws");
        let clone_dir = ws_dir.join("test-repo");

        // Add a commit to source repo on main
        let cmds: Vec<Vec<&str>> = vec![
            vec!["git", "checkout", "main"],
            vec![
                "git",
                "commit",
                "--allow-empty",
                "-m",
                "new upstream commit",
            ],
        ];
        for args in &cmds {
            let output = Command::new(args[0])
                .args(&args[1..])
                .current_dir(source_repo.path())
                .output()
                .unwrap();
            assert!(output.status.success());
        }

        // Fetch mirror to pick up the new commit
        let parsed = parse_identity(&identity).unwrap();
        let mirror_dir = mirror::dir(&paths.mirrors_dir, &parsed);
        git::fetch(&mirror_dir, true).unwrap();

        // Get the new commit sha from mirror
        let mirror_sha = git::run(Some(&mirror_dir), &["rev-parse", "origin/main"]).unwrap();

        // Before propagation, clone doesn't have the new commit
        let clone_sha_before =
            git::run(Some(&clone_dir), &["rev-parse", "wsp-mirror/main"]).unwrap();
        assert_ne!(clone_sha_before, mirror_sha);

        // Propagate
        let meta = load_metadata(&ws_dir).unwrap();
        propagate_mirror_to_clones(&ws_dir, &meta);

        // After propagation, clone should have the new commit
        let clone_sha_after =
            git::run(Some(&clone_dir), &["rev-parse", "wsp-mirror/main"]).unwrap();
        assert_eq!(clone_sha_after, mirror_sha);
    }

    #[test]
    fn test_clone_has_origin_remote_refs() {
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(&paths, "origin-refs", &refs, None, &upstream_urls).unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "origin-refs");
        let clone_dir = ws_dir.join("test-repo");

        // origin/main should exist after clone setup
        assert!(
            git::ref_exists(&clone_dir, "refs/remotes/origin/main"),
            "origin/main should exist after ws new"
        );
    }

    #[test]
    fn test_remove_detects_diverged_squash_merge() {
        let (paths, _d, source_repo, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(&paths, "rm-div-squash", &refs, None, &upstream_urls).unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "rm-div-squash");
        let repo_dir = ws_dir.join("test-repo");

        // Commit and push on the workspace branch
        commit_push_and_track(&repo_dir, "rm-div-squash", "feat.txt", "feature content");

        // Add diverging commits to main on the source repo (different files)
        let out = Command::new("git")
            .args(["checkout", "main"])
            .current_dir(source_repo.path())
            .output()
            .unwrap();
        assert!(out.status.success());
        for args in &[
            vec!["git", "config", "user.email", "test@test.com"],
            vec!["git", "config", "user.name", "Test"],
            vec!["git", "config", "commit.gpgsign", "false"],
        ] {
            let out = Command::new(args[0])
                .args(&args[1..])
                .current_dir(source_repo.path())
                .output()
                .unwrap();
            assert!(out.status.success());
        }
        fs::write(source_repo.path().join("diverge.txt"), "diverge").unwrap();
        for args in &[
            vec!["git", "add", "diverge.txt"],
            vec!["git", "commit", "-m", "diverge main"],
        ] {
            let out = Command::new(args[0])
                .args(&args[1..])
                .current_dir(source_repo.path())
                .output()
                .unwrap();
            assert!(out.status.success());
        }

        // Squash-merge the branch into main on the source repo
        squash_merge_branch(source_repo.path(), "rm-div-squash", "main");

        // Delete the remote branch on the source repo
        let out = Command::new("git")
            .args(["branch", "-D", "rm-div-squash"])
            .current_dir(source_repo.path())
            .output()
            .unwrap();
        assert!(out.status.success());

        // Remove should succeed without --force
        remove(&paths, "rm-div-squash", false).unwrap();
        assert!(!ws_dir.exists());
    }
}
