use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
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
}

pub const METADATA_FILE: &str = ".ws.yaml";

pub fn dir(workspaces_dir: &Path, name: &str) -> PathBuf {
    workspaces_dir.join(name)
}

pub fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("workspace name cannot be empty");
    }
    if name.contains('/') || name.contains('\\') {
        bail!("workspace name {:?} cannot contain path separators", name);
    }
    if name == "." || name == ".." {
        bail!("workspace name {:?} is not allowed", name);
    }
    Ok(())
}

pub fn load_metadata(ws_dir: &Path) -> Result<Metadata> {
    let data = fs::read_to_string(ws_dir.join(METADATA_FILE))?;
    let m: Metadata = serde_yml::from_str(&data)?;
    Ok(m)
}

pub fn save_metadata(ws_dir: &Path, m: &Metadata) -> Result<()> {
    let data = serde_yml::to_string(m)?;
    fs::write(ws_dir.join(METADATA_FILE), data)?;
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

    match create_inner(&paths.mirrors_dir, &branch, &ws_dir, name, repo_refs) {
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

    let meta = Metadata {
        name: name.to_string(),
        branch: branch.to_string(),
        repos,
        created: Utc::now(),
    };

    for (identity, r) in repo_refs {
        add_worktree(mirrors_dir, ws_dir, identity, branch, r)
            .map_err(|e| anyhow::anyhow!("adding worktree for {}: {}", identity, e))?;
    }

    save_metadata(ws_dir, &meta)?;
    Ok(())
}

pub fn add_repos(
    mirrors_dir: &Path,
    ws_dir: &Path,
    repo_refs: &BTreeMap<String, String>,
) -> Result<()> {
    let mut meta = load_metadata(ws_dir)?;

    for (identity, r) in repo_refs {
        if meta.repos.contains_key(identity) {
            eprintln!("  {} already in workspace, skipping", identity);
            continue;
        }
        add_worktree(mirrors_dir, ws_dir, identity, &meta.branch, r)
            .map_err(|e| anyhow::anyhow!("adding worktree for {}: {}", identity, e))?;
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

pub fn has_pending_changes(ws_dir: &Path) -> Result<Vec<String>> {
    let meta = load_metadata(ws_dir)?;
    let mut dirty = Vec::new();

    for identity in meta.repos.keys() {
        let parsed = match parse_identity(identity) {
            Ok(p) => p,
            Err(_) => continue,
        };
        let repo_dir = ws_dir.join(&parsed.repo);

        let changed = git::changed_file_count(&repo_dir).unwrap_or(0);
        let ahead = git::ahead_count(&repo_dir).unwrap_or(0);

        if changed > 0 || ahead > 0 {
            dirty.push(parsed.repo);
        }
    }

    Ok(dirty)
}

pub fn remove(paths: &Paths, name: &str, force: bool) -> Result<()> {
    let ws_dir = dir(&paths.workspaces_dir, name);
    let meta =
        load_metadata(&ws_dir).map_err(|e| anyhow::anyhow!("reading workspace metadata: {}", e))?;

    // Collect active repos (no fixed ref) that need branch cleanup
    struct ActiveRepo {
        identity: String,
        parsed: giturl::Parsed,
        mirror_dir: std::path::PathBuf,
        fetch_failed: bool,
    }

    let mut active_repos: Vec<ActiveRepo> = Vec::new();
    let mut context_repos: Vec<(giturl::Parsed, std::path::PathBuf)> = Vec::new();

    for (identity, entry) in &meta.repos {
        let parsed = match parse_identity(identity) {
            Ok(p) => p,
            Err(_) => {
                eprintln!(
                    "  warning: cannot parse {}, skipping worktree cleanup",
                    identity
                );
                continue;
            }
        };
        let mirror_dir = mirror::dir(&paths.mirrors_dir, &parsed);

        let is_active = match entry {
            None => true,
            Some(re) => re.r#ref.is_empty(),
        };

        if is_active {
            // Best-effort fetch to detect remote merges (e.g. PR merged on GitHub)
            let fetch_failed = git::fetch(&mirror_dir).is_err();
            if fetch_failed {
                eprintln!("  warning: fetch failed for {}, using local data", identity);
            }
            active_repos.push(ActiveRepo {
                identity: identity.clone(),
                parsed,
                mirror_dir,
                fetch_failed,
            });
        } else {
            context_repos.push((parsed, mirror_dir));
        }
    }

    // Pre-flight: check if all active branches are merged
    if !force {
        let mut unmerged: Vec<(String, bool)> = Vec::new();
        for ar in &active_repos {
            if !git::branch_exists(&ar.mirror_dir, &meta.branch) {
                continue; // branch already gone, nothing to check
            }
            let default_branch = match git::default_branch(&ar.mirror_dir) {
                Ok(b) => b,
                Err(e) => {
                    eprintln!(
                        "  warning: cannot detect default branch for {}: {}",
                        ar.identity, e
                    );
                    continue;
                }
            };
            let merged = git::branch_is_merged(&ar.mirror_dir, &meta.branch, &default_branch)
                .unwrap_or(false);
            if !merged {
                unmerged.push((ar.identity.clone(), ar.fetch_failed));
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

    // Pass 2: actual removal
    // Remove worktrees for all repos
    for ar in &active_repos {
        let worktree_path = ws_dir.join(&ar.parsed.repo);
        if let Err(e) = git::worktree_remove(&ar.mirror_dir, &worktree_path) {
            eprintln!("  warning: removing worktree for {}: {}", ar.identity, e);
        }
    }
    for (parsed, mirror_dir) in &context_repos {
        let worktree_path = ws_dir.join(&parsed.repo);
        if let Err(e) = git::worktree_remove(mirror_dir, &worktree_path) {
            eprintln!("  warning: removing worktree for {}: {}", parsed.repo, e);
        }
    }

    // Delete branches from active repos
    for ar in &active_repos {
        if !git::branch_exists(&ar.mirror_dir, &meta.branch) {
            continue;
        }
        if let Err(e) = git::branch_delete(&ar.mirror_dir, &meta.branch) {
            eprintln!(
                "  warning: deleting branch {} in {}: {}",
                meta.branch, ar.identity, e
            );
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

fn add_worktree(
    mirrors_dir: &Path,
    ws_dir: &Path,
    identity: &str,
    branch: &str,
    git_ref: &str,
) -> Result<()> {
    let parsed = parse_identity(identity)?;
    let mirror_dir = mirror::dir(mirrors_dir, &parsed);
    let worktree_path = ws_dir.join(&parsed.repo);

    // Context repo: check out at the specified ref
    if !git_ref.is_empty() {
        if git::branch_exists(&mirror_dir, git_ref) {
            return git::worktree_add_existing(&mirror_dir, &worktree_path, git_ref);
        }
        let remote_ref = format!("refs/remotes/origin/{}", git_ref);
        if git::ref_exists(&mirror_dir, &remote_ref) {
            let origin_ref = format!("origin/{}", git_ref);
            return git::worktree_add_existing(&mirror_dir, &worktree_path, &origin_ref);
        }
        // Tag or SHA: detached HEAD
        return git::worktree_add_detached(&mirror_dir, &worktree_path, git_ref);
    }

    // Active repo: create/checkout workspace branch
    if git::branch_exists(&mirror_dir, branch) {
        return git::worktree_add_existing(&mirror_dir, &worktree_path, branch);
    }

    let default_branch = git::default_branch(&mirror_dir)?;

    // In bare clones, branches are at refs/heads/<name>, not refs/remotes/origin/<name>.
    // Try origin/<branch> first; fall back to just <branch> for bare clones.
    let start_point_candidate = format!("origin/{}", default_branch);
    let start_point = if git::ref_exists(&mirror_dir, &start_point_candidate) {
        start_point_candidate
    } else {
        default_branch
    };

    git::worktree_add(&mirror_dir, &worktree_path, branch, &start_point)
}

fn parse_identity(identity: &str) -> Result<giturl::Parsed> {
    giturl::Parsed::from_identity(identity)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    /// Sets up a test environment using tempdirs. Returns Paths, TempDirs (keep alive!), and identity.
    fn setup_test_env() -> (Paths, tempfile::TempDir, tempfile::TempDir, String) {
        let tmp_data = tempfile::tempdir().unwrap();
        let tmp_home = tempfile::tempdir().unwrap();

        let data_dir = tmp_data.path().join("ws");
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

        (paths, tmp_data, repo_dir, parsed.identity())
    }

    #[test]
    fn test_create_and_load_metadata() {
        let (paths, _d, _r, identity) = setup_test_env();

        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(&paths, "test-ws", &refs, None).unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "test-ws");
        let meta = load_metadata(&ws_dir).unwrap();

        assert_eq!(meta.name, "test-ws");
        assert_eq!(meta.branch, "test-ws");
        assert!(meta.repos.contains_key(&identity));

        // Worktree directory should exist
        assert!(ws_dir.join("test-repo").exists());
    }

    #[test]
    fn test_create_with_branch_prefix() {
        let (paths, _d, _r, identity) = setup_test_env();

        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(&paths, "my-feature", &refs, Some("jganoff")).unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "my-feature");
        let meta = load_metadata(&ws_dir).unwrap();

        assert_eq!(meta.name, "my-feature");
        assert_eq!(meta.branch, "jganoff/my-feature");
        assert!(meta.repos.contains_key(&identity));
        assert!(ws_dir.join("test-repo").exists());
    }

    #[test]
    fn test_create_with_empty_branch_prefix() {
        let (paths, _d, _r, identity) = setup_test_env();

        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(&paths, "empty-prefix", &refs, Some("")).unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "empty-prefix");
        let meta = load_metadata(&ws_dir).unwrap();

        assert_eq!(meta.branch, "empty-prefix");
    }

    #[test]
    fn test_create_duplicate() {
        let (paths, _d, _r, identity) = setup_test_env();

        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(&paths, "test-ws-dup", &refs, None).unwrap();
        assert!(create(&paths, "test-ws-dup", &refs, None).is_err());
    }

    #[test]
    fn test_detect() {
        let (paths, _d, _r, identity) = setup_test_env();

        let refs = BTreeMap::from([(identity, String::new())]);
        create(&paths, "test-ws-detect", &refs, None).unwrap();

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
    fn test_remove_deletes_merged_branch() {
        let (paths, _d, _r, identity) = setup_test_env();

        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(&paths, "rm-merged", &refs, None).unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "rm-merged");
        assert!(ws_dir.exists());

        // Branch was created from main with no extra commits, so it's merged
        let parsed = parse_identity(&identity).unwrap();
        let mirror_dir = mirror::dir(&paths.mirrors_dir, &parsed);
        assert!(git::branch_exists(&mirror_dir, "rm-merged"));

        remove(&paths, "rm-merged", false).unwrap();
        assert!(!ws_dir.exists());
        assert!(!git::branch_exists(&mirror_dir, "rm-merged"));
    }

    #[test]
    fn test_remove_blocks_unmerged_branch() {
        let (paths, _d, _r, identity) = setup_test_env();

        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(&paths, "rm-unmerged", &refs, None).unwrap();

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

        // Workspace and branch should still exist
        assert!(ws_dir.exists());
        let parsed = parse_identity(&identity).unwrap();
        let mirror_dir = mirror::dir(&paths.mirrors_dir, &parsed);
        assert!(git::branch_exists(&mirror_dir, "rm-unmerged"));
    }

    #[test]
    fn test_remove_force_deletes_unmerged_branch() {
        let (paths, _d, _r, identity) = setup_test_env();

        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(&paths, "rm-force", &refs, None).unwrap();

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

        let parsed = parse_identity(&identity).unwrap();
        let mirror_dir = mirror::dir(&paths.mirrors_dir, &parsed);
        assert!(!git::branch_exists(&mirror_dir, "rm-force"));
    }

    #[test]
    fn test_list_all() {
        let (paths, _d, _r, identity) = setup_test_env();

        // Initially empty
        let names = list_all(&paths.workspaces_dir).unwrap();
        assert!(names.is_empty());

        // Create a workspace
        let refs = BTreeMap::from([(identity, String::new())]);
        create(&paths, "ws-1-list", &refs, None).unwrap();

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
        };

        save_metadata(tmp.path(), &meta).unwrap();
        let loaded = load_metadata(tmp.path()).unwrap();

        assert_eq!(loaded.name, meta.name);
        assert_eq!(loaded.repos.len(), 3);

        // Active repo: nil entry
        assert!(loaded.repos["github.com/acme/api-gateway"].is_none());

        // Context repo with branch ref
        assert_eq!(
            loaded.repos["github.com/acme/user-service"]
                .as_ref()
                .unwrap()
                .r#ref,
            "main"
        );

        // Context repo with tag ref
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
            ("dot", ".", true),
            ("dotdot", "..", true),
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

        let data_dir = tmp_data.path().join("ws");
        let workspaces_dir = tmp_home.path().join("dev").join("workspaces");
        fs::create_dir_all(&workspaces_dir).unwrap();

        let paths = Paths::from_dirs(&data_dir, &workspaces_dir);

        // Try to create with a nonexistent repo identity — will fail
        let refs = BTreeMap::from([("nonexistent.local/user/nope".into(), String::new())]);
        let result = create(&paths, "fail-ws", &refs, None);
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
        let (paths, _d, _r, identity) = setup_test_env();

        // Create workspace with the repo as context (ref = "main")
        let refs = BTreeMap::from([(identity.clone(), "main".into())]);
        create(&paths, "ctx-ws", &refs, None).unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "ctx-ws");
        let meta = load_metadata(&ws_dir).unwrap();

        assert!(meta.repos[&identity].is_some());
        assert_eq!(meta.repos[&identity].as_ref().unwrap().r#ref, "main");

        // Worktree directory should exist
        assert!(ws_dir.join("test-repo").exists());
    }

    #[test]
    fn test_add_repos_to_existing_workspace() {
        let (paths, _d, _r, identity) = setup_test_env();

        // Create workspace with active repo
        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(&paths, "add-ws", &refs, None).unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "add-ws");

        // Try adding the same repo again — should skip
        add_repos(&paths.mirrors_dir, &ws_dir, &refs).unwrap();

        let meta = load_metadata(&ws_dir).unwrap();
        assert_eq!(meta.repos.len(), 1);
    }

    #[test]
    fn test_has_pending_changes_clean() {
        let (paths, _d, _r, identity) = setup_test_env();

        let refs = BTreeMap::from([(identity, String::new())]);
        create(&paths, "pending-clean", &refs, None).unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "pending-clean");
        let dirty = has_pending_changes(&ws_dir).unwrap();
        assert!(dirty.is_empty());
    }

    #[test]
    fn test_has_pending_changes_uncommitted() {
        let (paths, _d, _r, identity) = setup_test_env();

        let refs = BTreeMap::from([(identity, String::new())]);
        create(&paths, "pending-dirty", &refs, None).unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "pending-dirty");
        let repo_dir = ws_dir.join("test-repo");
        fs::write(repo_dir.join("dirty.txt"), "x").unwrap();

        let dirty = has_pending_changes(&ws_dir).unwrap();
        assert!(dirty.contains(&"test-repo".to_string()));
    }

    #[test]
    fn test_remove_skips_branch_delete_for_context_repos() {
        let (paths, _d, _r, identity) = setup_test_env();

        // Create workspace with context repo (pinned to "main")
        let refs = BTreeMap::from([(identity, "main".into())]);
        create(&paths, "rm-ws-ctx", &refs, None).unwrap();

        // Remove should succeed without touching context repo branches
        remove(&paths, "rm-ws-ctx", false).unwrap();
    }
}
