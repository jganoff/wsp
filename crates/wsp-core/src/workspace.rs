use std::collections::BTreeMap;
use std::fs;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::config::{Config, Paths};
use crate::filelock;
use crate::git;
use crate::giturl;
use crate::mirror;
use crate::util::read_stdin_line;

pub const CURRENT_METADATA_VERSION: u32 = 0;

fn default_version() -> u32 {
    CURRENT_METADATA_VERSION
}

fn is_current_version(v: &u32) -> bool {
    *v == CURRENT_METADATA_VERSION
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkspaceRepoRef {
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub r#ref: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub url: Option<String>,
}

/// Workspace metadata stored in `.wsp.yaml`.
/// Adding a field? Search for `Metadata {` across the codebase — there are 25+ manual
/// initializers in tests. New Option fields need `config: None,` (or similar) in each.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Metadata {
    #[serde(
        default = "default_version",
        skip_serializing_if = "is_current_version"
    )]
    pub version: u32,
    pub name: String,
    pub branch: String,
    pub repos: BTreeMap<String, Option<WorkspaceRepoRef>>,
    pub created: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_from: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dirs: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<crate::template::TemplateConfig>,
    /// Per-repo setup command overrides for this workspace, keyed by repo identity.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub setup_commands: BTreeMap<String, Vec<String>>,
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

    /// Apply workspace config onto global config, returning a modified copy.
    /// Workspace config overrides global config; absent fields leave config unchanged.
    /// Same pattern as `Template::apply_config`.
    pub fn apply_workspace_config(&self, cfg: &crate::config::Config) -> crate::config::Config {
        let mut effective = cfg.clone();
        if let Some(ref settings) = self.config {
            if let Some(ref li) = settings.language_integrations {
                let target = effective
                    .language_integrations
                    .get_or_insert_with(std::collections::BTreeMap::new);
                for (k, v) in li {
                    target.insert(k.clone(), *v);
                }
            }
            if let Some(ref strategy) = settings.sync_strategy {
                effective.sync_strategy = Some(strategy.clone());
            }
            if let Some(ref gc) = settings.git_config {
                let target = effective
                    .git_config
                    .get_or_insert_with(std::collections::BTreeMap::new);
                for (k, v) in gc {
                    // Defense-in-depth: skip dangerous keys even if they slipped
                    // through load-time validation (e.g. programmatic construction).
                    if crate::config::validate_git_config_key(k).is_err() {
                        eprintln!(
                            "warning: workspace git config key {:?} is not allowed and was skipped",
                            k
                        );
                        continue;
                    }
                    target.insert(k.clone(), v.clone());
                }
            }
        }
        effective
    }
}

/// Detects repo-name collisions and returns a dirs map with `owner-repo` entries
/// for all identities that share the same repo short name.
/// Only colliding identities appear in the returned map.
pub fn compute_dir_names(identities: &[&str]) -> Result<BTreeMap<String, String>> {
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
    if name.starts_with('-') {
        bail!("workspace name {:?} cannot start with a dash", name);
    }
    if name.starts_with('.') {
        bail!("workspace name {:?} cannot start with a dot", name);
    }
    // Allow only safe characters — workspace names become directory names, git branch
    // names, and are interpolated into shell hooks (e.g. tmux rename-window).
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        bail!(
            "workspace name {:?} contains invalid characters (allowed: a-z, A-Z, 0-9, dash, underscore, dot)",
            name
        );
    }
    Ok(())
}

pub fn load_metadata(ws_dir: &Path) -> Result<Metadata> {
    let data = crate::util::read_yaml_file(&ws_dir.join(METADATA_FILE))?;
    let m: Metadata = serde_yaml_ng::from_str(&data)?;
    if m.version > CURRENT_METADATA_VERSION {
        eprintln!(
            "warning: .wsp.yaml has version {}, but this wsp only supports version {}. Some fields may be ignored.",
            m.version, CURRENT_METADATA_VERSION
        );
    }
    for (identity, dir_name) in &m.dirs {
        validate_dir_name(dir_name)
            .map_err(|e| anyhow::anyhow!("invalid dir override for {}: {}", identity, e))?;
    }
    // Reject workspace metadata with dangerous git config keys at load time.
    // Defense-in-depth: `apply_workspace_config` also skips them at apply time,
    // but catching them here gives an early, loud error rather than a silent skip.
    if let Some(ref settings) = m.config
        && let Some(ref gc) = settings.git_config
    {
        for key in gc.keys() {
            crate::config::validate_git_config_key(key).with_context(|| {
                format!(
                    "workspace metadata contains disallowed git config key '{}'",
                    key
                )
            })?;
        }
    }
    Ok(m)
}

fn validate_dir_name(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("directory name cannot be empty");
    }
    if name.contains('\0') || name.contains('/') || name.contains('\\') {
        bail!(
            "directory name {:?} contains path separators or null bytes",
            name
        );
    }
    if name == "."
        || name == ".."
        || name.contains("/../")
        || name.starts_with("../")
        || name.ends_with("/..")
    {
        bail!("directory name {:?} contains path traversal", name);
    }
    Ok(())
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
        let candidate = dir.join(METADATA_FILE);
        if candidate.exists() && is_workspace_metadata(&candidate) {
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

/// Identify which repo the current working directory belongs to.
///
/// Walks `cwd` against each repo's clone directory and returns the identity
/// of the repo whose directory contains `cwd`, or `None` if not found.
///
/// Uses `Metadata::dir_name()` so it works even on older workspaces where
/// `meta.dirs` is empty (falls back to the repo component of the identity).
pub fn repo_from_cwd(ws_dir: &Path, meta: &Metadata, cwd: &Path) -> Option<String> {
    let cwd = cwd.canonicalize().ok()?;
    let ws_dir = ws_dir.canonicalize().ok()?;
    for identity in meta.repos.keys() {
        // Skip unparseable identities rather than aborting the whole search.
        let dir_name = match meta.dir_name(identity) {
            Ok(d) => d,
            Err(_) => continue,
        };
        let repo_dir = ws_dir.join(&dir_name);
        if cwd.starts_with(&repo_dir) {
            return Some(identity.clone());
        }
    }
    None
}

/// Check whether a `.wsp.yaml` file is workspace metadata rather than a
/// per-repo config or template that happens to share the same filename.
/// Workspace metadata always contains required `name` and `branch` keys;
/// templates and per-repo files do not.
fn is_workspace_metadata(path: &Path) -> bool {
    #[derive(Deserialize)]
    struct Probe {
        #[allow(dead_code)]
        name: String,
        #[allow(dead_code)]
        branch: String,
    }
    let Ok(content) = crate::util::read_yaml_file(path) else {
        return false;
    };
    serde_yaml_ng::from_str::<Probe>(&content).is_ok()
}

/// Create a new workspace: clone all repos from their mirrors, check out
/// the workspace branch, and write `.wsp.yaml` metadata.
#[allow(clippy::too_many_arguments)]
pub fn create(
    paths: &Paths,
    name: &str,
    repo_refs: &BTreeMap<String, String>,
    branch_prefix: Option<&str>,
    branch_override: Option<&str>,
    upstream_urls: &BTreeMap<String, String>,
    description: Option<&str>,
    created_from: Option<&str>,
) -> Result<()> {
    validate_name(name)?;

    let branch = if let Some(b) = branch_override {
        b.to_string()
    } else {
        match branch_prefix.filter(|p| !p.is_empty()) {
            Some(prefix) => format!("{}/{}", prefix, name),
            None => name.to_string(),
        }
    };
    let branch_tracks_remote = branch_override.is_some();

    git::validate_branch_name(&branch)?;

    let ws_dir = dir(&paths.workspaces_dir, name);
    if ws_dir.exists() {
        // Allow resuming a partial workspace (dir exists but no valid metadata).
        // A previous `wsp new` may have crashed mid-clone, leaving a partial dir.
        let meta_path = ws_dir.join(METADATA_FILE);
        if meta_path.exists() {
            bail!("workspace {:?} already exists", name);
        }
        eprintln!("Resuming partial workspace creation for {:?}...", name);
    } else {
        fs::create_dir_all(&ws_dir)?;
    }

    match create_inner(&CreateInnerOpts {
        mirrors_dir: &paths.mirrors_dir,
        branch: &branch,
        branch_tracks_remote,
        ws_dir: &ws_dir,
        name,
        repo_refs,
        upstream_urls,
        description,
        created_from,
    }) {
        Ok(()) => Ok(()),
        Err(e) => {
            // Clean up workspace dir on failure (best-effort), but only if
            // the metadata was never written (partial state). If metadata
            // exists, the workspace is valid enough to keep.
            let meta_path = ws_dir.join(METADATA_FILE);
            if !meta_path.exists() {
                let _ = fs::remove_dir_all(&ws_dir);
            }
            Err(e)
        }
    }
}

struct CreateInnerOpts<'a> {
    mirrors_dir: &'a Path,
    branch: &'a str,
    /// When true, the workspace branch already exists remotely; check it out
    /// with `--track origin/<branch>` so push/pull work without configuration.
    branch_tracks_remote: bool,
    ws_dir: &'a Path,
    name: &'a str,
    repo_refs: &'a BTreeMap<String, String>,
    upstream_urls: &'a BTreeMap<String, String>,
    description: Option<&'a str>,
    created_from: Option<&'a str>,
}

fn create_inner(opts: &CreateInnerOpts) -> Result<()> {
    let mut repos: BTreeMap<String, Option<WorkspaceRepoRef>> = BTreeMap::new();
    for identity in opts.repo_refs.keys() {
        let url = opts.upstream_urls.get(identity).cloned();
        repos.insert(
            identity.clone(),
            Some(WorkspaceRepoRef {
                r#ref: String::new(),
                url,
            }),
        );
    }

    let identities: Vec<&str> = opts.repo_refs.keys().map(|s| s.as_str()).collect();
    let dirs = compute_dir_names(&identities)?;

    let meta = Metadata {
        version: CURRENT_METADATA_VERSION,
        name: opts.name.to_string(),
        branch: opts.branch.to_string(),
        repos,
        created: Utc::now(),
        description: opts.description.map(|s| s.to_string()),
        last_used: None,
        created_from: opts.created_from.map(|s| s.to_string()),
        dirs: dirs.clone(),
        config: None,
        setup_commands: std::collections::BTreeMap::new(),
    };

    for (index, identity) in opts.repo_refs.keys().enumerate() {
        let dn = meta.dir_name(identity)?;
        let dest = opts.ws_dir.join(&dn);
        let upstream = opts
            .upstream_urls
            .get(identity)
            .map(|s| s.as_str())
            .unwrap_or("");

        if dest.exists() {
            // Adopt existing directory (resume after partial failure).
            // Same checks as add_repos: validate identity, propagate refs,
            // and prompt about URL/branch mismatches.
            validate_existing_dir(&dest, identity)?;
            propagate_mirror_refs(opts.mirrors_dir, &dest, identity)?;
            if !upstream.is_empty() {
                prompt_origin_url_for_adopt(&dest, upstream)?;
            }
            prompt_branch_for_adopt(&dest, opts.branch)?;
            eprintln!("  adopted existing directory {}/", dn);
        } else {
            eprintln!(
                "  [{}/{}] Cloning {}...",
                index + 1,
                opts.repo_refs.len(),
                identity
            );
            clone_from_mirror(
                opts.mirrors_dir,
                opts.ws_dir,
                identity,
                &dn,
                opts.branch,
                upstream,
                opts.branch_tracks_remote,
            )
            .map_err(|e| anyhow::anyhow!("cloning repo {}: {}", identity, e))?;
        }
    }

    save_metadata(opts.ws_dir, &meta)?;
    Ok(())
}

/// Validate that an existing directory can be adopted as a managed repo.
/// Checks that it is not a symlink, is a git repo, has an origin remote, and its URL
/// matches the expected identity.
fn validate_existing_dir(dir: &Path, expected_identity: &str) -> Result<()> {
    // Refuse symlinks to prevent adoption of attacker-controlled directories
    let meta = fs::symlink_metadata(dir)?;
    if meta.file_type().is_symlink() {
        bail!(
            "directory {:?} is a symlink (refusing to adopt)",
            dir.file_name().unwrap_or_default()
        );
    }
    if !dir.join(".git").exists() {
        bail!(
            "directory {:?} exists but is not a git repository",
            dir.file_name().unwrap_or_default()
        );
    }
    let origin_url = git::remote_get_url(dir, "origin").map_err(|_| {
        anyhow::anyhow!(
            "directory {:?} exists but has no origin remote",
            dir.file_name().unwrap_or_default()
        )
    })?;
    let parsed = giturl::parse(&origin_url).map_err(|e| {
        anyhow::anyhow!(
            "directory {:?} has unparseable origin URL {:?}: {}",
            dir.file_name().unwrap_or_default(),
            origin_url,
            e
        )
    })?;
    let actual_identity = parsed.identity();
    if actual_identity != expected_identity {
        bail!(
            "directory {:?} origin remote ({}) doesn't match expected repo ({})",
            dir.file_name().unwrap_or_default(),
            actual_identity,
            expected_identity
        );
    }
    Ok(())
}

/// Prompt the user about origin URL when adopting an existing directory.
/// If the clone's origin URL differs from the registered URL, offer to repoint.
/// In non-interactive contexts, keeps as-is with a warning.
fn prompt_origin_url_for_adopt(dir: &Path, registered_url: &str) -> Result<()> {
    let clone_url = match git::remote_get_url(dir, "origin") {
        Ok(url) => url,
        Err(_) => return Ok(()), // no origin — already caught by validate_existing_dir
    };

    if clone_url == registered_url {
        return Ok(());
    }

    // Check if they resolve to the same identity (e.g., SSH vs HTTPS for same repo).
    // If identities match, the URLs are functionally equivalent but syntactically different.
    let clone_identity = giturl::parse(&clone_url).ok().map(|p| p.identity());
    let registered_identity = giturl::parse(registered_url).ok().map(|p| p.identity());
    if clone_identity.is_none()
        || registered_identity.is_none()
        || clone_identity != registered_identity
    {
        // Identity mismatch or unparseable — validate_existing_dir should have caught this
        return Ok(());
    }

    let dir_name = dir.file_name().unwrap_or_default().to_string_lossy();

    if !std::io::stdin().is_terminal() {
        eprintln!(
            "  warning: {}/ origin URL differs from registered URL (non-interactive, leaving as-is)",
            dir_name
        );
        eprintln!("    clone:      {}", clone_url);
        eprintln!("    registered: {}", registered_url);
        return Ok(());
    }

    eprintln!(
        "  warning: {}/ origin URL differs from registered URL",
        dir_name
    );
    eprintln!("    clone:      {}", clone_url);
    eprintln!("    registered: {}", registered_url);
    eprintln!("    [1] Keep current origin URL (default)");
    eprintln!("    [2] Repoint origin to registered URL");
    eprint!("  choice [1]: ");

    let choice = read_stdin_line();
    if choice.trim() == "2" {
        git::remote_set_url(dir, "origin", registered_url)?;
        eprintln!("  repointed origin to {}", registered_url);
    }

    Ok(())
}

/// Prompt the user about branch state when adopting an existing directory.
/// Returns Ok(()) after handling the branch (or leaving as-is).
/// In non-interactive contexts (stdin is not a terminal), defaults to leaving as-is.
fn prompt_branch_for_adopt(dir: &Path, ws_branch: &str) -> Result<()> {
    let current = git::branch_current(dir).unwrap_or_default();

    if current == ws_branch {
        // Already on workspace branch — nothing to do
        return Ok(());
    }

    let branch_exists = git::branch_exists(dir, ws_branch);
    let dir_name = dir.file_name().unwrap_or_default().to_string_lossy();

    if !std::io::stdin().is_terminal() {
        eprintln!(
            "  warning: {} is on branch '{}', not workspace branch '{}' (non-interactive, leaving as-is)",
            dir_name, current, ws_branch
        );
        return Ok(());
    }

    if branch_exists {
        eprintln!(
            "  warning: {} is on branch '{}', workspace branch is '{}'",
            dir_name, current, ws_branch
        );
        eprintln!("    [1] Leave as-is (default)");
        eprintln!("    [2] Switch to workspace branch '{}'", ws_branch);
    } else {
        eprintln!(
            "  warning: {} is on branch '{}', workspace branch '{}' does not exist",
            dir_name, current, ws_branch
        );
        eprintln!("    [1] Leave as-is (default)");
        eprintln!(
            "    [2] Create and checkout workspace branch '{}' from current HEAD",
            ws_branch
        );
    }

    eprint!("  choice [1]: ");
    let choice = read_stdin_line();

    if choice.trim() == "2" {
        if branch_exists {
            git::checkout(dir, ws_branch)?;
            eprintln!("  switched to branch '{}'", ws_branch);
        } else {
            git::checkout_new_branch(dir, ws_branch, "HEAD")?;
            eprintln!("  created and switched to branch '{}'", ws_branch);
        }
    }

    Ok(())
}

/// Propagate mirror refs into an existing clone directory.
/// Runs steps 4-6 of the clone_from_mirror process:
/// populate origin/* refs, set origin/HEAD, fix default branch tracking.
fn propagate_mirror_refs(mirrors_dir: &Path, dest: &Path, identity: &str) -> Result<()> {
    let parsed = parse_identity(identity)?;
    let mirror_dir = mirror::dir(mirrors_dir, &parsed);
    if !mirror_dir.exists() {
        return Ok(());
    }

    let mirror_default_br = match git::default_branch_from_mirror(&mirror_dir) {
        Ok(branch) => branch,
        Err(e) => {
            eprintln!(
                "  warning: cannot read default branch from mirror for {}: {}",
                identity, e
            );
            None
        }
    };

    // Populate origin/* refs from mirror (local fetch, no network)
    let _ = git::fetch_from_path(dest, &mirror_dir, MIRROR_PROPAGATE_REFSPEC, false);

    // Set origin/HEAD
    if let Some(ref default_br) = mirror_default_br {
        let _ = git::remote_set_head(dest, "origin", default_br);
    }

    // Fix default branch tracking and fast-forward local default branch
    if let Some(ref default_br) = mirror_default_br {
        let local_ref = format!("refs/heads/{}", default_br);
        let origin_ref = format!("origin/{}", default_br);
        if git::ref_exists(dest, &format!("refs/remotes/{}", origin_ref)) {
            let _ = git::set_upstream(dest, default_br, &origin_ref);
            // Only fast-forward; don't reset a branch that has local-only commits
            if git::is_ancestor(dest, &local_ref, &origin_ref) {
                let _ = git::update_ref(dest, &local_ref, &origin_ref);
            }
        }
    }

    Ok(())
}

pub fn add_repos(
    mirrors_dir: &Path,
    ws_dir: &Path,
    repo_refs: &BTreeMap<String, String>,
    upstream_urls: &BTreeMap<String, String>,
    branch_tracks_remote: bool,
) -> Result<()> {
    // Phase 1: snapshot metadata to determine branch and dir layout (fast lock)
    let snapshot = filelock::read_metadata(ws_dir)?;
    let branch = snapshot.branch.clone();

    // Phase 2: clone repos from mirrors outside the lock (slow I/O).
    // Pre-compute directory names for the union of existing + new repos using
    // compute_dir_names, which detects collisions both against existing repos
    // and among the new repos themselves (e.g. alice/utils + bob/utils).
    // Directory renames for existing repos are deferred to phase 3 (under lock).

    // Filter out repos already in the workspace
    let new_identities: Vec<&String> = repo_refs
        .keys()
        .filter(|id| {
            if snapshot.repos.contains_key(id.as_str()) {
                eprintln!("  {} already in workspace, skipping", id);
                false
            } else {
                true
            }
        })
        .collect();

    // Compute dir names for existing + new repos together to detect all collisions
    let all_identities: Vec<&str> = snapshot
        .repos
        .keys()
        .map(|s| s.as_str())
        .chain(new_identities.iter().map(|s| s.as_str()))
        .collect();
    let all_dirs = compute_dir_names(&all_identities)?;

    // Determine which existing repos need renaming (they now appear in all_dirs
    // but weren't in snapshot.dirs, or their dir name changed)
    struct RenameInfo {
        existing_id: String,
        old_dir: String,
        new_dir: String,
    }
    let mut renames: Vec<RenameInfo> = Vec::new();
    for existing_id in snapshot.repos.keys() {
        if let Some(new_dir) = all_dirs.get(existing_id) {
            let old_dir = snapshot.dir_name(existing_id)?;
            if *new_dir != old_dir {
                renames.push(RenameInfo {
                    existing_id: existing_id.clone(),
                    old_dir,
                    new_dir: new_dir.clone(),
                });
            }
        }
    }

    struct CloneInfo {
        identity: String,
        dir_name: String,
    }
    let mut clones: Vec<CloneInfo> = Vec::new();

    for (index, identity) in new_identities.iter().enumerate() {
        let upstream = upstream_urls
            .get(identity.as_str())
            .map(|s| s.as_str())
            .unwrap_or("");

        // Use disambiguated name from all_dirs if present, otherwise default
        let dn = match all_dirs.get(identity.as_str()) {
            Some(d) => d.clone(),
            None => {
                let parsed = parse_identity(identity)?;
                parsed.repo
            }
        };

        // A per-repo branch override is stored in repo_refs values (set when
        // the user passes `repo@branch`). When present, use it for the clone
        // with tracking enabled — the user is asserting the branch exists.
        let per_repo_branch = repo_refs
            .get(identity.as_str())
            .filter(|b| !b.is_empty())
            .map(|s| s.as_str());
        let (clone_branch, clone_branch_tracks_remote) = match per_repo_branch {
            Some(b) => (b, true),
            None => (branch.as_str(), branch_tracks_remote),
        };

        let dest = ws_dir.join(&dn);
        if dest.exists() {
            // Adopt existing directory instead of cloning
            validate_existing_dir(&dest, identity)?;
            propagate_mirror_refs(mirrors_dir, &dest, identity)?;
            if !upstream.is_empty() {
                prompt_origin_url_for_adopt(&dest, upstream)?;
            }
            prompt_branch_for_adopt(&dest, clone_branch)?;
            eprintln!("  adopted existing directory {}/", dn);
        } else {
            eprintln!(
                "  [{}/{}] Cloning {}...",
                index + 1,
                new_identities.len(),
                identity
            );
            if let Err(clone_err) = clone_from_mirror(
                mirrors_dir,
                ws_dir,
                identity,
                &dn,
                clone_branch,
                upstream,
                clone_branch_tracks_remote,
            ) {
                if dest.exists()
                    && let Err(cleanup_err) = fs::remove_dir_all(&dest)
                {
                    return Err(anyhow::anyhow!(
                        "cloning repo {}: {}; also failed to remove partial clone {}: {}",
                        identity,
                        clone_err,
                        dest.display(),
                        cleanup_err
                    ));
                }
                return Err(anyhow::anyhow!("cloning repo {}: {}", identity, clone_err));
            }
        }

        clones.push(CloneInfo {
            identity: identity.to_string(),
            dir_name: dn,
        });
    }

    if clones.is_empty() {
        return Ok(());
    }

    // Phase 3: rename colliding directories and update metadata under lock (fast)
    filelock::with_metadata(ws_dir, |meta| {
        // Rename existing repos that now collide with new additions
        for ri in &renames {
            if meta.repos.contains_key(&ri.existing_id) {
                fs::rename(ws_dir.join(&ri.old_dir), ws_dir.join(&ri.new_dir)).map_err(|e| {
                    anyhow::anyhow!("renaming directory for {}: {}", ri.existing_id, e)
                })?;
                meta.dirs.insert(ri.existing_id.clone(), ri.new_dir.clone());
            }
        }

        // Register new repos
        for ci in &clones {
            if all_dirs.contains_key(&ci.identity) {
                meta.dirs.insert(ci.identity.clone(), ci.dir_name.clone());
            }

            meta.repos.insert(ci.identity.clone(), None);
        }
        Ok(())
    })?;
    Ok(())
}

/// LEGACY(v0.5): remove the `wsp-mirror` remote from a clone if it exists.
/// Old versions of wsp added this remote; we no longer use it.
fn remove_legacy_wsp_mirror(clone_dir: &Path) {
    if git::has_remote(clone_dir, "wsp-mirror") {
        let _ = git::remove_remote(clone_dir, "wsp-mirror");
    }
}

/// Fetch a mirror from upstream and propagate refs to a clone (best-effort).
fn fetch_and_propagate(mirrors_dir: &Path, clone_dir: &Path, identity: &str) -> Result<()> {
    let parsed = parse_identity(identity)?;
    let mirror_path = mirror::dir(mirrors_dir, &parsed);
    remove_legacy_wsp_mirror(clone_dir);
    git::fetch(&mirror_path, true)?;
    git::fetch_from_path(clone_dir, &mirror_path, MIRROR_PROPAGATE_REFSPEC, true)?;
    Ok(())
}

/// Check all linked worktrees of `clone_dir` for uncommitted changes or
/// unpushed commits, returning problem strings formatted as
/// `"<identity> (linked worktree <path> ...)"`.
///
/// Covers four cases:
/// 1. Dirty working tree (`git status --short` non-empty)
/// 2. Branch with upstream tracking that is ahead (`@{upstream}..HEAD`)
/// 3. Local-only branch with commits not reachable from the default branch
///    — `ahead_count` returns 0 for untracked branches, so this extra check
///    prevents silent data loss when a user commits in a worktree on a
///    branch they haven't pushed yet.
/// 4. Clean linked worktree outside `ws_dir` — moving `ws_dir` to gc would
///    leave the external worktree with a broken `.git` file (orphaned).
///    Blocked even when clean; use `--force` to proceed.
fn check_linked_worktrees(clone_dir: &Path, ws_dir: &Path, identity: &str) -> Vec<String> {
    let mut problems = Vec::new();
    let worktrees = match git::list_linked_worktrees(clone_dir) {
        Ok(wts) => wts,
        Err(e) => {
            // Fail closed: if we cannot enumerate linked worktrees we cannot
            // verify their safety, so block the removal rather than assume safe.
            problems.push(format!(
                "{} (could not enumerate linked worktrees: {})",
                identity, e
            ));
            return problems;
        }
    };
    if worktrees.is_empty() {
        return problems;
    }

    // Resolve default branch once; used for local-only branch detection.
    let default_branch = git::default_branch_for_remote(clone_dir, "origin")
        .or_else(|_| git::default_branch(clone_dir))
        .unwrap_or_default();
    let merge_target = if !default_branch.is_empty() {
        let candidate = format!("origin/{}", default_branch);
        if git::ref_exists(clone_dir, &candidate) {
            candidate
        } else {
            default_branch.clone()
        }
    } else {
        String::new()
    };

    for wt in worktrees {
        // A prunable entry has no live checkout to inspect or orphan. Treat
        // Git's read-only classification as non-blocking; do not run
        // `git worktree prune` here: merely checking removal safety must not
        // alter worktree registrations.
        if wt.prunable {
            continue;
        }

        let wt_display = wt.path.display().to_string();
        let branch_label = wt.branch.as_deref().unwrap_or("detached HEAD");

        let wt_changed = git::changed_file_count(&wt.path).unwrap_or(0);
        if wt_changed > 0 {
            problems.push(format!(
                "{} (linked worktree {} has uncommitted changes)",
                identity, wt_display
            ));
            continue;
        }

        // Upstream-tracked branch: ahead_count works directly.
        let wt_ahead = git::ahead_count(&wt.path).unwrap_or(0);
        if wt_ahead > 0 {
            problems.push(format!(
                "{} (linked worktree {} branch '{}' has unpushed commits)",
                identity, wt_display, branch_label
            ));
            continue;
        }

        // Local-only branch (no upstream): count commits vs default branch.
        if let Some(ref branch) = wt.branch
            && !merge_target.is_empty()
        {
            let local_ahead = git::commit_count(&wt.path, &merge_target, branch).unwrap_or(0);
            if local_ahead > 0 {
                problems.push(format!(
                    "{} (linked worktree {} branch '{}' has {} unpushed commit{})",
                    identity,
                    wt_display,
                    branch,
                    local_ahead,
                    if local_ahead == 1 { "" } else { "s" }
                ));
                continue;
            }
        }

        // External worktree (clean, no unpushed work): moving ws_dir to gc
        // would orphan this worktree — its .git file would point to a deleted
        // gitdir. Block even when clean so the user is aware.
        //
        // Canonicalize both sides: git returns a resolved path (e.g. /private/var/... on
        // macOS) while ws_dir may be a symlinked path (e.g. /var/...). Without
        // canonicalization starts_with would falsely treat an inside-ws_dir worktree
        // as external.
        let wt_canonical = wt.path.canonicalize().unwrap_or_else(|_| wt.path.clone());
        let ws_canonical = ws_dir
            .canonicalize()
            .unwrap_or_else(|_| ws_dir.to_path_buf());
        if !wt_canonical.starts_with(&ws_canonical) {
            problems.push(format!(
                "{} (linked worktree {} is outside the workspace and would be orphaned by removal — use --force to proceed)",
                identity, wt_display
            ));
        }
    }
    problems
}

/// Remove one or more repos from a workspace.
///
/// Safety checks mirror those in [`check_removal_blockers`] with one key
/// difference: both `PushedToRemote` and `Unmerged` are treated as hard
/// blockers here because `remove_repos` has no interactive prompt path.
/// See [`check_removal_blockers`] for the soft-blocker variant used by `wsp rm`.
pub fn remove_repos(
    mirrors_dir: &Path,
    ws_dir: &Path,
    identities_to_remove: &[String],
    force: bool,
) -> Result<()> {
    // Phase 1: snapshot metadata for safety checks (fast lock)
    let snapshot = filelock::read_metadata(ws_dir)?;

    // Validate all identities exist in the workspace
    for identity in identities_to_remove {
        if !snapshot.repos.contains_key(identity) {
            bail!("repo {} is not in this workspace", identity);
        }
    }

    // Phase 2: safety checks including network fetch (slow, no lock held)
    if !force {
        let mut problems: Vec<String> = Vec::new();
        for identity in identities_to_remove {
            let dn = snapshot.dir_name(identity)?;
            let clone_dir = ws_dir.join(&dn);

            let changed = git::changed_file_count(&clone_dir).unwrap_or(0);
            if changed > 0 {
                // TODO: list the specific files using git::changed_files() — cap at
                // ~3 names, fall back to "N files" for larger counts. Would have
                // directly answered "I can't find out which ones they are."
                problems.push(format!("{} (uncommitted changes)", identity));
                continue;
            }

            let wt_problems = check_linked_worktrees(&clone_dir, ws_dir, identity);
            if !wt_problems.is_empty() {
                problems.extend(wt_problems);
                continue;
            }

            let current = git::branch_current(&clone_dir).unwrap_or_default();

            let fetch_failed = fetch_and_propagate(mirrors_dir, &clone_dir, identity).is_err();
            if fetch_failed {
                eprintln!("  warning: fetch failed for {}, using local data", identity);
            }

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

                // Also check the currently-checked-out branch when it differs from the
                // workspace branch — it may have unmerged work. Both PushedToRemote and
                // Unmerged are hard blockers here since remove_repos has no prompt path.
                if !current.is_empty()
                    && current != "HEAD"
                    && current != snapshot.branch
                    && git::validate_branch_name(&current).is_ok()
                {
                    match git::branch_safety(&clone_dir, &current, &target) {
                        git::BranchSafety::Merged | git::BranchSafety::SquashMerged => {}
                        git::BranchSafety::PushedToRemote => {
                            let mut msg = format!(
                                "{} (current branch '{}' is pushed but unmerged)",
                                identity, current
                            );
                            if fetch_failed {
                                msg.push_str(" (fetch failed, local data may be stale)");
                            }
                            problems.push(msg);
                        }
                        git::BranchSafety::Unmerged => {
                            let mut msg =
                                format!("{} (current branch '{}' is unmerged)", identity, current);
                            if fetch_failed {
                                msg.push_str(" (fetch failed, local data may be stale)");
                            }
                            problems.push(msg);
                        }
                    }
                }

                if git::branch_exists(&clone_dir, &snapshot.branch) {
                    match git::branch_safety(&clone_dir, &snapshot.branch, &target) {
                        git::BranchSafety::Merged | git::BranchSafety::SquashMerged => {}
                        git::BranchSafety::PushedToRemote => {
                            let mut msg =
                                format!("{} (unmerged branch, but pushed to remote)", identity);
                            if fetch_failed {
                                msg.push_str(" (fetch failed, local data may be stale)");
                            }
                            problems.push(msg);
                        }
                        git::BranchSafety::Unmerged => {
                            let mut msg = format!("{} (unmerged branch)", identity);
                            if fetch_failed {
                                msg.push_str(" (fetch failed, local data may be stale)");
                            }
                            problems.push(msg);
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

    // Phase 3: remove directories and update metadata under lock (fast)
    filelock::with_metadata(ws_dir, |meta| {
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
        let mut new_dirs = compute_dir_names(&remaining_ids)?;

        // Check if any collision disambiguations can be undone (same-collision group,
        // but renamed to a different disambiguated path). Collect failures so we can
        // retain the old dir name in new_dirs — prevents metadata from drifting
        // out of sync with the filesystem when the rename fails.
        let mut retain_old: Vec<(String, String)> = Vec::new();
        for (identity, new_dir) in &new_dirs {
            if let Some(old_dir) = meta.dirs.get(identity)
                && old_dir != new_dir
                && let Err(e) = fs::rename(ws_dir.join(old_dir), ws_dir.join(new_dir))
            {
                eprintln!("  warning: renaming directory for {}: {}", identity, e);
                retain_old.push((identity.clone(), old_dir.clone()));
            }
        }
        // Retain old dirs for failed renames — don't apply de-disambiguation for these
        for (identity, old_dir) in retain_old {
            new_dirs.insert(identity, old_dir);
        }

        // Check if repos that were disambiguated can now use their short name.
        // Collect failures so we can preserve the old dir entry — prevents metadata
        // from claiming the short name when the filesystem still has the long name.
        let mut retain_long: Vec<(String, String)> = Vec::new();
        for identity in meta.repos.keys() {
            if let Some(old_dir) = meta.dirs.get(identity).cloned()
                && !new_dirs.contains_key(identity)
            {
                let parsed = parse_identity(identity)?;
                let short_name = parsed.repo.clone();
                if let Err(e) = fs::rename(ws_dir.join(&old_dir), ws_dir.join(&short_name)) {
                    eprintln!("  warning: renaming directory for {}: {}", identity, e);
                    retain_long.push((identity.clone(), old_dir));
                }
            }
        }
        for (identity, old_dir) in retain_long {
            new_dirs.insert(identity, old_dir);
        }

        // Update dirs map
        meta.dirs = new_dirs;
        Ok(())
    })?;
    Ok(())
}

/// Resolved per-repo info for workspace-scoped commands.
pub struct RepoInfo {
    pub identity: String,
    pub dir_name: String,
    pub clone_dir: PathBuf,
    pub error: Option<String>,
}

impl Metadata {
    /// Build a RepoInfo for each repo in the workspace.
    pub fn repo_infos(&self, ws_dir: &Path) -> Vec<RepoInfo> {
        let mut infos = Vec::new();
        for identity in self.repos.keys() {
            let dir_name = match self.dir_name(identity) {
                Ok(d) => d,
                Err(e) => {
                    infos.push(RepoInfo {
                        identity: identity.clone(),
                        dir_name: identity.clone(),
                        clone_dir: PathBuf::new(),
                        error: Some(e.to_string()),
                    });
                    continue;
                }
            };
            let clone_dir = ws_dir.join(&dir_name);
            infos.push(RepoInfo {
                identity: identity.clone(),
                dir_name,
                clone_dir,
                error: None,
            });
        }
        infos
    }
}

const MIRROR_PROPAGATE_REFSPEC: &str = "+refs/remotes/origin/*:refs/remotes/origin/*";

/// A workspace repo queued for mirror ref propagation.
struct PropagationTarget {
    identity: String,
    dir_name: String,
    clone_dir: PathBuf,
    mirror_dir: PathBuf,
    /// Whether the identity is present in the global registry.
    registered: bool,
}

/// Warning for a workspace repo whose clone directory is gone.
///
/// Without this the fetch below fails with a bare `No such file or directory`
/// from the process spawn, which says nothing about the workspace.
fn missing_clone_warning(identity: &str, dir_name: &str) -> String {
    format!(
        "  warning: {}: clone directory '{}' is missing, skipping mirror ref propagation\n  \
         hint: run `wsp doctor` to inspect, or `wsp repo rm {}` to remove it from this workspace",
        identity, dir_name, dir_name
    )
}

/// Warning for a workspace repo that has no mirror to propagate from.
///
/// A missing mirror almost always means the identity was never registered (so
/// no mirror was ever cloned); the mirror being deleted out from under a
/// registered repo is the rarer case. Both surface as the same opaque
/// `does not appear to be a git repository` fatal from `git fetch`, so name the
/// cause and the fix instead of letting git's error through.
///
/// The mirror path is deliberately not shown — mirrors are invisible
/// infrastructure, and `wsp doctor --fix` is the remedy either way.
fn missing_mirror_warning(identity: &str, dir_name: &str, registered: bool) -> String {
    if registered {
        format!(
            "  warning: {}: mirror is missing, skipping mirror ref propagation\n  \
             hint: run `wsp doctor --fix` to re-clone the mirror",
            identity
        )
    } else {
        format!(
            "  warning: {} is referenced by this workspace but is not registered, \
             skipping mirror ref propagation\n  \
             hint: run `wsp doctor --fix` to register it from the clone's origin URL, \
             or `wsp repo rm {}` to remove it from this workspace",
            identity, dir_name
        )
    }
}

/// Propagate mirror refs into workspace clones (parallel, best-effort).
/// Fetches `refs/remotes/origin/*` from the mirror into each clone's `origin/*`.
/// Also removes the legacy `wsp-mirror` remote if present.
/// Callers wanting deleted-branch cleanup should pass `prune: true`.
///
/// Repos without a usable clone or mirror are skipped with an actionable
/// warning on stderr; `cfg` is only consulted to tell an unregistered repo apart
/// from a registered one whose mirror went missing.
pub fn propagate_mirror_to_clones(
    mirrors_dir: &Path,
    ws_dir: &Path,
    meta: &Metadata,
    cfg: &Config,
    prune: bool,
) {
    let targets: Vec<PropagationTarget> = meta
        .repos
        .keys()
        .filter_map(|id| {
            let dn = meta.dir_name(id).ok()?;
            let parsed = parse_identity(id).ok()?;
            Some(PropagationTarget {
                identity: id.clone(),
                clone_dir: ws_dir.join(&dn),
                dir_name: dn,
                mirror_dir: mirror::dir(mirrors_dir, &parsed),
                registered: cfg.repos.contains_key(id),
            })
        })
        .collect();

    if targets.is_empty() {
        return;
    }

    std::thread::scope(|s| {
        let handles: Vec<_> = targets
            .iter()
            .map(|t| {
                s.spawn(move || {
                    if !t.clone_dir.exists() {
                        return Some(missing_clone_warning(&t.identity, &t.dir_name));
                    }
                    remove_legacy_wsp_mirror(&t.clone_dir);
                    if !t.mirror_dir.exists() {
                        return Some(missing_mirror_warning(
                            &t.identity,
                            &t.dir_name,
                            t.registered,
                        ));
                    }
                    git::fetch_from_path(
                        &t.clone_dir,
                        &t.mirror_dir,
                        MIRROR_PROPAGATE_REFSPEC,
                        prune,
                    )
                    .err()
                    .map(|e| format!("  warning: propagate mirror for {}: {}", t.identity, e))
                })
            })
            .collect();
        // Print after joining so warnings from parallel fetches don't interleave.
        for h in handles {
            match h.join() {
                Ok(Some(warning)) => eprintln!("{}", warning),
                Ok(None) => {}
                Err(_) => eprintln!("warning: propagate thread panicked"),
            }
        }
    });
}

// ---------------------------------------------------------------------------
// RootProblem — structured representation of workspace root issues
// ---------------------------------------------------------------------------

/// A problem detected in the workspace root directory.
#[derive(Debug, Clone)]
pub struct RootProblem {
    /// Relative path from workspace root (e.g. ".claude/settings.local.json", "notes.md")
    pub path: String,
    pub kind: RootProblemKind,
}

#[derive(Debug, Clone)]
pub enum RootProblemKind {
    /// Untracked file or directory (path ends with `/` for directories)
    Untracked,
    /// Modified managed file with detail description
    Modified { detail: String },
}

impl std::fmt::Display for RootProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.kind {
            RootProblemKind::Untracked => write!(f, "?? {}", self.path),
            RootProblemKind::Modified { detail } => write!(f, " M {} ({})", self.path, detail),
        }
    }
}

// ---------------------------------------------------------------------------
// .wspignore — pattern parsing and matching
// ---------------------------------------------------------------------------

/// Default content for the global wspignore file, seeded on first use.
pub const DEFAULT_WSPIGNORE: &str = "\
# Global wspignore — paths to suppress in workspace root checks.
# Edit this file to add/remove patterns. One path per line.
# Trailing / matches a directory and everything inside it.

# OS noise
.DS_Store
Thumbs.db
desktop.ini

# Claude Code local settings (not managed by wsp)
.claude/settings.local.json
";

#[derive(Debug, Clone, PartialEq)]
pub enum IgnorePattern {
    /// Exact filename match (e.g. "settings.local.json")
    Exact(String),
    /// Directory prefix match — matches any path starting with this prefix (e.g. ".claude/")
    DirPrefix(String),
}

/// Parse wspignore file content into patterns.
fn parse_wspignore(content: &str) -> Vec<IgnorePattern> {
    content
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                return None;
            }
            if let Some(dir) = trimmed.strip_suffix('/') {
                Some(IgnorePattern::DirPrefix(format!("{}/", dir)))
            } else {
                Some(IgnorePattern::Exact(trimmed.to_string()))
            }
        })
        .collect()
}

/// Load patterns from a wspignore file, returning empty vec if the file doesn't exist.
fn load_wspignore_file(path: &Path) -> Vec<IgnorePattern> {
    match fs::read_to_string(path) {
        Ok(content) => parse_wspignore(&content),
        Err(_) => Vec::new(),
    }
}

/// Check if a root problem path matches any ignore pattern.
pub fn is_ignored(path: &str, patterns: &[IgnorePattern]) -> bool {
    for pat in patterns {
        match pat {
            IgnorePattern::Exact(name) => {
                if path == name {
                    return true;
                }
            }
            IgnorePattern::DirPrefix(prefix) => {
                // Match the directory itself (e.g. ".claude/" matches ".claude/")
                // and anything inside it (e.g. ".claude/" matches ".claude/settings.json")
                // Also matches bare dir name without slash (e.g. ".claude/" matches ".claude")
                if path.starts_with(prefix.as_str()) || path == prefix.trim_end_matches('/') {
                    return true;
                }
            }
        }
    }
    false
}

/// Load wspignore patterns from both global and per-workspace files.
/// Creates the global wspignore with defaults on first use.
pub fn load_wspignore(data_dir: &Path, ws_dir: &Path) -> Vec<IgnorePattern> {
    let _ = ensure_global_wspignore(data_dir);
    let mut patterns = load_wspignore_file(&data_dir.join("wspignore"));
    patterns.extend(load_wspignore_file(&ws_dir.join(".wspignore")));
    patterns
}

/// Filter out ignored problems from a list of root problems.
pub fn filter_ignored(problems: &[RootProblem], patterns: &[IgnorePattern]) -> Vec<RootProblem> {
    problems
        .iter()
        .filter(|p| !is_ignored(&p.path, patterns))
        .cloned()
        .collect()
}

/// Create the default global wspignore if it doesn't exist.
/// Uses O_CREAT|O_EXCL (create_new) for atomic creation — no TOCTOU race.
pub(crate) fn ensure_global_wspignore(data_dir: &Path) -> Result<()> {
    let path = data_dir.join("wspignore");
    // Ensure the data dir exists (may not on first ever use)
    fs::create_dir_all(data_dir).context("creating data directory")?;
    match fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)
    {
        Ok(mut f) => {
            f.write_all(DEFAULT_WSPIGNORE.as_bytes())
                .context("writing default wspignore")?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e).context("creating default wspignore"),
    }
    Ok(())
}

/// Check workspace root for user content not managed by wsp.
/// Returns a list of structured root problems.
pub fn check_root_content(ws_dir: &Path, metadata: &Metadata) -> Result<Vec<RootProblem>> {
    let mut problems = Vec::new();

    // Build set of known repo dir names
    let mut repo_dirs: std::collections::HashSet<String> = std::collections::HashSet::new();
    for identity in metadata.repos.keys() {
        if let Ok(dn) = metadata.dir_name(identity) {
            repo_dirs.insert(dn);
        }
    }

    let go_work_is_wsp = ws_dir.join("go.work").exists() && check_go_work(ws_dir).is_none();

    for entry in fs::read_dir(ws_dir).context("reading workspace root directory")? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        // Skip .wsp.yaml and its lock file
        if name_str == METADATA_FILE || name_str == ".wsp.yaml.lock" {
            continue;
        }

        // Skip .wspignore
        if name_str == ".wspignore" {
            continue;
        }

        // Skip repo clone dirs (checked by repo safety)
        if repo_dirs.contains(name_str.as_ref()) {
            continue;
        }

        // AGENTS.md
        if name_str == "AGENTS.md" {
            if let Some(problem) = check_agents_md(ws_dir) {
                problems.push(problem);
            }
            continue;
        }

        // CLAUDE.md
        if name_str == "CLAUDE.md" {
            if let Some(problem) = check_claude_md(ws_dir) {
                problems.push(problem);
            }
            continue;
        }

        // .claude/ directory
        if name_str == ".claude" {
            problems.extend(check_claude_dir(ws_dir));
            continue;
        }

        // go.work
        if name_str == "go.work" {
            if let Some(problem) = check_go_work(ws_dir) {
                problems.push(problem);
            }
            continue;
        }

        // go.work.sum — safe when go.work is wsp-generated
        if name_str == "go.work.sum" && go_work_is_wsp {
            continue;
        }

        // Everything else is flagged
        let ft = entry.file_type()?;
        if ft.is_dir() {
            problems.push(RootProblem {
                path: format!("{}/", name_str),
                kind: RootProblemKind::Untracked,
            });
        } else {
            problems.push(RootProblem {
                path: name_str.to_string(),
                kind: RootProblemKind::Untracked,
            });
        }
    }

    Ok(problems)
}

/// Check AGENTS.md for user content outside wsp markers.
fn check_agents_md(ws_dir: &Path) -> Option<RootProblem> {
    let path = ws_dir.join("AGENTS.md");
    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => {
            return Some(RootProblem {
                path: "AGENTS.md".into(),
                kind: RootProblemKind::Modified {
                    detail: "unreadable".into(),
                },
            });
        }
    };

    // Find the begin marker
    let begin_idx = match content.find(crate::agentmd::MARKER_BEGIN) {
        Some(idx) => idx,
        None => {
            return Some(RootProblem {
                path: "AGENTS.md".into(),
                kind: RootProblemKind::Modified {
                    detail: "wsp markers missing".into(),
                },
            });
        }
    };

    // Check content before the begin marker for user additions
    let preamble = &content[..begin_idx];
    for line in preamble.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Scaffold lines emitted by agentmd::build_initial_file()
        if trimmed.starts_with("# Workspace: ") {
            continue;
        }
        if trimmed == "<!-- Add your project-specific notes for AI agents here -->" {
            continue;
        }
        // Any other non-blank line is user content
        return Some(RootProblem {
            path: "AGENTS.md".into(),
            kind: RootProblemKind::Modified {
                detail: "user-added content".into(),
            },
        });
    }

    // Check content after the end marker for user additions.
    // agentmd::replace_marked_section preserves post-marker content,
    // so users reasonably expect it persists across wsp operations.
    if let Some(end_idx) = content.find(crate::agentmd::MARKER_END) {
        let after_end = &content[end_idx + crate::agentmd::MARKER_END.len()..];
        for line in after_end.lines() {
            if !line.trim().is_empty() {
                return Some(RootProblem {
                    path: "AGENTS.md".into(),
                    kind: RootProblemKind::Modified {
                        detail: "user-added content after markers".into(),
                    },
                });
            }
        }
    }

    None
}

/// Check CLAUDE.md — symlink to AGENTS.md is fine, anything else is flagged.
fn check_claude_md(ws_dir: &Path) -> Option<RootProblem> {
    let path = ws_dir.join("CLAUDE.md");
    match fs::symlink_metadata(&path) {
        Ok(meta) => {
            if meta.file_type().is_symlink() {
                match fs::read_link(&path) {
                    Ok(target) if target == Path::new("AGENTS.md") => None,
                    _ => Some(RootProblem {
                        path: "CLAUDE.md".into(),
                        kind: RootProblemKind::Modified {
                            detail: "symlink to unexpected target".into(),
                        },
                    }),
                }
            } else {
                Some(RootProblem {
                    path: "CLAUDE.md".into(),
                    kind: RootProblemKind::Untracked,
                })
            }
        }
        Err(_) => None, // doesn't exist, fine
    }
}

/// Check .claude/ directory for non-wsp content.
fn check_claude_dir(ws_dir: &Path) -> Vec<RootProblem> {
    let claude_dir = ws_dir.join(".claude");
    let mut problems = Vec::new();

    // Known wsp-managed paths (relative to .claude/)
    let managed: std::collections::HashSet<&str> = [
        "skills/wsp-manage/SKILL.md",
        "skills/wsp-report/SKILL.md",
        "skills/wsp-new-feature/SKILL.md",
    ]
    .iter()
    .copied()
    .collect();

    // Intermediate directories that only contain managed content
    let managed_dirs: std::collections::HashSet<&str> = [
        "skills",
        "skills/wsp-manage",
        "skills/wsp-report",
        "skills/wsp-new-feature",
    ]
    .iter()
    .copied()
    .collect();

    fn walk(
        base: &Path,
        rel: &str,
        managed: &std::collections::HashSet<&str>,
        managed_dirs: &std::collections::HashSet<&str>,
        problems: &mut Vec<RootProblem>,
    ) {
        let dir = if rel.is_empty() {
            base.to_path_buf()
        } else {
            base.join(rel)
        };
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => return,
        };
        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            let child_rel = if rel.is_empty() {
                name_str.to_string()
            } else {
                format!("{}/{}", rel, name_str)
            };

            let ft = match entry.file_type() {
                Ok(ft) => ft,
                Err(_) => continue,
            };

            if ft.is_dir() {
                if managed_dirs.contains(child_rel.as_str()) {
                    walk(base, &child_rel, managed, managed_dirs, problems);
                } else {
                    problems.push(RootProblem {
                        path: format!(".claude/{}/", child_rel),
                        kind: RootProblemKind::Untracked,
                    });
                }
            } else if !managed.contains(child_rel.as_str()) {
                problems.push(RootProblem {
                    path: format!(".claude/{}", child_rel),
                    kind: RootProblemKind::Untracked,
                });
            }
        }
    }

    walk(&claude_dir, "", &managed, &managed_dirs, &mut problems);
    problems
}

/// Check go.work — wsp-generated header means it's managed.
pub fn check_go_work(ws_dir: &Path) -> Option<RootProblem> {
    let path = ws_dir.join("go.work");
    if !path.exists() {
        return None;
    }
    match fs::read_to_string(&path) {
        Ok(content) if content.starts_with(crate::lang::GO_WORK_HEADER) => None,
        Ok(_) => Some(RootProblem {
            path: "go.work".into(),
            kind: RootProblemKind::Untracked,
        }),
        Err(_) => Some(RootProblem {
            path: "go.work".into(),
            kind: RootProblemKind::Modified {
                detail: "unreadable".into(),
            },
        }),
    }
}

/// Returns true if the workspace directory exists but has no `.wsp.yaml`
/// (partial creation — wsp new was interrupted before metadata was written).
pub fn is_partial_workspace(paths: &Paths, name: &str) -> bool {
    let ws_dir = dir(&paths.workspaces_dir, name);
    ws_dir.exists() && !ws_dir.join(METADATA_FILE).exists()
}

/// Categorized issues that would prevent workspace removal without `--force`.
///
/// Returned by [`check_removal_blockers`] so the CLI can decide whether to
/// prompt the user (folding pushed-but-unmerged branch warnings into an
/// open-PR confirmation) rather than producing a generic error.
#[derive(Debug, Default)]
pub struct RemovalBlockers {
    /// The workspace branch name (for error messages).
    pub branch: String,
    /// Uncommitted changes, linked-worktree problems, wrong-branch unpushed
    /// commits, or workspace root content. Cannot be confirmed via a prompt;
    /// require `--force` to override.
    pub hard: Vec<String>,
    /// Branch exists on the remote (`origin/<branch>`) but is not merged into
    /// the default branch. An open PR may exist. Can be acknowledged via an
    /// interactive confirmation prompt (or `--force`).
    pub pushed_unmerged: Vec<String>,
    /// Branch exists only locally and was never pushed. No PR can exist.
    /// Requires `--force` to override.
    pub local_unmerged: Vec<String>,
}

impl RemovalBlockers {
    pub fn is_empty(&self) -> bool {
        self.hard.is_empty() && self.pushed_unmerged.is_empty() && self.local_unmerged.is_empty()
    }

    /// All problems as a flat sorted list, for error display.
    pub fn all_sorted(&self) -> Vec<String> {
        let mut all: Vec<String> = self
            .hard
            .iter()
            .chain(self.pushed_unmerged.iter())
            .chain(self.local_unmerged.iter())
            .cloned()
            .collect();
        all.sort();
        all
    }
}

/// Run all removal safety checks and return categorized blockers.
///
/// This is the same logic that [`remove`] applies internally when `force=false`,
/// factored out so the CLI can inspect the results before deciding whether to
/// prompt the user. Performs a remote fetch (via the mirror) as part of
/// branch-safety checking.
///
/// Unlike [`remove_repos`], pushed-but-unmerged branches are soft blockers
/// (returned in `pushed_unmerged`) so the CLI can fold them into an open-PR
/// prompt. `remove_repos` has no prompt path so it treats them as hard blockers.
pub fn check_removal_blockers(paths: &Paths, name: &str) -> Result<RemovalBlockers> {
    validate_name(name)?;
    let ws_dir = dir(&paths.workspaces_dir, name);
    let meta =
        load_metadata(&ws_dir).map_err(|e| anyhow::anyhow!("reading workspace metadata: {}", e))?;

    let mut blockers = RemovalBlockers {
        branch: meta.branch.clone(),
        ..Default::default()
    };

    for identity in meta.repos.keys() {
        let dn = meta.dir_name(identity)?;
        let clone_dir = ws_dir.join(&dn);

        // Check for pending local changes on HEAD
        let changed = git::changed_file_count(&clone_dir).unwrap_or(0);
        if changed > 0 {
            blockers
                .hard
                .push(format!("{} (uncommitted changes)", identity));
            continue;
        }

        let wt_problems = check_linked_worktrees(&clone_dir, &ws_dir, identity);
        if !wt_problems.is_empty() {
            blockers.hard.extend(wt_problems);
            continue;
        }

        // Check if HEAD is on the wrong branch — the workspace branch may
        // have unpushed commits that the HEAD-relative checks above missed.
        let current = git::branch_current(&clone_dir).unwrap_or_default();
        if current != meta.branch && git::branch_exists(&clone_dir, &meta.branch) {
            let ws_ahead =
                git::commit_count(&clone_dir, &format!("origin/{}", meta.branch), &meta.branch)
                    .or_else(|_| {
                        // No remote tracking branch — count all commits vs default branch
                        let default = git::default_branch(&clone_dir).unwrap_or("main".into());
                        git::commit_count(&clone_dir, &format!("origin/{}", default), &meta.branch)
                    })
                    .unwrap_or(0);
            if ws_ahead > 0 {
                blockers.hard.push(format!(
                    "{} (not on workspace branch; {} has {} unpushed commit{})",
                    identity,
                    meta.branch,
                    ws_ahead,
                    if ws_ahead == 1 { "" } else { "s" }
                ));
                continue;
            }
        }

        let fetch_failed = fetch_and_propagate(&paths.mirrors_dir, &clone_dir, identity).is_err();
        if fetch_failed {
            eprintln!("  warning: fetch failed for {}, using local data", identity);
        }

        let default_branch = match git::default_branch_for_remote(&clone_dir, "origin") {
            Ok(b) => b,
            Err(_) => match git::default_branch(&clone_dir) {
                Ok(b) => b,
                Err(e) => {
                    eprintln!(
                        "  warning: cannot detect default branch for {}: {}",
                        identity, e
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

        // Also check the currently-checked-out branch when it differs from the
        // workspace branch — the user may have switched to a different branch
        // mid-workspace and that branch may have unmerged work.
        if !current.is_empty()
            && current != "HEAD"
            && current != meta.branch
            && git::validate_branch_name(&current).is_ok()
        {
            match git::branch_safety(&clone_dir, &current, &target) {
                git::BranchSafety::Merged | git::BranchSafety::SquashMerged => {}
                git::BranchSafety::PushedToRemote => {
                    // If there are local commits not yet on the remote (e.g. the
                    // user is on `main` with a stray local commit), the work is
                    // not safely on the remote — treat as a hard blocker instead.
                    let has_unpushed =
                        git::commit_count(&clone_dir, &format!("origin/{}", current), &current)
                            .unwrap_or_else(|e| {
                                eprintln!(
                                    "  warning: cannot count unpushed commits for '{}': {} \
                             (assuming unpushed)",
                                    current, e
                                );
                                1 // fail-closed
                            })
                            > 0;
                    let mut msg = if has_unpushed {
                        format!("{} (current branch '{}' is unmerged)", identity, current)
                    } else {
                        format!(
                            "{} (current branch '{}' is unmerged, but pushed to remote)",
                            identity, current
                        )
                    };
                    if fetch_failed {
                        msg.push_str(" (fetch failed, local data may be stale)");
                    }
                    if has_unpushed {
                        blockers.local_unmerged.push(msg);
                    } else {
                        blockers.pushed_unmerged.push(msg);
                    }
                }
                git::BranchSafety::Unmerged => {
                    let mut msg =
                        format!("{} (current branch '{}' is unmerged)", identity, current);
                    if fetch_failed {
                        msg.push_str(" (fetch failed, local data may be stale)");
                    }
                    blockers.local_unmerged.push(msg);
                }
            }
        }

        if !git::branch_exists(&clone_dir, &meta.branch) {
            continue;
        }
        match git::branch_safety(&clone_dir, &meta.branch, &target) {
            git::BranchSafety::Merged | git::BranchSafety::SquashMerged => {}
            git::BranchSafety::PushedToRemote => {
                let mut msg = format!("{} (unmerged branch, but pushed to remote)", identity);
                if fetch_failed {
                    msg.push_str(" (fetch failed, local data may be stale)");
                }
                blockers.pushed_unmerged.push(msg);
            }
            git::BranchSafety::Unmerged => {
                let mut msg = format!("{} (unmerged branch)", identity);
                if fetch_failed {
                    msg.push_str(" (fetch failed, local data may be stale)");
                }
                blockers.local_unmerged.push(msg);
            }
        }
    }

    // Check workspace root for user content
    let ignore_patterns = load_wspignore(
        paths
            .config_path
            .parent()
            .expect("config_path must have a parent directory"),
        &ws_dir,
    );
    match check_root_content(&ws_dir, &meta) {
        Ok(raw_problems) => {
            let root_problems = filter_ignored(&raw_problems, &ignore_patterns);
            let ignored: Vec<_> = raw_problems
                .iter()
                .filter(|p| !root_problems.iter().any(|rp| rp.path == p.path))
                .collect();
            if !ignored.is_empty() {
                let names: Vec<&str> = ignored.iter().map(|p| p.path.as_str()).collect();
                eprintln!(
                    "  note: {} root item{} suppressed by wspignore: {}",
                    ignored.len(),
                    if ignored.len() == 1 { "" } else { "s" },
                    names.join(", ")
                );
            }
            if !root_problems.is_empty() {
                let mut msg = String::from("workspace root has user content:");
                for p in &root_problems {
                    msg.push_str(&format!("\n      {}", p));
                }
                blockers.hard.push(msg);
            }
        }
        Err(e) => {
            eprintln!("  warning: root content check failed: {}", e);
        }
    }

    Ok(blockers)
}

/// Remove a workspace, returning the gc entry it became.
///
/// `None` when there was nothing to preserve: a partial workspace (a directory
/// with no `.wsp.yaml`, from an interrupted `wsp new`) is deleted outright
/// rather than moved to gc, so it is *not* recoverable. Callers must not
/// promise recovery on a `None`.
pub fn remove(paths: &Paths, name: &str, force: bool) -> Result<Option<crate::gc::GcEntry>> {
    validate_name(name)?;
    let ws_dir = dir(&paths.workspaces_dir, name);

    // Handle partial workspace: directory exists but .wsp.yaml was never written
    // (wsp new interrupted before metadata was saved). The CLI layer handles
    // confirmation for non-empty directories via --yes; by the time remove() is
    // called the user has already confirmed. Just delete it.
    let meta_path = ws_dir.join(METADATA_FILE);
    if ws_dir.exists() && !meta_path.exists() {
        fs::remove_dir_all(&ws_dir)
            .map_err(|e| anyhow::anyhow!("removing partial workspace {:?}: {}", name, e))?;
        return Ok(None);
    }

    let meta =
        load_metadata(&ws_dir).map_err(|e| anyhow::anyhow!("reading workspace metadata: {}", e))?;

    if !force {
        let blockers = check_removal_blockers(paths, name)?;
        if !blockers.is_empty() {
            let list = blockers
                .all_sorted()
                .iter()
                .map(|p| format!("\n  - {}", p))
                .collect::<String>();
            bail!(
                "workspace {:?} has unsaved work ({}):{}\n\nUse --force to remove anyway",
                name,
                meta.branch,
                list
            );
        }
    }

    // The entry, not `()`: `wsp rm` reports when the workspace expires, and
    // the authoritative removal time is the one just written to disk.
    crate::gc::move_to_gc(paths, name, &meta.branch).map(Some)
}

/// Rename result for a single repo.
#[derive(Debug)]
pub struct RenameRepoResult {
    pub name: String,
    pub old_branch: String,
    pub new_branch: String,
    pub ok: bool,
    pub error: Option<String>,
}

/// Rename a workspace: directory, metadata, and git branches in active repos.
pub fn rename(paths: &Paths, old_name: &str, new_name: &str) -> Result<Vec<RenameRepoResult>> {
    validate_name(old_name)?;
    validate_name(new_name)?;

    let old_dir = dir(&paths.workspaces_dir, old_name);
    if !old_dir.exists() {
        bail!("workspace {:?} does not exist", old_name);
    }
    let new_dir = dir(&paths.workspaces_dir, new_name);
    if new_dir.exists() {
        bail!("workspace {:?} already exists", new_name);
    }

    let meta = load_metadata(&old_dir)
        .map_err(|e| anyhow::anyhow!("reading workspace metadata: {}", e))?;

    // Derive the new branch name by replacing old_name with new_name in the branch.
    // Branch format is either "<prefix>/<name>" or just "<name>".
    let new_branch = if let Some(prefix) = meta.branch.strip_suffix(old_name) {
        // prefix includes the trailing "/" if present
        format!("{}{}", prefix, new_name)
    } else {
        // Branch was manually set or doesn't match the name pattern — just use new_name
        new_name.to_string()
    };

    git::validate_branch_name(&new_branch)?;

    let old_branch = meta.branch.clone();
    let mut results = Vec::new();

    // Rename branches in all repos
    for identity in meta.repos.keys() {
        let dn = meta.dir_name(identity)?;
        let clone_dir = old_dir.join(&dn);

        match git::branch_rename(&clone_dir, &old_branch, &new_branch) {
            Ok(()) => {
                results.push(RenameRepoResult {
                    name: dn,
                    old_branch: old_branch.clone(),
                    new_branch: new_branch.clone(),
                    ok: true,

                    error: None,
                });
            }
            Err(e) => {
                results.push(RenameRepoResult {
                    name: dn,
                    old_branch: old_branch.clone(),
                    new_branch: new_branch.clone(),
                    ok: false,

                    error: Some(e.to_string()),
                });
            }
        }
    }

    // Bail if any branch rename failed — roll back successful renames first
    let failures: Vec<&RenameRepoResult> = results.iter().filter(|r| !r.ok).collect();
    if !failures.is_empty() {
        for r in results.iter().filter(|r| r.ok) {
            let clone_dir = old_dir.join(&r.name);
            if let Err(e) = git::branch_rename(&clone_dir, &new_branch, &old_branch) {
                eprintln!("  warning: rollback failed for {}: {}", r.name, e);
            }
        }
        let msgs: Vec<String> = failures
            .iter()
            .map(|r| {
                format!(
                    "{}: {}",
                    r.name,
                    r.error.as_deref().unwrap_or("unknown error")
                )
            })
            .collect();
        bail!(
            "branch rename failed in {} repo(s), aborting:\n  {}",
            failures.len(),
            msgs.join("\n  ")
        );
    }

    // Update metadata under lock to prevent concurrent mutation data loss
    let new_name_owned = new_name.to_string();
    let new_branch_clone = new_branch.clone();
    let meta = crate::filelock::with_metadata(&old_dir, |meta| {
        meta.name = new_name_owned;
        meta.branch = new_branch_clone;
        Ok(())
    })?;

    // Rename directory
    fs::rename(&old_dir, &new_dir)?;

    // Regenerate AGENTS.md with updated metadata
    if let Err(e) = crate::agentmd::update(&new_dir, &meta) {
        eprintln!("  warning: failed to update AGENTS.md: {}", e);
    }

    // Re-run language integrations (go.work, etc.)
    let cfg = crate::config::Config::load_from(&paths.config_path).unwrap_or_default();
    crate::lang::run_integrations(&new_dir, &meta, &cfg);

    Ok(results)
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

/// Clone a repo into the workspace from its bare mirror.
///
/// Steps:
///   1. `git clone --local <mirror> <dest>` — shares objects with hardlinks
///      when supported, otherwise copies them; origin → mirror path
///   2. `git remote set-url origin <upstream_url>` — repoint to upstream
///   3. Read default branch from mirror
///   4. `git fetch <mirror_path> +refs/remotes/origin/*:refs/remotes/origin/*`
///      — populate origin refs from mirror (local-only, no network, no trace)
///   5. `git remote set-head origin <default_branch>`
///   6. Fix tracking: set-upstream-to origin/<default> or unset
///   7. Checkout workspace branch. If `origin/<branch>` exists (remote branch
///      already present, e.g. via `-b` or auto-detect), uses `--track` so
///      push/pull work without configuration. Otherwise creates a fresh local
///      branch from `origin/<default>` with no upstream tracking (intentional:
///      tracking `origin/main` would cause bare `git push` to target the wrong
///      branch).
///
/// Called from two sites: `create_inner` (workspace creation) and `add_repos`
/// (adding repos to an existing workspace). If you change this signature,
/// update both callers.
fn clone_from_mirror(
    mirrors_dir: &Path,
    ws_dir: &Path,
    identity: &str,
    dir_name: &str,
    branch: &str,
    upstream_url: &str,
    _branch_tracks_remote: bool,
) -> Result<()> {
    let parsed = parse_identity(identity)?;
    let mirror_dir = mirror::dir(mirrors_dir, &parsed);
    let dest = ws_dir.join(dir_name);

    // 1. Clone from mirror (hardlinks when supported, origin → mirror path)
    git::clone_local(&mirror_dir, &dest)?;

    // 2. Repoint origin to the real upstream URL
    if !upstream_url.is_empty() {
        git::remote_set_url(&dest, "origin", upstream_url)?;
    }

    // 3. Read default branch from mirror
    let mirror_default_br = git::default_branch_from_mirror(&mirror_dir)
        .with_context(|| format!("reading default branch from mirror for {}", identity))?;

    // 4. Populate origin/* refs from mirror (local fetch, no network).
    // Note: bare mirrors have refs/remotes/origin/* only after their first
    // upstream fetch (`git fetch` in the mirror). Before that, only
    // refs/heads/* exists (from clone_bare). Step 1's `git clone --local`
    // already creates origin/* from the mirror's refs/heads/*, so this
    // fetch is a no-op on fresh mirrors but essential for mirrors that
    // have been fetched (the normal production path).
    git::fetch_from_path(&dest, &mirror_dir, MIRROR_PROPAGATE_REFSPEC, false)?;

    // 5. Set origin/HEAD
    if let Some(ref default_br) = mirror_default_br {
        let _ = git::remote_set_head(&dest, "origin", default_br);
    }

    // 6. Fix default branch tracking and fast-forward local default branch.
    // Clone from mirror creates main tracking origin/main. Re-set explicitly,
    // then fast-forward local main to match origin/main (step 1's clone may
    // have created it from the mirror's stale HEAD).
    if let Some(ref default_br) = mirror_default_br {
        let local_ref = format!("refs/heads/{}", default_br);
        let origin_ref = format!("origin/{}", default_br);
        if git::ref_exists(&dest, &format!("refs/remotes/{}", origin_ref)) {
            let _ = git::set_upstream(&dest, default_br, &origin_ref);
            if git::is_ancestor(&dest, &local_ref, &origin_ref) {
                let _ = git::update_ref(&dest, &local_ref, &origin_ref);
            }
        } else {
            let _ = git::unset_upstream(&dest, default_br);
        }
    }

    // 7. Checkout workspace branch
    if git::branch_exists(&dest, branch) {
        git::checkout(&dest, branch)?;
        // Note: if this early-return fires during a resume (branch checked out
        // but metadata not yet written), tracking may not be set up. Narrow
        // edge case; user can run `git branch --set-upstream-to origin/<branch>`.
        return Ok(());
    }

    // If the branch already exists remotely, track it so `git pull` / `git push`
    // work without extra configuration. This covers both the explicit `-b` case
    // and the auto-detect case (branch name matches an existing remote branch).
    let remote_ref = format!("origin/{}", branch);
    if git::ref_exists(&dest, &format!("refs/remotes/{}", remote_ref)) {
        git::checkout_new_branch_tracking(&dest, branch, &remote_ref)?;
        return Ok(());
    }

    // No remote branch — the workspace branch is new locally. Don't track
    // origin/<default>: that would cause bare `git push` to target the wrong
    // branch. Devs set tracking explicitly via `git push -u` after first push.
    match mirror_default_br {
        Some(default_br) => {
            // Guard against refspec metacharacters before building the targeted refspec
            // below. The branch name originates from `git symbolic-ref` on a
            // locally-owned mirror (already validated by git), so this is defence-in-depth
            // rather than a trust boundary. A subprocess fork per clone would be wasteful;
            // an inline check for the characters that git treats as refspec wildcards is
            // sufficient and free.
            const REFSPEC_UNSAFE: &[char] = &['*', ':', '?', '[', '\\'];
            if default_br.chars().any(|c| REFSPEC_UNSAFE.contains(&c)) {
                bail!(
                    "mirror default branch {:?} contains characters that are unsafe in a git \
                     refspec; the mirror may be corrupted",
                    default_br
                );
            }
            // tracking_ref is the full canonical path used with ref_exists; start_point
            // is the short form passed to `git checkout -b`. Step 6 above uses a
            // different local named `origin_ref` holding the short form — keep them
            // distinct to avoid confusion.
            let tracking_ref = format!("refs/remotes/origin/{}", default_br);
            let start_point = format!("origin/{}", default_br);

            // tracking_ref may be absent when the mirror's refs/remotes/origin/* are
            // inconsistent with refs/heads/* (e.g. after an upstream default branch rename
            // with --prune). Attempt a targeted local re-fetch from the mirror before failing.
            if !git::ref_exists(&dest, &tracking_ref) {
                let refspec = format!(
                    "+refs/heads/{}:refs/remotes/origin/{}",
                    default_br, default_br
                );
                eprintln!(
                    "  note: {} absent from dest clone, re-fetching from mirror",
                    tracking_ref
                );
                git::fetch_from_path(&dest, &mirror_dir, &refspec, false)
                    .with_context(|| format!("re-fetch of {} from mirror failed", tracking_ref))?;
                if !git::ref_exists(&dest, &tracking_ref) {
                    bail!(
                        "cannot create workspace branch from '{}': {} is still missing after \
                         re-fetch — the mirror may be inconsistent; try `wsp doctor --fix`",
                        default_br,
                        tracking_ref
                    );
                }
            }

            git::checkout_new_branch(&dest, branch, &start_point)?;
        }
        None => {
            // Empty repo — no branches exist yet. Create an orphan branch.
            git::checkout_orphan(&dest, branch)?;
        }
    }

    Ok(())
}

/// Returns true if the git config key can execute arbitrary commands and must
/// not be applied from workspace or global configuration.
///
/// Returns true if `key` is a dangerous git config key.
///
/// Delegates to [`crate::config::validate_git_config_key`] which uses
/// prefix-based, case-insensitive matching against the canonical denylist.
/// Do not add keys here — update `config::DANGEROUS_GIT_CONFIG_KEY_PREFIXES`.
fn is_dangerous_git_config_key(key: &str) -> bool {
    crate::config::validate_git_config_key(key).is_err()
}

/// Apply git config values to repo clones in a workspace.
/// If `only` is Some, only apply to the listed identities.
/// Keys that can execute arbitrary commands are skipped with a warning.
pub fn apply_git_config(
    ws_dir: &Path,
    meta: &Metadata,
    git_config: &std::collections::BTreeMap<String, String>,
    only: Option<&[String]>,
) {
    for identity in meta.repos.keys() {
        if let Some(filter) = only
            && !filter.iter().any(|f| f == identity)
        {
            continue;
        }
        let dir_name = match meta.dir_name(identity) {
            Ok(d) => d,
            Err(_) => continue,
        };
        let repo_dir = ws_dir.join(&dir_name);
        if !repo_dir.join(".git").exists() {
            continue;
        }
        for (key, value) in git_config {
            if is_dangerous_git_config_key(key) {
                eprintln!(
                    "  warning: git config key {:?} is not allowed and was skipped for {}",
                    key, dir_name
                );
                continue;
            }
            if let Err(e) = git::set_config(&repo_dir, key, value) {
                eprintln!(
                    "  warning: git config {} = {} failed for {}: {}",
                    key, value, dir_name, e
                );
            }
        }
    }
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
        create(
            &paths,
            "test-ws",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

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
    fn test_active_repo_has_no_upstream_tracking() {
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity, String::new())]);
        create(
            &paths,
            "no-track",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "no-track");
        let clone_dir = ws_dir.join("test-repo");

        // Branch must have no upstream — a bare `git push` should not target origin/main
        let result = git::run(Some(&clone_dir), &["rev-parse", "--verify", "@{upstream}"]);
        assert!(
            result.is_err(),
            "workspace branch should have no upstream tracking"
        );
    }

    #[test]
    fn test_default_branch_tracks_origin_not_mirror() {
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity, String::new())]);
        create(
            &paths,
            "track-origin",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "track-origin");
        let clone_dir = ws_dir.join("test-repo");

        // main should track origin/main, not wsp-mirror/main
        let upstream = git::run(
            Some(&clone_dir),
            &[
                "for-each-ref",
                "--format=%(upstream:short)",
                "refs/heads/main",
            ],
        )
        .unwrap();
        assert_eq!(
            upstream, "origin/main",
            "main should track origin/main, got {:?}",
            upstream
        );
    }

    #[test]
    fn test_create_with_branch_prefix() {
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(
            &paths,
            "my-feature",
            &refs,
            Some("jganoff"),
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "my-feature");
        let meta = load_metadata(&ws_dir).unwrap();

        assert_eq!(meta.name, "my-feature");
        assert_eq!(meta.branch, "jganoff/my-feature");
        assert!(meta.repos.contains_key(&identity));
        assert!(ws_dir.join("test-repo").exists());
    }

    /// Returns the same as setup_test_env plus a remote branch that has been
    /// created in the source repo and fetched into the mirror.
    fn setup_test_env_with_remote_branch(
        branch: &str,
    ) -> (
        Paths,
        tempfile::TempDir,
        tempfile::TempDir,
        String,
        BTreeMap<String, String>,
    ) {
        let (paths, tmp_data, repo_dir, identity, upstream_urls) = setup_test_env();

        // Create the branch in the source repo and fetch it into the mirror.
        let cmds: Vec<Vec<&str>> = vec![
            vec!["git", "checkout", "-b", branch],
            vec!["git", "commit", "--allow-empty", "-m", "branch commit"],
            vec!["git", "checkout", "main"],
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

        let parsed = giturl::Parsed {
            host: "test.local".into(),
            owner: "user".into(),
            repo: "test-repo".into(),
        };
        let mirror_dir = mirror::dir(&paths.mirrors_dir, &parsed);
        git::fetch(&mirror_dir, false).unwrap();

        (paths, tmp_data, repo_dir, identity, upstream_urls)
    }

    #[test]
    fn test_create_with_branch_override() {
        let (paths, _d, _r, identity, upstream_urls) =
            setup_test_env_with_remote_branch("jfrey/new-feature");

        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(
            &paths,
            "new-feature",
            &refs,
            None,
            Some("jfrey/new-feature"),
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "new-feature");
        let meta = load_metadata(&ws_dir).unwrap();

        assert_eq!(meta.name, "new-feature");
        assert_eq!(meta.branch, "jfrey/new-feature");
        assert!(meta.repos.contains_key(&identity));
        assert!(ws_dir.join("test-repo").exists());

        // Workspace branch should be checked out
        let clone_dir = ws_dir.join("test-repo");
        let current = git::branch_current(&clone_dir).unwrap();
        assert_eq!(current, "jfrey/new-feature");
    }

    #[test]
    fn test_branch_override_tracks_remote() {
        let (paths, _d, _r, identity, upstream_urls) =
            setup_test_env_with_remote_branch("jfrey/tracked");

        let refs = BTreeMap::from([(identity, String::new())]);
        create(
            &paths,
            "tracked",
            &refs,
            None,
            Some("jfrey/tracked"),
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "tracked");
        let clone_dir = ws_dir.join("test-repo");

        // Branch must track origin/jfrey/tracked so push/pull work automatically.
        let upstream = git::run(
            Some(&clone_dir),
            &[
                "for-each-ref",
                "--format=%(upstream:short)",
                "refs/heads/jfrey/tracked",
            ],
        )
        .unwrap();
        assert_eq!(
            upstream, "origin/jfrey/tracked",
            "branch_override branch should track origin/<branch>, got {:?}",
            upstream
        );
    }

    /// Regression: when the computed branch (prefix/name) already exists remotely
    /// but no branch_override is passed, the clone must still track it.
    /// Previously this required new.rs to pass the branch as branch_override;
    /// now clone_from_mirror detects the remote ref unconditionally.
    #[test]
    fn test_computed_branch_auto_tracks_remote_without_override() {
        let (paths, _d, _r, identity, upstream_urls) =
            setup_test_env_with_remote_branch("jg/myfeature");

        let refs = BTreeMap::from([(identity, String::new())]);
        // No branch_override — simulates plain `wsp new myfeature` with prefix=jg.
        // The computed branch jg/myfeature exists remotely; clone_from_mirror
        // must detect origin/jg/myfeature and set up tracking automatically.
        create(
            &paths,
            "myfeature",
            &refs,
            Some("jg"),
            None, // no explicit override
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "myfeature");
        let clone_dir = ws_dir.join("test-repo");

        let upstream = git::run(
            Some(&clone_dir),
            &[
                "for-each-ref",
                "--format=%(upstream:short)",
                "refs/heads/jg/myfeature",
            ],
        )
        .unwrap();
        assert_eq!(
            upstream, "origin/jg/myfeature",
            "computed branch should auto-track origin/<branch> when it exists remotely"
        );
    }

    /// When `wsp new <name>` passes an explicit branch_override, tracking still works.
    #[test]
    fn test_auto_track_computed_branch_with_prefix() {
        let (paths, _d, _r, identity, upstream_urls) =
            setup_test_env_with_remote_branch("jg/myfeature");

        let refs = BTreeMap::from([(identity, String::new())]);
        create(
            &paths,
            "myfeature",
            &refs,
            Some("jg"),
            Some("jg/myfeature"), // explicit override also tracks correctly
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "myfeature");
        let clone_dir = ws_dir.join("test-repo");

        let upstream = git::run(
            Some(&clone_dir),
            &[
                "for-each-ref",
                "--format=%(upstream:short)",
                "refs/heads/jg/myfeature",
            ],
        )
        .unwrap();
        assert_eq!(
            upstream, "origin/jg/myfeature",
            "auto-tracked branch should track origin/<branch>"
        );
    }

    #[test]
    fn test_create_with_empty_branch_prefix() {
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(
            &paths,
            "empty-prefix",
            &refs,
            Some(""),
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "empty-prefix");
        let meta = load_metadata(&ws_dir).unwrap();

        assert_eq!(meta.branch, "empty-prefix");
    }

    #[test]
    fn test_create_with_empty_repo() {
        // An empty repo has no commits or branches. wsp should handle this
        // by creating an orphan branch instead of branching off origin/main.
        let tmp_data = tempfile::tempdir().unwrap();
        let tmp_home = tempfile::tempdir().unwrap();
        let data_dir = tmp_data.path().join("wsp");
        let workspaces_dir = tmp_home.path().join("dev").join("workspaces");
        fs::create_dir_all(&workspaces_dir).unwrap();
        let paths = Paths::from_dirs(&data_dir, &workspaces_dir);

        // Create an empty bare repo (git init --bare — no commits)
        let parsed = giturl::Parsed {
            host: "test.local".into(),
            owner: "user".into(),
            repo: "empty-repo".into(),
        };
        let mirror_dir = mirror::dir(&paths.mirrors_dir, &parsed);
        fs::create_dir_all(&mirror_dir).unwrap();
        let output = Command::new("git")
            .args(["init", "--bare"])
            .current_dir(&mirror_dir)
            .output()
            .unwrap();
        assert!(output.status.success());

        let identity = parsed.identity();
        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        let upstream_urls = BTreeMap::from([(identity.clone(), String::new())]);

        create(
            &paths,
            "empty-ws",
            &refs,
            Some("jganoff"),
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "empty-ws");
        let meta = load_metadata(&ws_dir).unwrap();
        assert_eq!(meta.branch, "jganoff/empty-ws");

        // The clone should exist and be on the orphan branch
        let clone_dir = ws_dir.join(meta.dir_name(&identity).unwrap());
        let head = git::run(Some(&clone_dir), &["symbolic-ref", "--short", "HEAD"]).unwrap();
        assert_eq!(head, "jganoff/empty-ws");
    }

    #[test]
    fn test_create_fails_closed_when_nonempty_mirror_head_is_missing() {
        let (paths, _data, _source, identity, upstream_urls) = setup_test_env();
        let parsed = parse_identity(&identity).unwrap();
        let mirror_dir = mirror::dir(&paths.mirrors_dir, &parsed);
        git::run(
            Some(&mirror_dir),
            &["symbolic-ref", "--delete", "refs/remotes/origin/HEAD"],
        )
        .unwrap();
        git::run(
            Some(&mirror_dir),
            &["symbolic-ref", "HEAD", "refs/heads/missing"],
        )
        .unwrap();

        let refs = BTreeMap::from([(identity, String::new())]);
        let err = create(
            &paths,
            "broken-head",
            &refs,
            Some("jganoff"),
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap_err();

        assert!(
            err.to_string()
                .contains("reading default branch from mirror"),
            "unexpected error: {err:#}"
        );
        assert!(
            !dir(&paths.workspaces_dir, "broken-head").exists(),
            "failed creation must clean up the partial workspace"
        );
    }

    fn setup_non_main_mirror(
        paths: &Paths,
        branch: &str,
        repo_name: &str,
    ) -> (
        tempfile::TempDir,
        String,
        BTreeMap<String, String>,
        BTreeMap<String, String>,
    ) {
        let repo_dir = tempfile::tempdir().unwrap();
        let init_branch = format!("--initial-branch={}", branch);
        let cmds: Vec<Vec<&str>> = vec![
            vec!["git", "init", &init_branch],
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

        let parsed = giturl::Parsed {
            host: "test.local".into(),
            owner: "user".into(),
            repo: repo_name.into(),
        };
        mirror::clone(
            &paths.mirrors_dir,
            &parsed,
            repo_dir.path().to_str().unwrap(),
        )
        .unwrap();

        let mirror_dir = mirror::dir(&paths.mirrors_dir, &parsed);
        let head_target = format!("refs/heads/{}", branch);
        let output = Command::new("git")
            .args(["symbolic-ref", "refs/remotes/origin/HEAD", &head_target])
            .current_dir(&mirror_dir)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "setting HEAD ref: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let identity = parsed.identity();
        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        let upstream_urls = BTreeMap::from([(
            identity.clone(),
            repo_dir.path().to_str().unwrap().to_string(),
        )]);
        (repo_dir, identity, refs, upstream_urls)
    }

    #[test]
    fn test_create_with_non_main_default_branch() {
        let tmp_data = tempfile::tempdir().unwrap();
        let tmp_home = tempfile::tempdir().unwrap();
        let data_dir = tmp_data.path().join("wsp");
        let workspaces_dir = tmp_home.path().join("dev").join("workspaces");
        fs::create_dir_all(&workspaces_dir).unwrap();
        let paths = Paths::from_dirs(&data_dir, &workspaces_dir);

        let (_repo_dir, identity, refs, upstream_urls) =
            setup_non_main_mirror(&paths, "master", "master-repo");

        create(
            &paths,
            "master-ws",
            &refs,
            Some("jganoff"),
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "master-ws");
        let meta = load_metadata(&ws_dir).unwrap();
        assert_eq!(meta.branch, "jganoff/master-ws");

        let clone_dir = ws_dir.join(meta.dir_name(&identity).unwrap());
        let head = git::run(Some(&clone_dir), &["symbolic-ref", "--short", "HEAD"]).unwrap();
        assert_eq!(head, "jganoff/master-ws");

        // Workspace branch must be descended from master, not an orphan.
        git::run(
            Some(&clone_dir),
            &["merge-base", "--is-ancestor", "origin/master", "HEAD"],
        )
        .expect("workspace branch should be descended from origin/master");
    }

    #[test]
    fn test_create_with_slash_default_branch() {
        // Branch names containing slashes (e.g. release/2.x) must parse correctly;
        // the old split('/').last() parser truncated them to just '2.x'.
        let tmp_data = tempfile::tempdir().unwrap();
        let tmp_home = tempfile::tempdir().unwrap();
        let data_dir = tmp_data.path().join("wsp");
        let workspaces_dir = tmp_home.path().join("dev").join("workspaces");
        fs::create_dir_all(&workspaces_dir).unwrap();
        let paths = Paths::from_dirs(&data_dir, &workspaces_dir);

        let (_repo_dir, identity, refs, upstream_urls) =
            setup_non_main_mirror(&paths, "release/2.x", "slash-branch-repo");

        create(
            &paths,
            "slash-ws",
            &refs,
            Some("jganoff"),
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "slash-ws");
        let meta = load_metadata(&ws_dir).unwrap();
        assert_eq!(meta.branch, "jganoff/slash-ws");

        let clone_dir = ws_dir.join(meta.dir_name(&identity).unwrap());
        let head = git::run(Some(&clone_dir), &["symbolic-ref", "--short", "HEAD"]).unwrap();
        assert_eq!(head, "jganoff/slash-ws");

        git::run(
            Some(&clone_dir),
            &["merge-base", "--is-ancestor", "origin/release/2.x", "HEAD"],
        )
        .expect("workspace branch should be descended from origin/release/2.x");
    }

    #[test]
    fn test_add_repos_with_non_main_default_branch() {
        // add_repos calls clone_from_mirror through the same path as create_inner.
        // Verify that adding a repo with a non-main default branch works correctly.
        let (paths, _d, _r, main_identity, main_upstream_urls) = setup_test_env();

        let main_refs = BTreeMap::from([(main_identity.clone(), String::new())]);
        create(
            &paths,
            "add-non-main-ws",
            &main_refs,
            Some("jganoff"),
            None,
            &main_upstream_urls,
            None,
            None,
        )
        .unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "add-non-main-ws");

        let (_repo_dir, identity, refs, upstream_urls) =
            setup_non_main_mirror(&paths, "master", "master-add-repo");

        add_repos(&paths.mirrors_dir, &ws_dir, &refs, &upstream_urls, false).unwrap();

        let meta = load_metadata(&ws_dir).unwrap();
        let clone_dir = ws_dir.join(meta.dir_name(&identity).unwrap());
        let head = git::run(Some(&clone_dir), &["symbolic-ref", "--short", "HEAD"]).unwrap();
        assert_eq!(head, "jganoff/add-non-main-ws");

        git::run(
            Some(&clone_dir),
            &["merge-base", "--is-ancestor", "origin/master", "HEAD"],
        )
        .expect("added repo branch should be descended from origin/master");
    }

    #[test]
    fn test_add_repos_with_slash_default_branch() {
        // add_repos must not truncate slash-containing branch names (e.g. release/2.x).
        let (paths, _d, _r, main_identity, main_upstream_urls) = setup_test_env();

        let main_refs = BTreeMap::from([(main_identity.clone(), String::new())]);
        create(
            &paths,
            "add-slash-ws",
            &main_refs,
            Some("jganoff"),
            None,
            &main_upstream_urls,
            None,
            None,
        )
        .unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "add-slash-ws");

        let (_repo_dir, identity, refs, upstream_urls) =
            setup_non_main_mirror(&paths, "release/2.x", "slash-add-repo");

        add_repos(&paths.mirrors_dir, &ws_dir, &refs, &upstream_urls, false).unwrap();

        let meta = load_metadata(&ws_dir).unwrap();
        let clone_dir = ws_dir.join(meta.dir_name(&identity).unwrap());
        let head = git::run(Some(&clone_dir), &["symbolic-ref", "--short", "HEAD"]).unwrap();
        assert_eq!(head, "jganoff/add-slash-ws");

        git::run(
            Some(&clone_dir),
            &["merge-base", "--is-ancestor", "origin/release/2.x", "HEAD"],
        )
        .expect("added repo branch should be descended from origin/release/2.x");
    }

    #[test]
    fn test_create_fails_when_mirror_default_branch_is_missing() {
        // Simulates a mirror where refs/remotes/origin/HEAD was updated (e.g. after
        // an upstream rename + prune) to point to a branch that no longer exists
        // in refs/heads/. create() must return Err rather than silently succeeding
        // or panicking.
        let tmp_data = tempfile::tempdir().unwrap();
        let tmp_home = tempfile::tempdir().unwrap();
        let data_dir = tmp_data.path().join("wsp");
        let workspaces_dir = tmp_home.path().join("dev").join("workspaces");
        fs::create_dir_all(&workspaces_dir).unwrap();
        let paths = Paths::from_dirs(&data_dir, &workspaces_dir);

        let (_repo_dir, _identity, refs, upstream_urls) =
            setup_non_main_mirror(&paths, "master", "inconsistent-repo");

        // Point the mirror's HEAD to a branch that was never fetched into refs/heads/.
        let parsed = giturl::Parsed {
            host: "test.local".into(),
            owner: "user".into(),
            repo: "inconsistent-repo".into(),
        };
        let mirror_dir = mirror::dir(&paths.mirrors_dir, &parsed);
        let out = Command::new("git")
            .args([
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/heads/nonexistent-branch",
            ])
            .current_dir(&mirror_dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "corrupting HEAD symref: {}",
            String::from_utf8_lossy(&out.stderr)
        );

        let result = create(
            &paths,
            "bad-ws",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        );
        assert!(
            result.is_err(),
            "create should fail when mirror HEAD points to a nonexistent branch"
        );
        let msg = format!("{:#}", result.unwrap_err());
        assert!(
            msg.contains("re-fetch") || msg.contains("nonexistent"),
            "error should mention the failed re-fetch: {}",
            msg
        );
    }

    #[test]
    fn test_create_fails_when_mirror_default_branch_has_refspec_metacharacter() {
        // Simulates a corrupted mirror whose HEAD resolves to a branch name containing
        // '*', which would turn the targeted refspec into a glob fetching all refs.
        let tmp_data = tempfile::tempdir().unwrap();
        let tmp_home = tempfile::tempdir().unwrap();
        let data_dir = tmp_data.path().join("wsp");
        let workspaces_dir = tmp_home.path().join("dev").join("workspaces");
        fs::create_dir_all(&workspaces_dir).unwrap();
        let paths = Paths::from_dirs(&data_dir, &workspaces_dir);

        let (_repo_dir, _identity, refs, upstream_urls) =
            setup_non_main_mirror(&paths, "master", "wildcard-repo");

        let parsed = giturl::Parsed {
            host: "test.local".into(),
            owner: "user".into(),
            repo: "wildcard-repo".into(),
        };
        let mirror_dir = mirror::dir(&paths.mirrors_dir, &parsed);
        // Write the symref file directly: `*` is not a valid git ref name so
        // `git symbolic-ref` would reject it, but git reads the file as-is.
        // strip_ref_branch will strip `refs/heads/` leaving `*`, which the
        // inline metacharacter guard in clone_from_mirror must reject.
        let head_path = mirror_dir
            .join("refs")
            .join("remotes")
            .join("origin")
            .join("HEAD");
        fs::write(&head_path, "ref: refs/heads/*\n").unwrap();

        let result = create(
            &paths,
            "wildcard-ws",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        );
        assert!(
            result.is_err(),
            "create should fail when mirror HEAD contains a refspec metacharacter"
        );
        let msg = format!("{:#}", result.unwrap_err());
        assert!(
            msg.contains("unsafe") || msg.contains("metacharacter") || msg.contains("corrupted"),
            "error should mention the unsafe branch name: {}",
            msg
        );
    }

    #[test]
    fn test_create_duplicate() {
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(
            &paths,
            "test-ws-dup",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();
        assert!(
            create(
                &paths,
                "test-ws-dup",
                &refs,
                None,
                None,
                &upstream_urls,
                None,
                None
            )
            .is_err()
        );
    }

    #[test]
    fn test_local_default_branch_matches_origin_after_create() {
        let (paths, _d, source_repo, identity, upstream_urls) = setup_test_env();

        // Add a commit to upstream after mirror was cloned, then fetch into mirror
        // so the mirror is ahead of what the initial bare clone had.
        let output = Command::new("git")
            .args(["commit", "--allow-empty", "-m", "upstream advance"])
            .current_dir(source_repo.path())
            .output()
            .unwrap();
        assert!(output.status.success());

        let parsed = giturl::Parsed::from_identity(&identity).unwrap();
        mirror::fetch(&paths.mirrors_dir, &parsed).unwrap();

        // Create workspace — local main should be fast-forwarded to origin/main
        let refs = BTreeMap::from([(identity, String::new())]);
        create(
            &paths,
            "ff-test",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

        let clone_dir = dir(&paths.workspaces_dir, "ff-test").join("test-repo");

        let local_main = git::run(Some(&clone_dir), &["rev-parse", "refs/heads/main"]).unwrap();
        let origin_main =
            git::run(Some(&clone_dir), &["rev-parse", "refs/remotes/origin/main"]).unwrap();

        assert_eq!(
            local_main, origin_main,
            "local main should match origin/main after create"
        );
    }

    #[test]
    fn test_detect() {
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity, String::new())]);
        create(
            &paths,
            "test-ws-detect",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

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
    fn test_detect_skips_per_repo_wsp_yaml() {
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity, String::new())]);
        create(
            &paths,
            "test-ws-detect-skip",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "test-ws-detect-skip");
        let repo_dir = ws_dir.join("test-repo");

        // Place a per-repo .wsp.yaml (template format, no name/branch) inside
        // the repo clone. detect() should walk past it to the workspace root.
        fs::write(
            repo_dir.join(".wsp.yaml"),
            "setup_commands:\n- echo hello\n",
        )
        .unwrap();

        let found = detect(&repo_dir).unwrap();
        assert_eq!(found, ws_dir, "detect should skip per-repo .wsp.yaml");
    }

    #[test]
    fn test_detect_not_in_workspace() {
        // Use a directory that has no .wsp.yaml anywhere in its ancestor chain.
        // We create a fresh workspace dir via setup_test_env (which uses a temp
        // dir), then verify a *sibling* directory (which has no .wsp.yaml) is
        // not detected as a workspace.
        let tmp = tempfile::tempdir().unwrap();
        let not_a_ws = tmp.path().join("plain-dir");
        std::fs::create_dir_all(&not_a_ws).unwrap();
        // Confirm the temp root itself has no .wsp.yaml (if it does, the
        // environment is polluted and we skip rather than fail spuriously).
        if detect(tmp.path()).is_ok() {
            // The tmpdir is nested inside an existing workspace (environment
            // artifact). Skip this assertion — it cannot be meaningfully tested
            // in this environment.
            return;
        }
        assert!(
            detect(&not_a_ws).is_err(),
            "a plain directory with no .wsp.yaml should not be detected as a workspace"
        );
    }

    #[test]
    fn test_remove_merged_workspace() {
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(
            &paths,
            "rm-merged",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

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
        create(
            &paths,
            "rm-origin-ahead",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

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
        create(
            &paths,
            "rm-unmerged",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

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
            err.contains("unsaved work"),
            "expected 'unsaved work' in error: {}",
            err
        );

        // Workspace should still exist
        assert!(ws_dir.exists());
    }

    #[test]
    fn test_remove_force_deletes_unmerged() {
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(
            &paths,
            "rm-force",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

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
    fn test_remove_blocks_pending_changes() {
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity, String::new())]);
        create(
            &paths,
            "rm-dirty",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "rm-dirty");
        let repo_dir = ws_dir.join("test-repo");
        fs::write(repo_dir.join("dirty.txt"), "x").unwrap();

        let result = remove(&paths, "rm-dirty", false);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("uncommitted changes"),
            "expected 'uncommitted changes' in error: {}",
            err
        );
        assert!(ws_dir.exists());
    }

    #[test]
    fn test_remove_blocks_dirty_linked_worktree() {
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity, String::new())]);
        create(
            &paths,
            "rm-wt",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "rm-wt");
        let repo_dir = ws_dir.join("test-repo");

        // Create a linked worktree on a new branch
        let wt_dir = ws_dir.join("side-work");
        let out = std::process::Command::new("git")
            .args(["worktree", "add", wt_dir.to_str().unwrap(), "-b", "side"])
            .current_dir(&repo_dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git worktree add: {}",
            String::from_utf8_lossy(&out.stderr)
        );

        // Make a dirty change in the linked worktree (not the main working tree)
        fs::write(wt_dir.join("dirty.txt"), "x").unwrap();

        // remove must block — the main working tree is clean, but the linked worktree isn't
        let result = remove(&paths, "rm-wt", false);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("linked worktree") && err.contains("uncommitted changes"),
            "expected linked worktree error, got: {}",
            err
        );
        assert!(ws_dir.exists(), "workspace directory must not be removed");
    }

    #[test]
    fn test_remove_blocks_unpushed_commits_in_linked_worktree() {
        // A linked worktree on a local-only branch (no upstream) with a commit
        // must block removal even though the working tree is clean.
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity, String::new())]);
        create(
            &paths,
            "rm-wt-ahead",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "rm-wt-ahead");
        let repo_dir = ws_dir.join("test-repo");
        let wt_dir = ws_dir.join("side-work");

        // Create a linked worktree on a new local-only branch
        let out = Command::new("git")
            .args([
                "worktree",
                "add",
                wt_dir.to_str().unwrap(),
                "-b",
                "local-only",
            ])
            .current_dir(&repo_dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );

        // Commit in the linked worktree (working tree will be clean after commit)
        crate::testutil::local_commit(&wt_dir, "work.txt", "important work");

        // remove must block — committed but unpushed on a local-only branch
        let result = remove(&paths, "rm-wt-ahead", false);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("linked worktree") && err.contains("unpushed"),
            "expected unpushed commit error, got: {}",
            err
        );
        assert!(ws_dir.exists());
    }

    #[test]
    fn test_remove_repos_blocks_dirty_linked_worktree() {
        // Verify remove_repos also checks linked worktrees (separate code path).
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(
            &paths,
            "rm-repos-wt",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "rm-repos-wt");
        let repo_dir = ws_dir.join("test-repo");
        let wt_dir = ws_dir.join("side-work");

        let out = Command::new("git")
            .args(["worktree", "add", wt_dir.to_str().unwrap(), "-b", "side"])
            .current_dir(&repo_dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );

        fs::write(wt_dir.join("dirty.txt"), "x").unwrap();

        let result = remove_repos(&paths.mirrors_dir, &ws_dir, &[identity], false);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("linked worktree"),
            "expected linked worktree error, got: {}",
            err
        );
    }

    #[test]
    fn test_remove_blocks_dirty_detached_head_linked_worktree() {
        // A linked worktree in detached HEAD state with uncommitted changes
        // must block removal — exercises the `branch: None` path.
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity, String::new())]);
        create(
            &paths,
            "rm-wt-detach",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "rm-wt-detach");
        let repo_dir = ws_dir.join("test-repo");
        let wt_dir = ws_dir.join("detach-work");

        // `--detach` creates a linked worktree in detached HEAD state
        let out = Command::new("git")
            .args(["worktree", "add", "--detach", wt_dir.to_str().unwrap()])
            .current_dir(&repo_dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git worktree add --detach: {}",
            String::from_utf8_lossy(&out.stderr)
        );

        // Dirty change in detached HEAD worktree
        fs::write(wt_dir.join("dirty.txt"), "x").unwrap();

        let result = remove(&paths, "rm-wt-detach", false);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("linked worktree") && err.contains("uncommitted changes"),
            "expected linked worktree uncommitted-changes error, got: {}",
            err
        );
        assert!(ws_dir.exists(), "workspace must not be removed");
    }

    #[test]
    fn test_check_linked_worktrees_clean_inside_ws_dir_no_problems() {
        // A clean linked worktree inside ws_dir must produce no problems from
        // check_linked_worktrees — guard against false positives in the helper.
        // Tested via direct call to avoid check_root_content interference
        // (which would flag the worktree dir as unexpected workspace content).
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(
            &paths,
            "wt-clean-unit",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "wt-clean-unit");
        let repo_dir = ws_dir.join("test-repo");
        let wt_dir = ws_dir.join("side-clean");

        let out = Command::new("git")
            .args([
                "worktree",
                "add",
                wt_dir.to_str().unwrap(),
                "-b",
                "clean-side",
            ])
            .current_dir(&repo_dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );

        // Call the helper directly — no dirty/unpushed work, worktree is inside ws_dir
        let problems = check_linked_worktrees(&repo_dir, &ws_dir, &identity);
        assert!(
            problems.is_empty(),
            "clean inside-ws_dir linked worktree should produce no problems: {:?}",
            problems
        );
    }

    #[test]
    fn test_remove_blocks_external_clean_linked_worktree() {
        // A clean linked worktree outside ws_dir must block removal — moving
        // ws_dir to gc would orphan the worktree (its .git file would point to
        // a deleted gitdir). Requires --force to proceed.
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity, String::new())]);
        create(
            &paths,
            "rm-wt-ext",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "rm-wt-ext");
        let repo_dir = ws_dir.join("test-repo");

        // Place the linked worktree outside ws_dir — simulates `git worktree add ~/tmp/quick-fix`
        let wt_dir = paths.workspaces_dir.join("rm-wt-ext-side");
        let out = Command::new("git")
            .args([
                "worktree",
                "add",
                wt_dir.to_str().unwrap(),
                "-b",
                "ext-side",
            ])
            .current_dir(&repo_dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );

        // Must block even though the worktree is clean
        let result = remove(&paths, "rm-wt-ext", false);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("orphaned"),
            "expected orphan warning, got: {}",
            err
        );
        assert!(ws_dir.exists(), "workspace must not be removed");
    }

    #[test]
    fn test_remove_ignores_prunable_worktree_without_pruning() {
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(
            &paths,
            "rm-wt-prunable",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "rm-wt-prunable");
        let repo_dir = ws_dir.join("test-repo");
        let wt_dir = paths.workspaces_dir.join("rm-wt-prunable-side");
        let out = Command::new("git")
            .args([
                "worktree",
                "add",
                wt_dir.to_str().unwrap(),
                "-b",
                "prunable-side",
            ])
            .current_dir(&repo_dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );

        // Simulate an agent-created temporary worktree whose directory was
        // deleted without unregistering it. The safety check must observe but
        // must not prune the stale Git metadata.
        fs::remove_dir_all(&wt_dir).unwrap();
        let before = git::list_linked_worktrees(&repo_dir).unwrap();
        assert_eq!(before.len(), 1);
        assert!(before[0].prunable);

        let problems = check_linked_worktrees(&repo_dir, &ws_dir, &identity);
        assert!(problems.is_empty());
        let after = git::list_linked_worktrees(&repo_dir).unwrap();
        assert_eq!(after.len(), 1, "safety check must not prune the entry");
        assert!(after[0].prunable);

        // If another safety check blocks removal, `wsp rm` must not alter the
        // stale worktree registration.
        let dirty_file = repo_dir.join("uncommitted.txt");
        fs::write(&dirty_file, "uncommitted work").unwrap();
        let result = remove(&paths, "rm-wt-prunable", false);
        assert!(result.is_err());
        let after_blocked_remove = git::list_linked_worktrees(&repo_dir).unwrap();
        assert_eq!(after_blocked_remove.len(), 1);
        assert!(after_blocked_remove[0].prunable);

        fs::remove_file(dirty_file).unwrap();
        remove(&paths, "rm-wt-prunable", false).unwrap();
        assert!(!ws_dir.exists());
    }

    // Tests wrong-branch detection: user checked out main locally but the
    // workspace branch has local-only commits not yet pushed.
    #[test]
    fn test_remove_blocks_wrong_branch_with_unpushed_commits() {
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(
            &paths,
            "rm-wrong-branch",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "rm-wrong-branch");
        let repo_dir = ws_dir.join("test-repo");

        // Configure git identity for commits
        for args in &[
            vec!["git", "config", "user.email", "test@test.com"],
            vec!["git", "config", "user.name", "Test"],
            vec!["git", "config", "commit.gpgsign", "false"],
        ] {
            let out = Command::new(args[0])
                .args(&args[1..])
                .current_dir(&repo_dir)
                .output()
                .unwrap();
            assert!(out.status.success());
        }

        // Commit something on the workspace branch (do NOT push)
        fs::write(repo_dir.join("wip.txt"), "work in progress").unwrap();
        let out = Command::new("git")
            .args(["add", "wip.txt"])
            .current_dir(&repo_dir)
            .output()
            .unwrap();
        assert!(out.status.success());
        let out = Command::new("git")
            .args(["commit", "-m", "wip"])
            .current_dir(&repo_dir)
            .output()
            .unwrap();
        assert!(out.status.success());

        // Switch to main — HEAD is now on a different branch than the workspace branch
        let out = Command::new("git")
            .args(["checkout", "main"])
            .current_dir(&repo_dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );

        // remove must block: the workspace branch has unpushed commits even though
        // HEAD is clean on main.
        let result = remove(&paths, "rm-wrong-branch", false);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("not on workspace branch"),
            "expected 'not on workspace branch' in error: {}",
            err
        );
        assert!(ws_dir.exists());
    }

    #[test]
    fn test_list_all() {
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();

        // Initially empty
        let names = list_all(&paths.workspaces_dir).unwrap();
        assert!(names.is_empty());

        // Create a workspace
        let refs = BTreeMap::from([(identity, String::new())]);
        create(
            &paths,
            "ws-1-list",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

        let names = list_all(&paths.workspaces_dir).unwrap();
        assert_eq!(names, vec!["ws-1-list"]);
    }

    #[test]
    fn test_save_and_load_metadata_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let meta = Metadata {
            version: CURRENT_METADATA_VERSION,
            name: "my-ws".into(),
            branch: "my-ws".into(),
            repos: BTreeMap::from([
                ("github.com/user/repo-a".into(), None),
                ("github.com/user/repo-b".into(), None),
            ]),
            created: Utc::now(),
            description: None,
            last_used: None,
            created_from: None,
            dirs: BTreeMap::new(),
            config: None,
            setup_commands: std::collections::BTreeMap::new(),
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
            version: CURRENT_METADATA_VERSION,
            name: "my-ws".into(),
            branch: "my-ws".into(),
            repos: BTreeMap::from([
                ("github.com/acme/api-gateway".into(), None),
                (
                    "github.com/acme/user-service".into(),
                    Some(WorkspaceRepoRef {
                        r#ref: "main".into(),
                        url: None,
                    }),
                ),
                (
                    "github.com/acme/proto".into(),
                    Some(WorkspaceRepoRef {
                        r#ref: "v1.0".into(),
                        url: None,
                    }),
                ),
            ]),
            created: Utc::now(),
            description: None,
            last_used: None,
            created_from: None,
            dirs: BTreeMap::new(),
            config: None,
            setup_commands: std::collections::BTreeMap::new(),
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
    fn test_created_from_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let meta = Metadata {
            version: CURRENT_METADATA_VERSION,
            name: "my-ws".into(),
            branch: "my-ws".into(),
            repos: BTreeMap::from([("github.com/user/repo-a".into(), None)]),
            created: Utc::now(),
            description: None,
            last_used: None,
            created_from: Some("backend".into()),
            dirs: BTreeMap::new(),
            config: None,
            setup_commands: std::collections::BTreeMap::new(),
        };

        save_metadata(tmp.path(), &meta).unwrap();
        let loaded = load_metadata(tmp.path()).unwrap();

        assert_eq!(loaded.created_from.as_deref(), Some("backend"));
    }

    #[test]
    fn test_validate_name() {
        let cases = vec![
            ("valid", "my-feature", false),
            ("valid with dots", "fix.bug", false),
            ("valid with underscore", "my_feature", false),
            ("valid uppercase", "My-Feature", false),
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
            ("space", "my feature", true),
            ("dollar sign", "test$var", true),
            ("backtick", "test`cmd`", true),
            ("command substitution", "test$(whoami)", true),
            ("single quote", "it's", true),
            ("double quote", "say\"hi\"", true),
            ("semicolon", "a;b", true),
            ("pipe", "a|b", true),
            ("ampersand", "a&b", true),
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
    fn test_validate_dir_name() {
        let cases = vec![
            ("valid simple", "repo-a", false),
            ("valid with owner prefix", "acme-utils", false),
            ("empty", "", true),
            ("forward slash", "a/b", true),
            ("backslash", "a\\b", true),
            ("null byte", "bad\0name", true),
            ("dotdot", "..", true),
            ("dot", ".", true),
            ("contains dotdot — valid name", "foo..bar", false),
            ("contains dotdot — valid name 2", "acme..corp", false),
            ("path traversal prefix", "../etc", true),
            ("path traversal mid", "a/../../etc", true),
            ("path traversal suffix", "a/..", true),
            ("absolute path", "/etc/passwd", true),
        ];
        for (name, input, want_err) in cases {
            let result = validate_dir_name(input);
            if want_err {
                assert!(result.is_err(), "{}: expected error", name);
            } else {
                assert!(result.is_ok(), "{}: unexpected error: {:?}", name, result);
            }
        }
    }

    #[test]
    fn test_load_metadata_rejects_traversal_in_dirs() {
        let cases = vec![
            ("path separator", "../../.ssh", "path separators"),
            ("dotdot", "..", "path traversal"),
        ];
        for (name, dir_val, expected_msg) in cases {
            let tmp = tempfile::tempdir().unwrap();
            let yaml = format!(
                "name: evil-ws\nbranch: evil-ws\nrepos:\n  github.com/acme/api:\ncreated: '2024-01-01T00:00:00Z'\ndirs:\n  github.com/acme/api: '{}'\n",
                dir_val
            );
            fs::write(tmp.path().join(METADATA_FILE), &yaml).unwrap();

            let result = load_metadata(tmp.path());
            assert!(result.is_err(), "{}: expected error", name);
            let err = result.unwrap_err().to_string();
            assert!(
                err.contains(expected_msg),
                "{}: expected {:?} in error: {}",
                name,
                expected_msg,
                err
            );
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
        let result = create(
            &paths,
            "fail-ws",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        );
        assert!(result.is_err());

        // Workspace dir should have been cleaned up
        let ws_dir = workspaces_dir.join("fail-ws");
        assert!(
            !ws_dir.exists(),
            "workspace dir should be cleaned up on failure"
        );
    }

    #[test]
    fn test_add_repos_to_existing_workspace() {
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();

        // Create workspace with active repo
        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(
            &paths,
            "add-ws",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "add-ws");

        // Try adding the same repo again — should skip
        add_repos(&paths.mirrors_dir, &ws_dir, &refs, &upstream_urls, false).unwrap();

        let meta = load_metadata(&ws_dir).unwrap();
        assert_eq!(meta.repos.len(), 1);
    }

    #[test]
    fn test_add_repos_cleans_partial_clone_when_mirror_head_is_missing() {
        let (paths, _d, source_repo, identity1, mut upstream_urls) = setup_test_env();
        let refs = BTreeMap::from([(identity1.clone(), String::new())]);
        create(
            &paths,
            "add-broken-head",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();
        let ws_dir = dir(&paths.workspaces_dir, "add-broken-head");

        let (identity2, urls2) = add_mirror_with_owner(
            &paths,
            source_repo.path(),
            "test.local",
            "other",
            "broken-repo",
        );
        upstream_urls.extend(urls2);
        let parsed = parse_identity(&identity2).unwrap();
        let mirror_dir = mirror::dir(&paths.mirrors_dir, &parsed);
        git::run(
            Some(&mirror_dir),
            &["symbolic-ref", "--delete", "refs/remotes/origin/HEAD"],
        )
        .unwrap();
        git::run(
            Some(&mirror_dir),
            &["symbolic-ref", "HEAD", "refs/heads/missing"],
        )
        .unwrap();

        let add_refs = BTreeMap::from([(identity2.clone(), String::new())]);
        let err = add_repos(
            &paths.mirrors_dir,
            &ws_dir,
            &add_refs,
            &upstream_urls,
            false,
        )
        .unwrap_err();

        assert!(
            err.to_string()
                .contains("reading default branch from mirror"),
            "unexpected error: {err:#}"
        );
        assert!(
            !ws_dir.join("broken-repo").exists(),
            "failed addition must remove its partial clone"
        );
        let meta = load_metadata(&ws_dir).unwrap();
        assert_eq!(meta.repos.len(), 1);
        assert!(meta.repos.contains_key(&identity1));
        assert!(!meta.repos.contains_key(&identity2));
    }

    #[test]
    fn test_add_repo_has_no_upstream_tracking() {
        let (paths, _d, source_repo, identity1, mut upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity1, String::new())]);
        create(
            &paths,
            "add-no-track",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "add-no-track");

        // Add a second repo via add_repos
        let (identity2, urls2) = add_mirror_with_owner(
            &paths,
            source_repo.path(),
            "test.local",
            "other",
            "added-repo",
        );
        upstream_urls.extend(urls2);

        let add_refs = BTreeMap::from([(identity2, String::new())]);
        add_repos(
            &paths.mirrors_dir,
            &ws_dir,
            &add_refs,
            &upstream_urls,
            false,
        )
        .unwrap();

        let clone_dir = ws_dir.join("added-repo");
        let result = git::run(Some(&clone_dir), &["rev-parse", "--verify", "@{upstream}"]);
        assert!(
            result.is_err(),
            "repo added via add_repos should have no upstream tracking"
        );
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
            version: CURRENT_METADATA_VERSION,
            name: "test".into(),
            branch: "test".into(),
            repos: BTreeMap::from([("github.com/acme/utils".into(), None)]),
            created: Utc::now(),
            description: None,
            last_used: None,
            created_from: None,
            dirs: BTreeMap::from([("github.com/acme/utils".into(), "acme-utils".into())]),
            config: None,
            setup_commands: std::collections::BTreeMap::new(),
        };
        assert_eq!(
            meta.dir_name("github.com/acme/utils").unwrap(),
            "acme-utils"
        );
    }

    #[test]
    fn test_dir_name_without_override() {
        let meta = Metadata {
            version: CURRENT_METADATA_VERSION,
            name: "test".into(),
            branch: "test".into(),
            repos: BTreeMap::from([("github.com/acme/utils".into(), None)]),
            created: Utc::now(),
            description: None,
            last_used: None,
            created_from: None,
            dirs: BTreeMap::new(),
            config: None,
            setup_commands: std::collections::BTreeMap::new(),
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
        create(
            &paths,
            "collide-ws",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

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
        create(
            &paths,
            "add-collide",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

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
        add_repos(&paths.mirrors_dir, &ws_dir, &new_refs, &urls2, false).unwrap();

        let meta = load_metadata(&ws_dir).unwrap();
        assert_eq!(meta.dir_name(&identity1).unwrap(), "user-test-repo");
        assert_eq!(meta.dir_name(&identity2).unwrap(), "other-test-repo");
        assert!(!ws_dir.join("test-repo").exists());
        assert!(ws_dir.join("user-test-repo").exists());
        assert!(ws_dir.join("other-test-repo").exists());
    }

    #[test]
    fn test_add_repos_intra_batch_collision() {
        let (paths, _d, source_repo, identity1, mut upstream_urls) = setup_test_env();

        // Create workspace with no repos
        let refs = BTreeMap::new();
        create(
            &paths,
            "batch-collide",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();
        let ws_dir = dir(&paths.workspaces_dir, "batch-collide");

        // Add two repos with the same short name ("test-repo") in one batch
        let (identity2, urls2) = add_mirror_with_owner(
            &paths,
            source_repo.path(),
            "test.local",
            "other",
            "test-repo",
        );
        upstream_urls.extend(urls2.clone());

        let new_refs = BTreeMap::from([
            (identity1.clone(), String::new()),
            (identity2.clone(), String::new()),
        ]);
        let mut all_urls = upstream_urls.clone();
        all_urls.extend(urls2);
        add_repos(&paths.mirrors_dir, &ws_dir, &new_refs, &all_urls, false).unwrap();

        let meta = load_metadata(&ws_dir).unwrap();
        assert_eq!(meta.dir_name(&identity1).unwrap(), "user-test-repo");
        assert_eq!(meta.dir_name(&identity2).unwrap(), "other-test-repo");
        assert!(ws_dir.join("user-test-repo").exists());
        assert!(ws_dir.join("other-test-repo").exists());
        // Short name should not exist — both are disambiguated
        assert!(!ws_dir.join("test-repo").exists());
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
        create(
            &paths,
            "rm-repo-ws",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "rm-repo-ws");
        assert!(ws_dir.join("test-repo").exists());
        assert!(ws_dir.join("other-repo").exists());

        remove_repos(
            &paths.mirrors_dir,
            &ws_dir,
            std::slice::from_ref(&identity2),
            false,
        )
        .unwrap();

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
        create(
            &paths,
            "rm-repo-nf",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "rm-repo-nf");
        let result = remove_repos(
            &paths.mirrors_dir,
            &ws_dir,
            &["test.local/nobody/fake".to_string()],
            false,
        );
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
        create(
            &paths,
            "rm-repo-dirty",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "rm-repo-dirty");
        let repo_dir = ws_dir.join("test-repo");
        fs::write(repo_dir.join("dirty.txt"), "x").unwrap();

        let result = remove_repos(
            &paths.mirrors_dir,
            &ws_dir,
            std::slice::from_ref(&identity),
            false,
        );
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("uncommitted changes")
        );
    }

    #[test]
    fn test_remove_repos_force_with_pending_changes() {
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(
            &paths,
            "rm-repo-force",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "rm-repo-force");
        let repo_dir = ws_dir.join("test-repo");
        fs::write(repo_dir.join("dirty.txt"), "x").unwrap();

        remove_repos(
            &paths.mirrors_dir,
            &ws_dir,
            std::slice::from_ref(&identity),
            true,
        )
        .unwrap();

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
        create(
            &paths,
            "rm-repo-col",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "rm-repo-col");
        assert!(ws_dir.join("user-test-repo").exists());
        assert!(ws_dir.join("other-test-repo").exists());

        remove_repos(
            &paths.mirrors_dir,
            &ws_dir,
            std::slice::from_ref(&identity2),
            false,
        )
        .unwrap();

        let meta = load_metadata(&ws_dir).unwrap();
        assert_eq!(meta.repos.len(), 1);
        assert!(meta.dirs.is_empty(), "no collisions, dirs should be empty");
        assert_eq!(meta.dir_name(&identity1).unwrap(), "test-repo");
        assert!(ws_dir.join("test-repo").exists());
        assert!(!ws_dir.join("user-test-repo").exists());
        assert!(!ws_dir.join("other-test-repo").exists());
    }

    /// Helper: rebase-merge a branch into target in the source repo.
    /// Simulates GitHub's "Rebase and merge" button: each commit is cherry-picked
    /// onto the target with a new hash, then the source branch is deleted.
    fn rebase_merge_branch(dir: &Path, branch: &str, target: &str) {
        for args in &[
            vec!["git", "checkout", target],
            vec!["git", "rebase", branch],
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

    /// Create a new branch, commit a file, and leave HEAD on that branch.
    /// Sets git identity so tests work on CI runners with no global git config.
    fn commit_on_new_branch(repo_dir: &Path, branch: &str, file: &str, content: &str) {
        for args in &[
            vec!["git", "config", "user.email", "test@test.com"],
            vec!["git", "config", "user.name", "Test"],
            vec!["git", "config", "commit.gpgsign", "false"],
        ] {
            let out = Command::new(args[0])
                .args(&args[1..])
                .current_dir(repo_dir)
                .output()
                .unwrap();
            assert!(out.status.success());
        }
        let out = Command::new("git")
            .args(["checkout", "-b", branch])
            .current_dir(repo_dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "checkout -b {}: {}",
            branch,
            String::from_utf8_lossy(&out.stderr)
        );
        fs::write(repo_dir.join(file), content).unwrap();
        for args in &[
            vec!["git", "add", file],
            vec!["git", "commit", "-m", &format!("add {}", file)],
        ] {
            let out = Command::new(args[0])
                .args(&args[1..])
                .current_dir(repo_dir)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{:?}: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }

    #[test]
    fn test_remove_allows_squash_merged_branch() {
        let (paths, _d, source_repo, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(
            &paths,
            "rm-squash",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "rm-squash");
        let repo_dir = ws_dir.join("test-repo");

        commit_push_and_track(&repo_dir, "rm-squash", "feat.txt", "feature");
        squash_merge_branch(source_repo.path(), "rm-squash", "main");

        // Remove should succeed without --force since branch is squash-merged
        remove(&paths, "rm-squash", false).unwrap();
        assert!(!ws_dir.exists());
    }

    // Regression test: after a squash-merge where the remote tracking branch is
    // deleted (GitHub auto-delete-branch), ahead_count falls back to
    // origin/main..HEAD and finds N commits. Previously this triggered the
    // early-exit "pending changes" guard, preventing branch_safety from running
    // and detecting the squash merge.
    #[test]
    fn test_remove_allows_squash_merged_after_branch_deletion() {
        let (paths, _d, source_repo, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(
            &paths,
            "rm-squash-del",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "rm-squash-del");
        let repo_dir = ws_dir.join("test-repo");

        commit_push_and_track(&repo_dir, "rm-squash-del", "feat.txt", "feature");
        squash_merge_branch(source_repo.path(), "rm-squash-del", "main");

        // Simulate GitHub auto-delete-branch: delete the remote tracking branch
        // and prune locally, so ahead_count falls back to origin/main..HEAD.
        let out = Command::new("git")
            .args(["push", "origin", "--delete", "rm-squash-del"])
            .current_dir(&repo_dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "delete remote branch: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let out = Command::new("git")
            .args(["fetch", "--prune", "origin"])
            .current_dir(&repo_dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "fetch --prune: {}",
            String::from_utf8_lossy(&out.stderr)
        );

        // Remove should succeed: branch_safety detects squash merge even though
        // ahead_count now sees N commits ahead of origin/main.
        remove(&paths, "rm-squash-del", false).unwrap();
        assert!(!ws_dir.exists());
    }

    #[test]
    fn test_remove_blocks_pushed_but_unmerged() {
        let (paths, _d, _source_repo, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(
            &paths,
            "rm-pushed",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

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
        create(
            &paths,
            "rmr-squash",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "rmr-squash");
        let repo_dir = ws_dir.join("test-repo");

        commit_push_and_track(&repo_dir, "rmr-squash", "feat.txt", "feature");
        squash_merge_branch(source_repo.path(), "rmr-squash", "main");

        remove_repos(
            &paths.mirrors_dir,
            &ws_dir,
            std::slice::from_ref(&identity),
            false,
        )
        .unwrap();
        let meta = load_metadata(&ws_dir).unwrap();
        assert!(meta.repos.is_empty());
    }

    // Regression test: same scenario as test_remove_allows_squash_merged_after_branch_deletion
    // but via remove_repos.
    #[test]
    fn test_remove_repos_allows_squash_merged_after_branch_deletion() {
        let (paths, _d, source_repo, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(
            &paths,
            "rmr-squash-del",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "rmr-squash-del");
        let repo_dir = ws_dir.join("test-repo");

        commit_push_and_track(&repo_dir, "rmr-squash-del", "feat.txt", "feature");
        squash_merge_branch(source_repo.path(), "rmr-squash-del", "main");

        // Delete remote branch and prune locally, as GitHub does after PR merge.
        let out = Command::new("git")
            .args(["push", "origin", "--delete", "rmr-squash-del"])
            .current_dir(&repo_dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "delete remote branch: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let out = Command::new("git")
            .args(["fetch", "--prune", "origin"])
            .current_dir(&repo_dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "fetch --prune: {}",
            String::from_utf8_lossy(&out.stderr)
        );

        remove_repos(
            &paths.mirrors_dir,
            &ws_dir,
            std::slice::from_ref(&identity),
            false,
        )
        .unwrap();
        let meta = load_metadata(&ws_dir).unwrap();
        assert!(meta.repos.is_empty());
    }

    // Rebase merge (GitHub "Rebase and merge"): each commit is cherry-picked onto
    // main with a new hash. branch_is_merged and branch_is_squash_merged both
    // return false, but is_content_merged catches it via file-content diff.
    #[test]
    fn test_remove_allows_rebase_merged_branch() {
        let (paths, _d, source_repo, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(
            &paths,
            "rm-rebase",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "rm-rebase");
        let repo_dir = ws_dir.join("test-repo");

        commit_push_and_track(&repo_dir, "rm-rebase", "feat.txt", "feature");
        rebase_merge_branch(source_repo.path(), "rm-rebase", "main");

        // Delete remote branch and prune, as GitHub does after rebase-merge.
        let out = Command::new("git")
            .args(["push", "origin", "--delete", "rm-rebase"])
            .current_dir(&repo_dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "delete remote branch: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let out = Command::new("git")
            .args(["fetch", "--prune", "origin"])
            .current_dir(&repo_dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "fetch --prune: {}",
            String::from_utf8_lossy(&out.stderr)
        );

        remove(&paths, "rm-rebase", false).unwrap();
        assert!(!ws_dir.exists());
    }

    #[test]
    fn test_remove_repos_blocks_pushed_but_unmerged() {
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(
            &paths,
            "rmr-pushed",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "rmr-pushed");
        let repo_dir = ws_dir.join("test-repo");

        commit_push_and_track(&repo_dir, "rmr-pushed", "wip.txt", "wip");

        let result = remove_repos(
            &paths.mirrors_dir,
            &ws_dir,
            std::slice::from_ref(&identity),
            false,
        );
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("pushed to remote"),
            "expected 'pushed to remote' in error: {}",
            err
        );
    }

    #[test]
    fn test_remove_blocks_unmerged_current_branch() {
        // Regression: workspace branch is clean (merged), but HEAD is on a local-only
        // branch with unpushed commits — remove must block.
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(
            &paths,
            "rm-cur-local",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "rm-cur-local");
        let repo_dir = ws_dir.join("test-repo");
        commit_on_new_branch(&repo_dir, "hotfix/urgent", "fix.txt", "urgent fix");

        let result = remove(&paths, "rm-cur-local", false);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("hotfix/urgent"),
            "expected current branch name in error: {}",
            err
        );
        assert!(ws_dir.exists());
    }

    #[test]
    fn test_remove_blocks_pushed_but_unmerged_current_branch() {
        // Regression: workspace branch is clean, HEAD is on a pushed-but-unmerged
        // branch — check_removal_blockers must classify it as pushed_unmerged.
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(
            &paths,
            "rm-cur-pushed",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "rm-cur-pushed");
        let repo_dir = ws_dir.join("test-repo");
        commit_on_new_branch(&repo_dir, "feature/wip", "wip.txt", "work in progress");

        let out = Command::new("git")
            .args(["push", "origin", "feature/wip"])
            .current_dir(&repo_dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let out = Command::new("git")
            .args(["fetch", "origin"])
            .current_dir(&repo_dir)
            .output()
            .unwrap();
        assert!(out.status.success());

        let blockers = check_removal_blockers(&paths, "rm-cur-pushed").unwrap();
        assert!(
            blockers
                .pushed_unmerged
                .iter()
                .any(|m| m.contains("feature/wip")),
            "expected 'feature/wip' in pushed_unmerged: {:?}",
            blockers.pushed_unmerged
        );
    }

    #[test]
    fn test_remove_repos_blocks_unmerged_current_branch() {
        // Regression: remove_repos must block when HEAD is on a local-only unmerged
        // branch, even when the workspace branch itself is clean.
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(
            &paths,
            "rmr-cur-local",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "rmr-cur-local");
        let repo_dir = ws_dir.join("test-repo");
        commit_on_new_branch(&repo_dir, "hotfix/urgent", "fix.txt", "urgent fix");

        let result = remove_repos(
            &paths.mirrors_dir,
            &ws_dir,
            std::slice::from_ref(&identity),
            false,
        );
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("hotfix/urgent"),
            "expected current branch name in error: {}",
            err
        );
    }

    #[test]
    fn test_remove_blocks_unmerged_current_when_workspace_branch_deleted() {
        // Regression: the primary data-loss path — workspace branch was merged and
        // deleted locally, user is on a local-only branch with unpushed commits.
        // This exercises the restructuring that moved target resolution before the
        // !branch_exists guard so the current-branch check still runs.
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(
            &paths,
            "rm-cur-del",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "rm-cur-del");
        let repo_dir = ws_dir.join("test-repo");
        commit_on_new_branch(&repo_dir, "hotfix/urgent", "fix.txt", "urgent fix");

        // Delete the workspace branch locally — simulates post-merge cleanup.
        let out = Command::new("git")
            .args(["branch", "-D", "rm-cur-del"])
            .current_dir(&repo_dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );

        // remove must still block — the current-branch check must fire even
        // though meta.branch no longer exists locally.
        let result = remove(&paths, "rm-cur-del", false);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("hotfix/urgent"),
            "expected current branch name in error: {}",
            err
        );
        assert!(ws_dir.exists());
    }

    #[test]
    fn test_remove_repos_blocks_unmerged_current_when_workspace_branch_deleted() {
        // Same as test_remove_blocks_unmerged_current_when_workspace_branch_deleted
        // but via remove_repos.
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(
            &paths,
            "rmr-cur-del",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "rmr-cur-del");
        let repo_dir = ws_dir.join("test-repo");
        commit_on_new_branch(&repo_dir, "hotfix/urgent", "fix.txt", "urgent fix");

        let out = Command::new("git")
            .args(["branch", "-D", "rmr-cur-del"])
            .current_dir(&repo_dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );

        let result = remove_repos(
            &paths.mirrors_dir,
            &ws_dir,
            std::slice::from_ref(&identity),
            false,
        );
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("hotfix/urgent"),
            "expected current branch name in error: {}",
            err
        );
    }

    #[test]
    fn test_remove_repos_blocks_pushed_but_unmerged_current_branch() {
        // remove_repos treats PushedToRemote on the current branch as a hard
        // blocker (requires --force), unlike check_removal_blockers which uses
        // a soft prompt. This test locks in that asymmetric behavior.
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(
            &paths,
            "rmr-cur-pushed",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "rmr-cur-pushed");
        let repo_dir = ws_dir.join("test-repo");
        commit_on_new_branch(&repo_dir, "feature/wip", "wip.txt", "work in progress");

        let out = Command::new("git")
            .args(["push", "origin", "feature/wip"])
            .current_dir(&repo_dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let out = Command::new("git")
            .args(["fetch", "origin"])
            .current_dir(&repo_dir)
            .output()
            .unwrap();
        assert!(out.status.success());

        let result = remove_repos(
            &paths.mirrors_dir,
            &ws_dir,
            std::slice::from_ref(&identity),
            false,
        );
        assert!(
            result.is_err(),
            "remove_repos must block on pushed-but-unmerged current branch"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("feature/wip"),
            "expected current branch name in error: {}",
            err
        );
    }

    #[test]
    fn test_remove_force_bypasses_current_branch_check() {
        // --force must bypass the current-branch blocker. This test verifies
        // two things in sequence: (1) check_removal_blockers reports the
        // unmerged current branch as a blocker, and (2) remove(force=true)
        // succeeds anyway — the flag intentionally skips all checks.
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(
            &paths,
            "rm-cur-force",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "rm-cur-force");
        let repo_dir = ws_dir.join("test-repo");
        commit_on_new_branch(&repo_dir, "hotfix/urgent", "fix.txt", "urgent fix");

        // Part 1: check_removal_blockers must report the unmerged current branch.
        let blockers = check_removal_blockers(&paths, "rm-cur-force").unwrap();
        assert!(
            !blockers.local_unmerged.is_empty(),
            "expected local_unmerged blocker for unmerged current branch, got: {:?}",
            blockers
        );

        // Part 2: remove(force=true) bypasses all checks and removes the workspace.
        remove(&paths, "rm-cur-force", true).unwrap();
        assert!(
            !ws_dir.exists(),
            "workspace must be removed when force=true"
        );
    }

    #[test]
    fn test_clone_has_only_origin() {
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(
            &paths,
            "only-origin",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "only-origin");
        let clone_dir = ws_dir.join("test-repo");

        // Verify only origin exists, no wsp-mirror
        let remotes = git::run(Some(&clone_dir), &["remote"]).unwrap();
        assert!(remotes.contains("origin"), "should have origin remote");
        assert!(
            !remotes.contains("wsp-mirror"),
            "should not have wsp-mirror remote"
        );

        // origin should point to source repo (upstream URL)
        let origin_url = git::run(Some(&clone_dir), &["remote", "get-url", "origin"]).unwrap();
        assert_eq!(origin_url, upstream_urls[&identity]);
    }

    #[test]
    fn test_remove_does_not_touch_mirror_branches() {
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(
            &paths,
            "rm-no-mirror",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

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
        create(
            &paths,
            "prop-ws",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

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

        // Before propagation, clone doesn't have the new commit at origin/main
        let clone_sha_before = git::run(Some(&clone_dir), &["rev-parse", "origin/main"]).unwrap();
        assert_ne!(clone_sha_before, mirror_sha);

        // Propagate
        let meta = load_metadata(&ws_dir).unwrap();
        propagate_mirror_to_clones(
            &paths.mirrors_dir,
            &ws_dir,
            &meta,
            &Config::default(),
            false,
        );

        // After propagation, clone should have the new commit at origin/main
        let clone_sha_after = git::run(Some(&clone_dir), &["rev-parse", "origin/main"]).unwrap();
        assert_eq!(clone_sha_after, mirror_sha);
    }

    /// A missing mirror is skipped with a warning (see the `wsp cd` integration
    /// tests for the message itself) and must leave the clone untouched.
    #[test]
    fn test_propagate_skips_missing_mirror() {
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(
            &paths,
            "prop-no-mirror",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "prop-no-mirror");
        let clone_dir = ws_dir.join("test-repo");
        let sha_before = git::run(Some(&clone_dir), &["rev-parse", "origin/main"]).unwrap();

        let parsed = parse_identity(&identity).unwrap();
        fs::remove_dir_all(mirror::dir(&paths.mirrors_dir, &parsed)).unwrap();

        let meta = load_metadata(&ws_dir).unwrap();
        propagate_mirror_to_clones(
            &paths.mirrors_dir,
            &ws_dir,
            &meta,
            &Config::default(),
            false,
        );

        let sha_after = git::run(Some(&clone_dir), &["rev-parse", "origin/main"]).unwrap();
        assert_eq!(sha_before, sha_after, "clone refs should be untouched");
    }

    #[test]
    fn test_propagate_removes_legacy_wsp_mirror() {
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(
            &paths,
            "prop-legacy",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "prop-legacy");
        let clone_dir = ws_dir.join("test-repo");

        // Manually add a wsp-mirror remote to simulate a legacy clone
        let parsed = parse_identity(&identity).unwrap();
        let mirror_dir = mirror::dir(&paths.mirrors_dir, &parsed);
        git::run(
            Some(&clone_dir),
            &["remote", "add", "wsp-mirror", mirror_dir.to_str().unwrap()],
        )
        .unwrap();
        assert!(
            git::has_remote(&clone_dir, "wsp-mirror"),
            "wsp-mirror should exist before propagate"
        );

        // Propagate
        let meta = load_metadata(&ws_dir).unwrap();
        propagate_mirror_to_clones(
            &paths.mirrors_dir,
            &ws_dir,
            &meta,
            &Config::default(),
            false,
        );

        // wsp-mirror should have been removed
        assert!(
            !git::has_remote(&clone_dir, "wsp-mirror"),
            "wsp-mirror should be removed after propagate"
        );
    }

    #[test]
    fn test_propagate_with_prune_removes_deleted_branches() {
        let (paths, _d, source_repo, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(
            &paths,
            "prop-prune",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "prop-prune");
        let clone_dir = ws_dir.join("test-repo");
        let parsed = parse_identity(&identity).unwrap();
        let mirror_dir = mirror::dir(&paths.mirrors_dir, &parsed);

        // Create a branch in source, fetch into mirror, propagate to clone
        let output = Command::new("git")
            .args(["checkout", "-b", "feature-x"])
            .current_dir(source_repo.path())
            .output()
            .unwrap();
        assert!(output.status.success());
        let output = Command::new("git")
            .args(["commit", "--allow-empty", "-m", "feature commit"])
            .current_dir(source_repo.path())
            .output()
            .unwrap();
        assert!(output.status.success());

        git::fetch(&mirror_dir, true).unwrap();
        let meta = load_metadata(&ws_dir).unwrap();
        propagate_mirror_to_clones(
            &paths.mirrors_dir,
            &ws_dir,
            &meta,
            &Config::default(),
            false,
        );

        // Clone should now see origin/feature-x
        assert!(
            git::ref_exists(&clone_dir, "refs/remotes/origin/feature-x"),
            "origin/feature-x should exist after propagation"
        );

        // Delete the branch in source and re-fetch mirror (mirror auto-prunes)
        let output = Command::new("git")
            .args(["checkout", "main"])
            .current_dir(source_repo.path())
            .output()
            .unwrap();
        assert!(output.status.success());
        let output = Command::new("git")
            .args(["branch", "-D", "feature-x"])
            .current_dir(source_repo.path())
            .output()
            .unwrap();
        assert!(output.status.success());

        git::fetch(&mirror_dir, true).unwrap();

        // Propagate with prune=true — should remove stale origin/feature-x
        propagate_mirror_to_clones(&paths.mirrors_dir, &ws_dir, &meta, &Config::default(), true);

        assert!(
            !git::ref_exists(&clone_dir, "refs/remotes/origin/feature-x"),
            "origin/feature-x should be pruned after propagation with prune=true"
        );
    }

    #[test]
    fn test_clone_has_origin_remote_refs() {
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(
            &paths,
            "origin-refs",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

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
        create(
            &paths,
            "rm-div-squash",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

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

    #[test]
    fn test_metadata_version_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let meta = Metadata {
            version: CURRENT_METADATA_VERSION,
            name: "my-ws".into(),
            branch: "my-ws".into(),
            repos: BTreeMap::from([("github.com/user/repo-a".into(), None)]),
            created: Utc::now(),
            description: None,
            last_used: None,
            created_from: None,
            dirs: BTreeMap::new(),
            config: None,
            setup_commands: std::collections::BTreeMap::new(),
        };

        save_metadata(tmp.path(), &meta).unwrap();

        // version 0 should be omitted from YAML via skip_serializing_if
        let yaml = fs::read_to_string(tmp.path().join(METADATA_FILE)).unwrap();
        assert!(
            !yaml.contains("version"),
            "version 0 should be omitted from YAML"
        );

        let loaded = load_metadata(tmp.path()).unwrap();
        assert_eq!(loaded.version, 0);
    }

    #[test]
    fn test_metadata_backward_compat_no_version() {
        let tmp = tempfile::tempdir().unwrap();
        let yaml = "name: old-ws\nbranch: old-ws\nrepos:\n  github.com/acme/api:\ncreated: '2024-01-01T00:00:00Z'\n";
        fs::write(tmp.path().join(METADATA_FILE), yaml).unwrap();

        let meta = load_metadata(tmp.path()).unwrap();
        assert_eq!(meta.version, 0);
    }

    #[test]
    fn test_metadata_unknown_version_loads() {
        let tmp = tempfile::tempdir().unwrap();
        let yaml = "version: 99\nname: future-ws\nbranch: future-ws\nrepos:\n  github.com/acme/api:\ncreated: '2024-01-01T00:00:00Z'\n";
        fs::write(tmp.path().join(METADATA_FILE), yaml).unwrap();

        let meta = load_metadata(tmp.path()).unwrap();
        assert_eq!(meta.version, 99);
        assert_eq!(meta.name, "future-ws");
    }

    // --- Root content detection tests ---

    fn make_simple_metadata(repos: &[&str]) -> Metadata {
        let mut map = BTreeMap::new();
        for id in repos {
            map.insert(id.to_string(), None);
        }
        Metadata {
            version: CURRENT_METADATA_VERSION,
            name: "test".into(),
            branch: "test".into(),
            repos: map,
            created: Utc::now(),
            description: None,
            last_used: None,
            created_from: None,
            dirs: BTreeMap::new(),
            config: None,
            setup_commands: std::collections::BTreeMap::new(),
        }
    }

    #[test]
    fn test_check_root_content() {
        struct Case {
            name: &'static str,
            setup: Box<dyn Fn(&Path)>,
            repos: Vec<&'static str>,
            want_clean: bool,
            want_contains: Vec<&'static str>,
        }

        #[allow(unused_mut)]
        let mut cases: Vec<Case> = vec![
            Case {
                name: "clean workspace — only repo dirs + .wsp.yaml",
                setup: Box::new(|ws| {
                    fs::create_dir_all(ws.join("api-gateway")).unwrap();
                    fs::write(ws.join(METADATA_FILE), "").unwrap();
                }),
                repos: vec!["github.com/acme/api-gateway"],
                want_clean: true,
                want_contains: vec![],
            },
            Case {
                name: "user file at root",
                setup: Box::new(|ws| {
                    fs::write(ws.join(METADATA_FILE), "").unwrap();
                    fs::write(ws.join("notes.md"), "my notes").unwrap();
                }),
                repos: vec![],
                want_clean: false,
                want_contains: vec!["?? notes.md"],
            },
            Case {
                name: "user directory at root",
                setup: Box::new(|ws| {
                    fs::write(ws.join(METADATA_FILE), "").unwrap();
                    fs::create_dir_all(ws.join("my-stuff")).unwrap();
                }),
                repos: vec![],
                want_clean: false,
                want_contains: vec!["?? my-stuff/"],
            },
            Case {
                name: "AGENTS.md with only scaffold + markers",
                setup: Box::new(|ws| {
                    fs::write(ws.join(METADATA_FILE), "").unwrap();
                    fs::write(
                        ws.join("AGENTS.md"),
                        "# Workspace: test\n\n<!-- Add your project-specific notes for AI agents here -->\n\n<!-- wsp:begin -->\nstuff\n<!-- wsp:end -->\n",
                    )
                    .unwrap();
                }),
                repos: vec![],
                want_clean: true,
                want_contains: vec![],
            },
            Case {
                name: "AGENTS.md with user notes before markers",
                setup: Box::new(|ws| {
                    fs::write(ws.join(METADATA_FILE), "").unwrap();
                    fs::write(
                        ws.join("AGENTS.md"),
                        "# Workspace: test\n\n## My Custom Notes\n\nImportant context here.\n\n<!-- wsp:begin -->\nstuff\n<!-- wsp:end -->\n",
                    )
                    .unwrap();
                }),
                repos: vec![],
                want_clean: false,
                want_contains: vec![" M AGENTS.md (user-added content)"],
            },
            Case {
                name: "AGENTS.md with missing markers",
                setup: Box::new(|ws| {
                    fs::write(ws.join(METADATA_FILE), "").unwrap();
                    fs::write(ws.join("AGENTS.md"), "# Some random content\n").unwrap();
                }),
                repos: vec![],
                want_clean: false,
                want_contains: vec![" M AGENTS.md (wsp markers missing)"],
            },
            Case {
                name: "CLAUDE.md as regular file",
                setup: Box::new(|ws| {
                    fs::write(ws.join(METADATA_FILE), "").unwrap();
                    fs::write(
                        ws.join("AGENTS.md"),
                        "# Workspace: test\n\n<!-- wsp:begin -->\n<!-- wsp:end -->\n",
                    )
                    .unwrap();
                    fs::write(ws.join("CLAUDE.md"), "custom content").unwrap();
                }),
                repos: vec![],
                want_clean: false,
                want_contains: vec!["?? CLAUDE.md"],
            },
            Case {
                name: ".claude/ with only managed skills",
                setup: Box::new(|ws| {
                    fs::write(ws.join(METADATA_FILE), "").unwrap();
                    for name in &["wsp-manage", "wsp-report"] {
                        let skill_dir = ws.join(format!(".claude/skills/{}", name));
                        fs::create_dir_all(&skill_dir).unwrap();
                        fs::write(skill_dir.join("SKILL.md"), "skill content").unwrap();
                    }
                }),
                repos: vec![],
                want_clean: true,
                want_contains: vec![],
            },
            Case {
                name: ".claude/ with user files",
                setup: Box::new(|ws| {
                    fs::write(ws.join(METADATA_FILE), "").unwrap();
                    for name in &["wsp-manage", "wsp-report"] {
                        let skill_dir = ws.join(format!(".claude/skills/{}", name));
                        fs::create_dir_all(&skill_dir).unwrap();
                        fs::write(skill_dir.join("SKILL.md"), "skill content").unwrap();
                    }
                    fs::write(ws.join(".claude/settings.json"), "{}").unwrap();
                }),
                repos: vec![],
                want_clean: false,
                want_contains: vec!["?? .claude/settings.json"],
            },
            Case {
                name: "go.work with wsp header",
                setup: Box::new(|ws| {
                    fs::write(ws.join(METADATA_FILE), "").unwrap();
                    fs::write(
                        ws.join("go.work"),
                        "// Code generated by wsp. DO NOT EDIT.\ngo 1.22\n\nuse (\n\t./api\n)\n",
                    )
                    .unwrap();
                }),
                repos: vec![],
                want_clean: true,
                want_contains: vec![],
            },
            Case {
                name: "go.work without wsp header",
                setup: Box::new(|ws| {
                    fs::write(ws.join(METADATA_FILE), "").unwrap();
                    fs::write(ws.join("go.work"), "go 1.22\n\nuse (\n\t./api\n)\n").unwrap();
                }),
                repos: vec![],
                want_clean: false,
                want_contains: vec!["?? go.work"],
            },
            Case {
                name: "go.work.sum alongside wsp go.work",
                setup: Box::new(|ws| {
                    fs::write(ws.join(METADATA_FILE), "").unwrap();
                    fs::write(
                        ws.join("go.work"),
                        "// Code generated by wsp. DO NOT EDIT.\ngo 1.22\n\nuse (\n\t./api\n)\n",
                    )
                    .unwrap();
                    fs::write(ws.join("go.work.sum"), "sum data").unwrap();
                }),
                repos: vec![],
                want_clean: true,
                want_contains: vec![],
            },
            Case {
                name: "go.work.sum without go.work is flagged",
                setup: Box::new(|ws| {
                    fs::write(ws.join(METADATA_FILE), "").unwrap();
                    fs::write(ws.join("go.work.sum"), "sum data").unwrap();
                }),
                repos: vec![],
                want_clean: false,
                want_contains: vec!["?? go.work.sum"],
            },
            Case {
                name: "lock file ignored",
                setup: Box::new(|ws| {
                    fs::write(ws.join(METADATA_FILE), "").unwrap();
                    fs::write(ws.join(".wsp.yaml.lock"), "12345").unwrap();
                }),
                repos: vec![],
                want_clean: true,
                want_contains: vec![],
            },
            Case {
                name: "noise files (.DS_Store) reported by check_root_content (filtered by wspignore)",
                setup: Box::new(|ws| {
                    fs::write(ws.join(METADATA_FILE), "").unwrap();
                    fs::write(ws.join(".DS_Store"), "").unwrap();
                    fs::write(ws.join("Thumbs.db"), "").unwrap();
                    fs::write(ws.join("desktop.ini"), "").unwrap();
                }),
                repos: vec![],
                want_clean: false,
                want_contains: vec!["?? .DS_Store", "?? Thumbs.db", "?? desktop.ini"],
            },
            Case {
                name: "multiple issues combined",
                setup: Box::new(|ws| {
                    fs::write(ws.join(METADATA_FILE), "").unwrap();
                    fs::write(ws.join("notes.md"), "x").unwrap();
                    let claude_dir = ws.join(".claude");
                    fs::create_dir_all(&claude_dir).unwrap();
                    fs::write(claude_dir.join("settings.json"), "{}").unwrap();
                }),
                repos: vec![],
                want_clean: false,
                want_contains: vec!["?? notes.md", "?? .claude/settings.json"],
            },
        ];

        #[cfg(unix)]
        cases.push(Case {
            name: "CLAUDE.md as symlink to AGENTS.md",
            setup: Box::new(|ws| {
                fs::write(ws.join(METADATA_FILE), "").unwrap();
                fs::write(
                    ws.join("AGENTS.md"),
                    "# Workspace: test\n\n<!-- wsp:begin -->\n<!-- wsp:end -->\n",
                )
                .unwrap();
                std::os::unix::fs::symlink("AGENTS.md", ws.join("CLAUDE.md")).unwrap();
            }),
            repos: vec![],
            want_clean: true,
            want_contains: vec![],
        });

        for tc in &cases {
            let tmp = tempfile::tempdir().unwrap();
            let ws_dir = tmp.path();
            (tc.setup)(ws_dir);

            let meta = make_simple_metadata(&tc.repos);
            let problems = check_root_content(ws_dir, &meta).unwrap();

            if tc.want_clean {
                assert!(
                    problems.is_empty(),
                    "case {:?}: expected clean, got {:?}",
                    tc.name,
                    problems
                );
            } else {
                assert!(
                    !problems.is_empty(),
                    "case {:?}: expected problems, got none",
                    tc.name
                );
            }

            for want in &tc.want_contains {
                assert!(
                    problems.iter().any(|p| p.to_string().contains(want)),
                    "case {:?}: expected problem containing {:?}, got {:?}",
                    tc.name,
                    want,
                    problems
                );
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn test_check_root_content_symlink() {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path();
        fs::write(ws.join(METADATA_FILE), "").unwrap();
        fs::write(
            ws.join("AGENTS.md"),
            "# Workspace: test\n\n<!-- wsp:begin -->\n<!-- wsp:end -->\n",
        )
        .unwrap();
        symlink("AGENTS.md", ws.join("CLAUDE.md")).unwrap();
        let meta = make_simple_metadata(&[]);
        let problems = check_root_content(ws, &meta).unwrap();
        assert!(
            problems.is_empty(),
            "CLAUDE.md symlink to AGENTS.md should be clean, got: {:?}",
            problems
        );
    }

    #[test]
    fn test_check_agents_md() {
        struct Case {
            name: &'static str,
            content: &'static str,
            want_clean: bool,
            want_contains: Option<&'static str>,
        }

        let cases = vec![
            Case {
                name: "scaffold only",
                content: "# Workspace: test\n\n<!-- Add your project-specific notes for AI agents here -->\n\n<!-- wsp:begin -->\nstuff\n<!-- wsp:end -->\n",
                want_clean: true,
                want_contains: None,
            },
            Case {
                name: "user heading before marker",
                content: "# Workspace: test\n\n## My Notes\n\n<!-- wsp:begin -->\nstuff\n<!-- wsp:end -->\n",
                want_clean: false,
                want_contains: Some("user-added content"),
            },
            Case {
                name: "user paragraph before marker",
                content: "# Workspace: test\n\nThis is important context for AI agents.\n\n<!-- wsp:begin -->\nstuff\n<!-- wsp:end -->\n",
                want_clean: false,
                want_contains: Some("user-added content"),
            },
            Case {
                name: "no markers",
                content: "# Some random file\n\nNo wsp markers here.\n",
                want_clean: false,
                want_contains: Some("wsp markers missing"),
            },
            Case {
                name: "empty preamble",
                content: "<!-- wsp:begin -->\nstuff\n<!-- wsp:end -->\n",
                want_clean: true,
                want_contains: None,
            },
            Case {
                name: "only blank lines before marker",
                content: "\n\n\n<!-- wsp:begin -->\nstuff\n<!-- wsp:end -->\n",
                want_clean: true,
                want_contains: None,
            },
            Case {
                name: "user content after end marker",
                content: "# Workspace: test\n\n<!-- wsp:begin -->\nstuff\n<!-- wsp:end -->\n\n## My post-marker notes\n",
                want_clean: false,
                want_contains: Some("user-added content after markers"),
            },
        ];

        for tc in &cases {
            let tmp = tempfile::tempdir().unwrap();
            let ws_dir = tmp.path();
            fs::write(ws_dir.join("AGENTS.md"), tc.content).unwrap();

            let result = check_agents_md(ws_dir);

            if tc.want_clean {
                assert!(
                    result.is_none(),
                    "case {:?}: expected clean, got {:?}",
                    tc.name,
                    result
                );
            } else {
                assert!(
                    result.is_some(),
                    "case {:?}: expected problem, got None",
                    tc.name
                );
                if let Some(want) = tc.want_contains {
                    assert!(
                        result.as_ref().unwrap().to_string().contains(want),
                        "case {:?}: expected {:?} in {:?}",
                        tc.name,
                        want,
                        result
                    );
                }
            }
        }
    }

    #[test]
    fn test_parse_wspignore() {
        struct Case {
            name: &'static str,
            input: &'static str,
            want: Vec<IgnorePattern>,
        }

        let cases = vec![
            Case {
                name: "empty",
                input: "",
                want: vec![],
            },
            Case {
                name: "only comments and blank lines",
                input: "# comment\n\n# another\n  \n",
                want: vec![],
            },
            Case {
                name: "exact paths",
                input: ".DS_Store\nnotes.md\n",
                want: vec![
                    IgnorePattern::Exact(".DS_Store".into()),
                    IgnorePattern::Exact("notes.md".into()),
                ],
            },
            Case {
                name: "directory prefixes",
                input: ".claude/\n.vscode/\n",
                want: vec![
                    IgnorePattern::DirPrefix(".claude/".into()),
                    IgnorePattern::DirPrefix(".vscode/".into()),
                ],
            },
            Case {
                name: "mixed with comments",
                input: "# OS noise\n.DS_Store\n\n# IDE\n.vscode/\nfoo.txt\n",
                want: vec![
                    IgnorePattern::Exact(".DS_Store".into()),
                    IgnorePattern::DirPrefix(".vscode/".into()),
                    IgnorePattern::Exact("foo.txt".into()),
                ],
            },
            Case {
                name: "whitespace trimming",
                input: "  .DS_Store  \n  .claude/  \n",
                want: vec![
                    IgnorePattern::Exact(".DS_Store".into()),
                    IgnorePattern::DirPrefix(".claude/".into()),
                ],
            },
        ];

        for tc in &cases {
            let got = parse_wspignore(tc.input);
            assert_eq!(got, tc.want, "case {:?}", tc.name);
        }
    }

    #[test]
    fn test_is_ignored() {
        struct Case {
            name: &'static str,
            path: &'static str,
            patterns: Vec<IgnorePattern>,
            want: bool,
        }

        let cases = vec![
            Case {
                name: "exact match",
                path: ".DS_Store",
                patterns: vec![IgnorePattern::Exact(".DS_Store".into())],
                want: true,
            },
            Case {
                name: "exact no match",
                path: "notes.md",
                patterns: vec![IgnorePattern::Exact(".DS_Store".into())],
                want: false,
            },
            Case {
                name: "dir prefix matches dir itself",
                path: ".claude/",
                patterns: vec![IgnorePattern::DirPrefix(".claude/".into())],
                want: true,
            },
            Case {
                name: "dir prefix matches files inside",
                path: ".claude/settings.json",
                patterns: vec![IgnorePattern::DirPrefix(".claude/".into())],
                want: true,
            },
            Case {
                name: "dir prefix matches nested files",
                path: ".claude/skills/custom/SKILL.md",
                patterns: vec![IgnorePattern::DirPrefix(".claude/".into())],
                want: true,
            },
            Case {
                name: "dir prefix no match",
                path: ".cursor/settings.json",
                patterns: vec![IgnorePattern::DirPrefix(".claude/".into())],
                want: false,
            },
            Case {
                name: "empty patterns",
                path: "anything",
                patterns: vec![],
                want: false,
            },
            Case {
                name: "dir name without slash not matched by dir prefix",
                path: ".claude",
                patterns: vec![IgnorePattern::DirPrefix(".claude/".into())],
                want: true,
            },
        ];

        for tc in &cases {
            let got = is_ignored(tc.path, &tc.patterns);
            assert_eq!(got, tc.want, "case {:?}", tc.name);
        }
    }

    #[test]
    fn test_filter_ignored() {
        let problems = vec![
            RootProblem {
                path: ".claude/settings.json".into(),
                kind: RootProblemKind::Untracked,
            },
            RootProblem {
                path: "notes.md".into(),
                kind: RootProblemKind::Untracked,
            },
            RootProblem {
                path: ".DS_Store".into(),
                kind: RootProblemKind::Untracked,
            },
        ];

        let patterns = vec![
            IgnorePattern::DirPrefix(".claude/".into()),
            IgnorePattern::Exact(".DS_Store".into()),
        ];

        let filtered = filter_ignored(&problems, &patterns);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].path, "notes.md");
    }

    #[test]
    fn test_ensure_global_wspignore() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path();

        // First call creates the file
        ensure_global_wspignore(data_dir).unwrap();
        let path = data_dir.join("wspignore");
        assert!(path.exists());
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains(".DS_Store"));
        assert!(content.contains(".claude/settings.local.json"));

        // Second call doesn't overwrite
        fs::write(&path, "custom content").unwrap();
        ensure_global_wspignore(data_dir).unwrap();
        let content2 = fs::read_to_string(&path).unwrap();
        assert_eq!(content2, "custom content");
    }

    #[test]
    fn test_wspignore_skip_file() {
        // .wspignore at workspace root should be skipped by check_root_content
        let tmp = tempfile::tempdir().unwrap();
        let ws_dir = tmp.path();
        fs::write(ws_dir.join(METADATA_FILE), "").unwrap();
        fs::write(ws_dir.join(".wspignore"), ".claude/\n").unwrap();

        let meta = make_simple_metadata(&[]);
        let problems = check_root_content(ws_dir, &meta).unwrap();
        assert!(
            problems.is_empty(),
            "expected .wspignore to be skipped, got {:?}",
            problems
        );
    }

    #[test]
    fn test_check_root_content_with_wspignore_filter() {
        // End-to-end: check_root_content + filter_ignored with a .wspignore file
        let tmp = tempfile::tempdir().unwrap();
        let ws_dir = tmp.path();
        // data_dir must be outside ws_dir — check_root_content scans the workspace root
        let data_tmp = tempfile::tempdir().unwrap();
        let data_dir = data_tmp.path();

        fs::write(ws_dir.join(METADATA_FILE), "").unwrap();
        fs::create_dir_all(ws_dir.join(".claude")).unwrap();
        fs::write(ws_dir.join(".claude/settings.json"), "{}").unwrap();
        fs::write(ws_dir.join("notes.md"), "my notes").unwrap();
        fs::write(ws_dir.join(".wspignore"), ".claude/\n").unwrap();

        // No global wspignore
        fs::write(data_dir.join("wspignore"), "").unwrap();

        let meta = make_simple_metadata(&[]);
        let problems = check_root_content(ws_dir, &meta).unwrap();
        let ignore = load_wspignore(data_dir, ws_dir);
        let filtered = filter_ignored(&problems, &ignore);

        // .claude/settings.json should be filtered out, notes.md should remain
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].path, "notes.md");
    }

    #[test]
    fn test_is_ignored_nested_exact() {
        // Exact pattern matching nested paths (e.g. per-file ignore inside .claude/)
        let patterns = vec![IgnorePattern::Exact(".claude/settings.local.json".into())];
        assert!(is_ignored(".claude/settings.local.json", &patterns));
        assert!(!is_ignored(".claude/settings.json", &patterns));
        assert!(!is_ignored(".claude/other.json", &patterns));
    }

    #[test]
    fn test_load_wspignore_merges_global_and_local() {
        let data_tmp = tempfile::tempdir().unwrap();
        let ws_tmp = tempfile::tempdir().unwrap();

        fs::write(data_tmp.path().join("wspignore"), "# global\n.DS_Store\n").unwrap();
        fs::write(ws_tmp.path().join(".wspignore"), "# local\nnotes.md\n").unwrap();

        let patterns = load_wspignore(data_tmp.path(), ws_tmp.path());
        assert_eq!(patterns.len(), 2);
        assert_eq!(patterns[0], IgnorePattern::Exact(".DS_Store".into()));
        assert_eq!(patterns[1], IgnorePattern::Exact("notes.md".into()));
    }

    #[test]
    fn test_ensure_global_wspignore_creates_data_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let nested = tmp.path().join("deep/nested/dir");
        // data_dir doesn't exist yet
        assert!(!nested.exists());

        ensure_global_wspignore(&nested).unwrap();
        assert!(nested.join("wspignore").exists());
    }

    /// Create a git repo in the given directory with one commit and an origin remote.
    fn create_local_repo(dir: &Path, origin_url: &str) {
        fs::create_dir_all(dir).unwrap();
        let cmds: Vec<Vec<&str>> = vec![
            vec!["git", "init", "--initial-branch=main"],
            vec!["git", "config", "user.email", "test@test.com"],
            vec!["git", "config", "user.name", "Test"],
            vec!["git", "config", "commit.gpgsign", "false"],
            vec!["git", "commit", "--allow-empty", "-m", "initial"],
            vec!["git", "remote", "add", "origin", origin_url],
        ];
        for args in &cmds {
            let output = Command::new(args[0])
                .args(&args[1..])
                .current_dir(dir)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "command {:?} failed: {}",
                args,
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    #[test]
    fn test_validate_existing_dir_success() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_dir = tmp.path().join("test-repo");
        create_local_repo(&repo_dir, "git@github.com:user/test-repo.git");

        let result = validate_existing_dir(&repo_dir, "github.com/user/test-repo");
        assert!(result.is_ok(), "expected Ok, got: {:?}", result);
    }

    #[test]
    fn test_validate_existing_dir_cases() {
        struct Case {
            name: &'static str,
            setup: Box<dyn Fn(&Path)>,
            identity: &'static str,
            expect_err: &'static str,
        }

        let cases = vec![
            Case {
                name: "not a git repo",
                setup: Box::new(|dir: &Path| {
                    fs::create_dir_all(dir).unwrap();
                }),
                identity: "github.com/user/test-repo",
                expect_err: "not a git repository",
            },
            Case {
                name: "no origin remote",
                setup: Box::new(|dir: &Path| {
                    fs::create_dir_all(dir).unwrap();
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
                            .current_dir(dir)
                            .output()
                            .unwrap();
                        assert!(output.status.success());
                    }
                }),
                identity: "github.com/user/test-repo",
                expect_err: "no origin remote",
            },
            Case {
                name: "identity mismatch",
                setup: Box::new(|dir: &Path| {
                    create_local_repo(dir, "git@github.com:other/wrong-repo.git");
                }),
                identity: "github.com/user/test-repo",
                expect_err: "doesn't match expected",
            },
        ];

        for tc in cases {
            let tmp = tempfile::tempdir().unwrap();
            let repo_dir = tmp.path().join("test-repo");
            (tc.setup)(&repo_dir);

            let result = validate_existing_dir(&repo_dir, tc.identity);
            assert!(result.is_err(), "{}: expected error, got Ok", tc.name);
            let err = result.unwrap_err().to_string();
            assert!(
                err.contains(tc.expect_err),
                "{}: expected error containing {:?}, got {:?}",
                tc.name,
                tc.expect_err,
                err
            );
        }
    }

    #[test]
    fn test_adopt_existing_dir_in_workspace() {
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();

        // Create workspace with the repo first
        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(
            &paths,
            "adopt-ws",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "adopt-ws");
        let meta = load_metadata(&ws_dir).unwrap();
        let branch = meta.branch.clone();

        // Create a second "upstream" repo and its mirror
        let repo2_dir = tempfile::tempdir().unwrap();
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
                .current_dir(repo2_dir.path())
                .output()
                .unwrap();
            assert!(output.status.success());
        }

        let parsed2 = giturl::Parsed {
            host: "test.local".into(),
            owner: "user".into(),
            repo: "local-repo".into(),
        };
        mirror::clone(
            &paths.mirrors_dir,
            &parsed2,
            repo2_dir.path().to_str().unwrap(),
        )
        .unwrap();

        // Set up mirror HEAD ref
        let mirror_dir2 = mirror::dir(&paths.mirrors_dir, &parsed2);
        let output = Command::new("git")
            .args([
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/heads/main",
            ])
            .current_dir(&mirror_dir2)
            .output()
            .unwrap();
        assert!(output.status.success());

        let identity2 = parsed2.identity();

        // Manually create a repo directory inside the workspace (simulating user workflow)
        // Use an SSH-style URL that matches the identity so validation passes
        let local_dir = ws_dir.join("local-repo");
        create_local_repo(&local_dir, "git@test.local:user/local-repo.git");

        // Checkout the workspace branch so adoption is silent
        git::checkout_new_branch(&local_dir, &branch, "HEAD").unwrap();

        // Now add_repos should adopt it instead of cloning
        let refs2 = BTreeMap::from([(identity2.clone(), String::new())]);
        let upstream_urls2 = BTreeMap::from([(
            identity2.clone(),
            repo2_dir.path().to_str().unwrap().to_string(),
        )]);
        add_repos(&paths.mirrors_dir, &ws_dir, &refs2, &upstream_urls2, false).unwrap();

        // Verify it was registered in metadata
        let meta = load_metadata(&ws_dir).unwrap();
        assert!(
            meta.repos.contains_key(&identity2),
            "adopted repo should be in metadata"
        );

        // Verify the directory still exists with its .git
        assert!(local_dir.join(".git").exists());
    }

    #[test]
    fn test_adopt_rejects_identity_mismatch() {
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();

        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(
            &paths,
            "adopt-mismatch",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "adopt-mismatch");

        // Create a directory with a different origin
        let local_dir = ws_dir.join("wrong-repo");
        create_local_repo(&local_dir, "git@github.com:other/wrong-repo.git");

        // Try to adopt it as a different identity — should fail
        let wrong_identity = "test.local/user/wrong-repo".to_string();
        let parsed_wrong = giturl::Parsed {
            host: "test.local".into(),
            owner: "user".into(),
            repo: "wrong-repo".into(),
        };
        // Create mirror for the wrong identity
        let wrong_upstream = tempfile::tempdir().unwrap();
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
                .current_dir(wrong_upstream.path())
                .output()
                .unwrap();
            assert!(output.status.success());
        }
        mirror::clone(
            &paths.mirrors_dir,
            &parsed_wrong,
            wrong_upstream.path().to_str().unwrap(),
        )
        .unwrap();

        let refs2 = BTreeMap::from([(wrong_identity.clone(), String::new())]);
        let upstream_urls2 = BTreeMap::from([(
            wrong_identity,
            wrong_upstream.path().to_str().unwrap().to_string(),
        )]);

        let result = add_repos(&paths.mirrors_dir, &ws_dir, &refs2, &upstream_urls2, false);
        assert!(result.is_err(), "should reject identity mismatch");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("doesn't match"),
            "error should mention mismatch, got: {}",
            err
        );
    }

    /// Regression: when the mirror's refs/heads/main was stale (behind
    /// refs/remotes/origin/main), git clone --local checked out the old tree
    /// and the subsequent checkout -b left a dirty index.
    #[test]
    fn test_create_clean_index_after_mirror_diverges() {
        let (paths, _d, repo_dir, identity, upstream_urls) = setup_test_env();

        // Push new commits to the upstream AFTER the mirror was created,
        // then fetch the mirror so refs/remotes/origin/main advances but
        // (pre-fix) refs/heads/main would stay stale.
        let cmds: Vec<Vec<&str>> = vec![
            vec!["git", "commit", "--allow-empty", "-m", "second"],
            vec!["git", "commit", "--allow-empty", "-m", "third"],
        ];
        for args in &cmds {
            let out = Command::new(args[0])
                .args(&args[1..])
                .current_dir(repo_dir.path())
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{:?}: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
        }

        let parsed = giturl::Parsed {
            host: "test.local".into(),
            owner: "user".into(),
            repo: "test-repo".into(),
        };
        mirror::fetch(&paths.mirrors_dir, &parsed).unwrap();

        // Create workspace — this used to leave staged diffs
        let refs = BTreeMap::from([(identity, String::new())]);
        create(
            &paths,
            "clean-idx",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

        let clone_dir = dir(&paths.workspaces_dir, "clean-idx").join("test-repo");

        // Index must match HEAD (no staged changes)
        let diff = git::run(Some(&clone_dir), &["diff", "--cached", "--stat"]).unwrap();
        assert!(
            diff.is_empty(),
            "expected clean index, got staged changes:\n{}",
            diff
        );
    }

    #[test]
    fn test_rename_basic() {
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();
        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(
            &paths,
            "old-name",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

        let results = rename(&paths, "old-name", "new-name").unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].ok);
        assert_eq!(results[0].old_branch, "old-name");
        assert_eq!(results[0].new_branch, "new-name");

        // Old dir gone, new dir exists
        assert!(!dir(&paths.workspaces_dir, "old-name").exists());
        assert!(dir(&paths.workspaces_dir, "new-name").exists());

        // Metadata updated
        let meta = load_metadata(&dir(&paths.workspaces_dir, "new-name")).unwrap();
        assert_eq!(meta.name, "new-name");
        assert_eq!(meta.branch, "new-name");

        // Branch renamed in repo
        let clone_dir = dir(&paths.workspaces_dir, "new-name").join("test-repo");
        let branch = git::branch_current(&clone_dir).unwrap();
        assert_eq!(branch, "new-name");
    }

    #[test]
    fn test_rename_with_branch_prefix() {
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();
        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(
            &paths,
            "my-feature",
            &refs,
            Some("jganoff"),
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

        let results = rename(&paths, "my-feature", "your-feature").unwrap();
        assert!(results[0].ok);
        assert_eq!(results[0].old_branch, "jganoff/my-feature");
        assert_eq!(results[0].new_branch, "jganoff/your-feature");

        let meta = load_metadata(&dir(&paths.workspaces_dir, "your-feature")).unwrap();
        assert_eq!(meta.branch, "jganoff/your-feature");

        let clone_dir = dir(&paths.workspaces_dir, "your-feature").join("test-repo");
        let branch = git::branch_current(&clone_dir).unwrap();
        assert_eq!(branch, "jganoff/your-feature");
    }

    #[test]
    fn test_rename_target_exists() {
        let (paths, _d, _r, identity, upstream_urls) = setup_test_env();
        let refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(
            &paths,
            "ws-a",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();
        create(
            &paths,
            "ws-b",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

        let err = rename(&paths, "ws-a", "ws-b").unwrap_err();
        assert!(err.to_string().contains("already exists"));
    }

    #[test]
    fn test_rename_source_missing() {
        let (paths, _d, _r, _identity, _upstream_urls) = setup_test_env();
        let err = rename(&paths, "nonexistent", "new-name").unwrap_err();
        assert!(err.to_string().contains("does not exist"));
    }

    // -------------------------------------------------------------------------
    // R026: rename partial failure — one repo succeeds, one fails
    // -------------------------------------------------------------------------

    /// Helper: create a second mirror repo under `test.local/user/other-repo`.
    /// Returns (identity, upstream_urls) for the second repo.
    fn setup_second_repo(paths: &Paths) -> (String, BTreeMap<String, String>, tempfile::TempDir) {
        let repo_tmp = tempfile::tempdir().unwrap();
        let cmds: Vec<Vec<&str>> = vec![
            vec!["git", "init", "--initial-branch=main"],
            vec!["git", "config", "user.email", "test@test.com"],
            vec!["git", "config", "user.name", "Test"],
            vec!["git", "config", "commit.gpgsign", "false"],
            vec!["git", "commit", "--allow-empty", "-m", "initial"],
        ];
        for args in &cmds {
            let out = Command::new(args[0])
                .args(&args[1..])
                .current_dir(repo_tmp.path())
                .output()
                .unwrap();
            assert!(out.status.success());
        }

        let parsed = giturl::Parsed {
            host: "test.local".into(),
            owner: "user".into(),
            repo: "other-repo".into(),
        };
        mirror::clone(
            &paths.mirrors_dir,
            &parsed,
            repo_tmp.path().to_str().unwrap(),
        )
        .unwrap();

        // Set up HEAD ref
        let mirror_dir = mirror::dir(&paths.mirrors_dir, &parsed);
        let out = Command::new("git")
            .args([
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/heads/main",
            ])
            .current_dir(&mirror_dir)
            .output()
            .unwrap();
        assert!(out.status.success());

        let identity = parsed.identity();
        let upstream_urls = BTreeMap::from([(
            identity.clone(),
            repo_tmp.path().to_str().unwrap().to_string(),
        )]);

        (identity, upstream_urls, repo_tmp)
    }

    #[test]
    fn test_rename_partial_failure_rolls_back() {
        // Set up workspace with two repos
        let (paths, _d, _r1, identity1, upstream_urls1) = setup_test_env();
        let (identity2, upstream_urls2, _r2) = setup_second_repo(&paths);

        let mut refs = BTreeMap::new();
        refs.insert(identity1.clone(), String::new());
        refs.insert(identity2.clone(), String::new());

        let mut upstream_urls = upstream_urls1.clone();
        upstream_urls.extend(upstream_urls2.clone());

        create(
            &paths,
            "ws-partial",
            &refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "ws-partial");

        // Pre-create the target branch on the second repo's clone so its branch
        // rename will fail (target branch already exists).
        let clone2_dir = ws_dir.join("other-repo");
        let out = Command::new("git")
            .args(["branch", "ws-partial-renamed"])
            .current_dir(&clone2_dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "pre-creating branch failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );

        // rename should fail because one branch rename fails
        let err = rename(&paths, "ws-partial", "ws-partial-renamed").unwrap_err();
        assert!(
            err.to_string().contains("branch rename failed"),
            "unexpected error: {}",
            err
        );

        // The workspace directory must still be at the old path (directory not renamed)
        assert!(
            ws_dir.exists(),
            "workspace directory should not have been renamed after rollback"
        );
        assert!(
            !dir(&paths.workspaces_dir, "ws-partial-renamed").exists(),
            "new workspace directory should not exist after failed rename"
        );

        // The first repo's branch must be back on 'ws-partial' (rolled back)
        let clone1_dir = ws_dir.join("test-repo");
        let branch1 = git::branch_current(&clone1_dir).unwrap();
        assert_eq!(
            branch1, "ws-partial",
            "first repo branch should be rolled back to old name"
        );

        // Metadata must still say 'ws-partial'
        let meta = load_metadata(&ws_dir).unwrap();
        assert_eq!(meta.branch, "ws-partial");
        assert_eq!(meta.name, "ws-partial");
    }

    // -------------------------------------------------------------------------
    // R015: apply_git_config tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_apply_git_config_basic() {
        // Basic config is applied to a real git repo clone in the workspace.
        use crate::testutil;

        let (clone_dir, _source, _clone_tmp, _source_tmp) = testutil::setup_clone_repo();

        // Build a minimal Metadata pointing at clone_dir
        let identity = "test.local/user/repo".to_string();
        let meta = Metadata {
            version: CURRENT_METADATA_VERSION,
            name: "test-ws".into(),
            branch: "feature".into(),
            repos: BTreeMap::from([(identity.clone(), None)]),
            created: chrono::Utc::now(),
            description: None,
            last_used: None,
            created_from: None,
            dirs: BTreeMap::new(),
            config: None,
            setup_commands: std::collections::BTreeMap::new(),
        };

        // clone_dir's parent is the ws_dir; dir_name falls back to parsed.repo = "repo"
        // but setup_clone_repo puts the clone at <tmp>/repo, so ws_dir = <tmp>
        let ws_dir = clone_dir.parent().unwrap();

        let git_config = BTreeMap::from([("user.email".to_string(), "ci@example.com".to_string())]);
        apply_git_config(ws_dir, &meta, &git_config, None);

        // Verify the config was actually written
        let got = git::get_config(&clone_dir, "user.email").unwrap();
        assert_eq!(got, "ci@example.com");
    }

    #[test]
    fn test_apply_git_config_workspace_overrides_repo_level() {
        // Workspace config takes precedence: if a key already exists in the
        // repo's local config, apply_git_config must overwrite it.
        use crate::testutil;

        let (clone_dir, _source, _clone_tmp, _source_tmp) = testutil::setup_clone_repo();

        // Write a repo-level value first
        git::set_config(&clone_dir, "user.email", "old@example.com").unwrap();

        let identity = "test.local/user/repo".to_string();
        let meta = Metadata {
            version: CURRENT_METADATA_VERSION,
            name: "test-ws".into(),
            branch: "feature".into(),
            repos: BTreeMap::from([(identity.clone(), None)]),
            created: chrono::Utc::now(),
            description: None,
            last_used: None,
            created_from: None,
            dirs: BTreeMap::new(),
            config: None,
            setup_commands: std::collections::BTreeMap::new(),
        };

        let ws_dir = clone_dir.parent().unwrap();
        let git_config = BTreeMap::from([(
            "user.email".to_string(),
            "workspace@example.com".to_string(),
        )]);
        apply_git_config(ws_dir, &meta, &git_config, None);

        let got = git::get_config(&clone_dir, "user.email").unwrap();
        assert_eq!(
            got, "workspace@example.com",
            "workspace config should override repo-level config"
        );
    }

    #[test]
    fn test_apply_git_config_rejects_dangerous_key() {
        // Dangerous keys (e.g. core.sshCommand) must be silently skipped —
        // they must not be written to the repo's local config.
        use crate::testutil;

        let (clone_dir, _source, _clone_tmp, _source_tmp) = testutil::setup_clone_repo();

        let identity = "test.local/user/repo".to_string();
        let meta = Metadata {
            version: CURRENT_METADATA_VERSION,
            name: "test-ws".into(),
            branch: "feature".into(),
            repos: BTreeMap::from([(identity.clone(), None)]),
            created: chrono::Utc::now(),
            description: None,
            last_used: None,
            created_from: None,
            dirs: BTreeMap::new(),
            config: None,
            setup_commands: std::collections::BTreeMap::new(),
        };

        let ws_dir = clone_dir.parent().unwrap();
        let git_config = BTreeMap::from([(
            "core.sshCommand".to_string(),
            "malicious-binary".to_string(),
        )]);
        apply_git_config(ws_dir, &meta, &git_config, None);

        // The dangerous key must NOT have been written
        let result = git::get_config(&clone_dir, "core.sshCommand");
        assert!(
            result.is_err(),
            "dangerous key core.sshCommand should not have been set, but got: {:?}",
            result.ok()
        );
    }

    #[test]
    fn test_is_dangerous_git_config_key() {
        // Case-insensitive matching
        assert!(is_dangerous_git_config_key("core.sshCommand"));
        assert!(is_dangerous_git_config_key("core.sshcommand"));
        assert!(is_dangerous_git_config_key("CORE.SSHCOMMAND"));
        assert!(is_dangerous_git_config_key("core.hooksPath"));
        assert!(is_dangerous_git_config_key("core.pager"));
        assert!(is_dangerous_git_config_key("diff.external"));
        assert!(is_dangerous_git_config_key("credential.helper"));

        // Safe keys should pass through
        assert!(!is_dangerous_git_config_key("user.email"));
        assert!(!is_dangerous_git_config_key("user.name"));
        assert!(!is_dangerous_git_config_key("commit.gpgsign"));
        assert!(!is_dangerous_git_config_key("push.default"));
    }

    #[test]
    fn test_create_with_zero_repos() {
        // `wsp new --empty` passes an empty repo map; workspace and metadata
        // must be created successfully with no clone directories.
        use crate::testutil;
        let tmp = tempfile::tempdir().unwrap();
        let paths = testutil::make_test_paths(&tmp);

        let empty_refs: BTreeMap<String, String> = BTreeMap::new();
        let empty_upstreams: BTreeMap<String, String> = BTreeMap::new();

        create(
            &paths,
            "myws",
            &empty_refs,
            None,
            None,
            &empty_upstreams,
            Some("empty workspace"),
            None,
        )
        .unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "myws");
        assert!(ws_dir.exists());

        let meta = load_metadata(&ws_dir).unwrap();
        assert_eq!(meta.name, "myws");
        assert!(meta.repos.is_empty(), "expected zero repos in metadata");

        // Only .wsp.yaml should exist in the workspace root — no clone dirs.
        let entries: Vec<_> = fs::read_dir(&ws_dir).unwrap().collect();
        assert_eq!(
            entries.len(),
            1,
            "expected only .wsp.yaml, found: {:?}",
            entries
                .iter()
                .map(|e| e.as_ref().unwrap().file_name())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_add_repos_per_repo_branch_override() {
        // When repo_refs values contain a branch name (the @branch suffix),
        // add_repos should check out that branch in the clone rather than the
        // workspace branch.
        let (paths, _d, source_repo, identity, upstream_urls) = setup_test_env();

        // Create the workspace on a workspace branch
        let ws_refs = BTreeMap::from([(identity.clone(), String::new())]);
        create(
            &paths,
            "branch-override",
            &ws_refs,
            None,
            None,
            &upstream_urls,
            None,
            None,
        )
        .unwrap();

        let ws_dir = dir(&paths.workspaces_dir, "branch-override");
        let meta = load_metadata(&ws_dir).unwrap();
        let ws_branch = meta.branch.clone();

        // Create a specific remote branch in the source repo
        let target_branch = "feature/per-repo";
        let cmds: Vec<Vec<&str>> = vec![
            vec!["git", "checkout", "-b", target_branch],
            vec!["git", "commit", "--allow-empty", "-m", "branch commit"],
            vec!["git", "checkout", "main"],
        ];
        for args in &cmds {
            let out = Command::new(args[0])
                .args(&args[1..])
                .current_dir(source_repo.path())
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "setup cmd {:?}: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
        }

        let (identity2, urls2) = add_mirror_with_owner(
            &paths,
            source_repo.path(),
            "test.local",
            "other",
            "added-repo",
        );
        let mut all_urls = upstream_urls.clone();
        all_urls.extend(urls2);

        // Pass target_branch as the per-repo branch override in repo_refs value
        let add_refs = BTreeMap::from([(identity2.clone(), target_branch.to_string())]);
        add_repos(&paths.mirrors_dir, &ws_dir, &add_refs, &all_urls, false).unwrap();

        // Cloned repo should be on target_branch, not the workspace branch
        let clone_dir = ws_dir.join("added-repo");
        let current = git::branch_current(&clone_dir).unwrap();
        assert_eq!(
            current, target_branch,
            "cloned repo should be on the per-repo branch override, not workspace branch {:?}",
            ws_branch
        );
    }

    #[test]
    fn test_remove_partial_workspace_empty_dir() {
        // An empty workspace directory with no .wsp.yaml (partial creation) should
        // be deleted by remove() without requiring --force.
        let (paths, _d, _r, _identity, _urls) = setup_test_env();

        let ws_name = "partial-empty";
        let ws_dir = dir(&paths.workspaces_dir, ws_name);
        fs::create_dir_all(&ws_dir).unwrap();

        assert!(ws_dir.exists());
        assert!(!ws_dir.join(METADATA_FILE).exists());

        remove(&paths, ws_name, false).unwrap();
        assert!(
            !ws_dir.exists(),
            "empty partial workspace should be removed"
        );
    }

    #[test]
    fn test_remove_partial_workspace_nonempty() {
        // A non-empty partial workspace is removed by remove() — the CLI layer
        // handles confirmation (--yes / prompt) before calling into the library.
        let (paths, _d, _r, _identity, _urls) = setup_test_env();

        let ws_name = "partial-nonempty";
        let ws_dir = dir(&paths.workspaces_dir, ws_name);
        fs::create_dir_all(ws_dir.join("some-content")).unwrap();
        std::fs::write(ws_dir.join("some-content").join("file.txt"), "data").unwrap();

        assert!(ws_dir.exists());
        assert!(!ws_dir.join(METADATA_FILE).exists());

        remove(&paths, ws_name, false).unwrap();
        assert!(
            !ws_dir.exists(),
            "non-empty partial workspace should be removed"
        );
    }
}
