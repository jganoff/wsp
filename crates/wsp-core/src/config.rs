use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub const CURRENT_CONFIG_VERSION: u32 = 0;

fn default_version() -> u32 {
    CURRENT_CONFIG_VERSION
}

fn is_current_version(v: &u32) -> bool {
    *v == CURRENT_CONFIG_VERSION
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoEntry {
    pub url: String,
    pub added: DateTime<Utc>,
}

/// Value for an experimental feature: either a boolean toggle or a string mode.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ExperimentalValue {
    Bool(bool),
    String(String),
}

impl ExperimentalValue {
    /// Returns true if the value is `Bool(true)` or a non-empty string (i.e. not "false").
    pub fn is_truthy(&self) -> bool {
        match self {
            ExperimentalValue::Bool(b) => *b,
            ExperimentalValue::String(s) => !s.is_empty() && !s.eq_ignore_ascii_case("false"),
        }
    }

    /// Returns the string value if this is a `String` variant, or None.
    #[allow(dead_code)]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            ExperimentalValue::String(s) => Some(s),
            ExperimentalValue::Bool(_) => None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExperimentalConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(flatten)]
    pub features: BTreeMap<String, ExperimentalValue>,
}

impl ExperimentalConfig {
    /// Returns true if the experimental gate is on AND the named feature is truthy.
    pub fn is_feature_enabled(&self, feature: &str) -> bool {
        self.enabled && self.features.get(feature).is_some_and(|v| v.is_truthy())
    }

    /// Returns the string value for a feature, if set and the gate is on.
    #[allow(dead_code)]
    pub fn feature_value(&self, feature: &str) -> Option<&str> {
        if !self.enabled {
            return None;
        }
        self.features.get(feature).and_then(|v| v.as_str())
    }

    /// Resolves `shell-tmux` with backward compat from `shell-tmux-title`.
    /// Returns the mode string (e.g. "window-title") or None if disabled.
    pub fn shell_tmux_mode(&self) -> Option<&str> {
        if !self.enabled {
            return None;
        }
        // Prefer new key
        if let Some(v) = self.features.get("shell-tmux") {
            return match v {
                ExperimentalValue::String(s)
                    if !s.eq_ignore_ascii_case("false") && !s.is_empty() =>
                {
                    Some(s)
                }
                ExperimentalValue::Bool(true) => Some("window-title"),
                _ => None,
            };
        }
        // Fall back to deprecated bool key
        if let Some(ExperimentalValue::Bool(true)) = self.features.get("shell-tmux-title") {
            return Some("window-title");
        }
        None
    }
}

/// Keys labeled as experimental in UI output (display only, no gating).
pub const EXPERIMENTAL_KEYS: &[&str] = &["shell.tmux", "shell.prompt"];

/// Git config keys that can execute arbitrary code when git operations run.
///
/// These are blocked in `wsp config set git.*` and `wsp template config set git.*`
/// to prevent a malicious shared template from achieving code execution on
/// `git fetch`, `git push`, etc. The list covers well-known command-execution
/// hooks documented in `git help config` (search "execute" / "command").
///
/// Prefixes are matched case-insensitively (git config keys are case-insensitive).
const DANGEROUS_GIT_CONFIG_KEY_PREFIXES: &[&str] = &[
    "core.sshcommand",
    "core.hookspath",
    "core.pager",
    "core.editor",   // opens arbitrary commands as the editor
    "core.gitproxy", // executed as a shell command for proxy connections
    "core.fsmonitor",
    "core.askpass",
    "diff.external",
    "credential.helper",
    "credential.useHttpPath", // not exec but leaks credentials to wrong host
    "filter.",                // filter.<name>.clean / smudge / process
    "merge.tool",
    "mergetool.", // mergetool.<tool>.cmd
    "diff.tool",
    "difftool.",                // difftool.<tool>.cmd
    "sendemail.smtpEncryption", // not exec, but smtp commands follow same pattern
    "uploadpack.packObjectsHook",
    "receive.advertisePushOptions", // not exec, but proto-level
    "remote.",                      // remote.<name>.preFetch / uploadPack
    "url.",                         // url.<base>.insteadOf can redirect fetches
    "trailer.",                     // trailer.<key>.command executes a shell command
    "gpg.program",                  // executes the GPG binary
    "gpg.",                         // gpg.<format>.program variants
];

/// Returns `Err` if `git_key` (the part after `git.` in a wsp config key, e.g.
/// `core.sshCommand` from `git.core.sshCommand`) is on the denylist.
///
/// Matching is case-insensitive and prefix-based so that sub-keys like
/// `filter.lfs.clean` are caught by the `filter.` prefix.
pub fn validate_git_config_key(git_key: &str) -> Result<()> {
    let lower = git_key.to_lowercase();
    for prefix in DANGEROUS_GIT_CONFIG_KEY_PREFIXES {
        let p = prefix.to_lowercase();
        if lower == p || lower.starts_with(&p) {
            anyhow::bail!(
                "git config key '{}' is not allowed: it can execute arbitrary commands \
                 when git operations run. Use git config directly if you need this.",
                git_key
            );
        }
    }
    Ok(())
}

/// Valid values for `shell.tmux` (and legacy `experimental.shell-tmux`).
pub const SHELL_TMUX_VALUES: &[&str] = &["window-title", "false"];

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(
        default = "default_version",
        skip_serializing_if = "is_current_version"
    )]
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch_prefix: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub repos: BTreeMap<String, RepoEntry>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "lang",
        alias = "language_integrations"
    )]
    pub language_integrations: Option<BTreeMap<String, bool>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspaces_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync_strategy: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_md: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gc_retention_days: Option<u32>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "git",
        alias = "git_config"
    )]
    pub git_config: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell_tmux: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell_prompt: Option<bool>,
    /// Global hints toggle. Set to `false` to suppress all hints.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hints: Option<bool>,
    /// Per-hint suppression. `advice.<key> = false` silences a specific hint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub advice: Option<BTreeMap<String, bool>>,
    #[serde(default, skip_serializing)]
    pub experimental: Option<ExperimentalConfig>,
}

impl Config {
    pub fn load_from(path: &Path) -> Result<Config> {
        if !path.exists() {
            return Ok(Config::default());
        }

        let data = crate::util::read_yaml_file(path)?;
        let mut cfg: Config = serde_yaml_ng::from_str(&data)?;
        if cfg.version > CURRENT_CONFIG_VERSION {
            eprintln!(
                "warning: config.yaml has version {}, but this wsp only supports version {}. Some fields may be ignored.",
                cfg.version, CURRENT_CONFIG_VERSION
            );
        }

        // Migrate experimental shell values to top-level fields
        if let Some(ref exp) = cfg.experimental {
            if cfg.shell_tmux.is_none()
                && let Some(mode) = exp.shell_tmux_mode()
            {
                cfg.shell_tmux = Some(mode.to_string());
            }
            if cfg.shell_prompt.is_none() && exp.is_feature_enabled("shell-prompt") {
                cfg.shell_prompt = Some(true);
            }
        }

        Ok(cfg)
    }

    /// Hardcoded defaults for git config applied to each clone.
    pub fn default_git_config() -> BTreeMap<String, String> {
        BTreeMap::from([
            ("push.autoSetupRemote".into(), "true".into()),
            ("push.default".into(), "current".into()),
            ("rerere.enabled".into(), "true".into()),
            ("branch.sort".into(), "-committerdate".into()),
        ])
    }

    /// Effective git config: hardcoded defaults merged with user overrides.
    /// User values win over defaults.
    pub fn effective_git_config(&self) -> BTreeMap<String, String> {
        let mut result = Self::default_git_config();
        if let Some(ref overrides) = self.git_config {
            for (k, v) in overrides {
                result.insert(k.clone(), v.clone());
            }
        }
        result
    }

    /// Resolves the effective shell-tmux mode. Checks top-level `shell_tmux` first,
    /// falls back to legacy `experimental.shell-tmux`.
    pub fn shell_tmux_mode(&self) -> Option<&str> {
        if let Some(ref mode) = self.shell_tmux {
            if !mode.eq_ignore_ascii_case("false") && !mode.is_empty() {
                return Some(mode);
            }
            return None;
        }
        self.experimental.as_ref().and_then(|e| e.shell_tmux_mode())
    }

    /// Returns true if shell-prompt is enabled. Checks top-level `shell_prompt` first,
    /// falls back to legacy `experimental.shell-prompt`.
    pub fn shell_prompt_enabled(&self) -> bool {
        if let Some(enabled) = self.shell_prompt {
            return enabled;
        }
        self.experimental
            .as_ref()
            .is_some_and(|e| e.is_feature_enabled("shell-prompt"))
    }

    pub fn upstream_url(&self, identity: &str) -> Option<&str> {
        self.repos.get(identity).map(|e| e.url.as_str())
    }

    pub fn save_to(&self, path: &Path) -> Result<()> {
        let dir = path.parent().context("config path has no parent")?;
        fs::create_dir_all(dir)?;

        let data = serde_yaml_ng::to_string(self)?;
        let mut tmp =
            tempfile::NamedTempFile::new_in(dir).context("creating temp file for atomic save")?;
        tmp.write_all(data.as_bytes())
            .context("writing config to temp file")?;
        tmp.persist(path).context("renaming temp file to config")?;
        Ok(())
    }
}

pub struct Paths {
    pub config_path: PathBuf,
    pub mirrors_dir: PathBuf,
    pub workspaces_dir: PathBuf,
    pub gc_dir: PathBuf,
    pub templates_dir: PathBuf,
}

impl Paths {
    /// Resolve paths from environment (XDG_DATA_HOME / HOME). Called once at startup.
    /// Loads config to check for a `workspaces_dir` override before falling back to default.
    pub fn resolve() -> Result<Paths> {
        let data = data_dir()?;
        let config_path = data.join("config.yaml");
        let cfg = Config::load_from(&config_path)?;
        let workspaces_dir = match cfg.workspaces_dir {
            Some(ref dir) => PathBuf::from(dir),
            None => default_workspaces_dir()?,
        };
        Ok(Paths {
            config_path,
            mirrors_dir: data.join("mirrors"),
            gc_dir: data.join("gc"),
            templates_dir: data.join("templates"),
            workspaces_dir,
        })
    }

    /// The data directory (parent of config.yaml).
    pub fn data_dir(&self) -> &Path {
        self.config_path
            .parent()
            .expect("config_path must have a parent directory; this is a programming error")
    }

    /// Construct paths from explicit directories. Used in tests.
    #[cfg(test)]
    pub fn from_dirs(data_dir: &Path, workspaces_dir: &Path) -> Paths {
        Paths {
            config_path: data_dir.join("config.yaml"),
            mirrors_dir: data_dir.join("mirrors"),
            gc_dir: data_dir.join("gc"),
            templates_dir: data_dir.join("templates"),
            workspaces_dir: workspaces_dir.to_path_buf(),
        }
    }
}

/// Resolves the ws data directory. Accepts injectable overrides for testing.
pub fn data_dir_with(xdg_data_home: Option<&str>, home: Option<&Path>) -> Result<PathBuf> {
    if let Some(xdg) = xdg_data_home.filter(|s| !s.is_empty()) {
        return Ok(PathBuf::from(xdg).join("wsp"));
    }
    let home = home.context("cannot determine home directory")?;
    Ok(home.join(".local").join("share").join("wsp"))
}

fn data_dir() -> Result<PathBuf> {
    data_dir_with(
        std::env::var("XDG_DATA_HOME").ok().as_deref(),
        dirs::home_dir().as_deref(),
    )
}

/// Resolves the default workspaces directory. Accepts injectable home for testing.
pub fn default_workspaces_dir_with(home: Option<&Path>) -> Result<PathBuf> {
    let home = home.context("cannot determine home directory")?;
    Ok(home.join("dev").join("workspaces"))
}

fn default_workspaces_dir() -> Result<PathBuf> {
    default_workspaces_dir_with(dirs::home_dir().as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn test_data_dir_xdg_set() {
        let dir = data_dir_with(Some("/custom/data"), None).unwrap();
        assert_eq!(dir, PathBuf::from("/custom/data/wsp"));
    }

    #[test]
    fn test_data_dir_xdg_empty_falls_back_to_home() {
        let dir = data_dir_with(Some(""), Some(Path::new("/home/user"))).unwrap();
        assert_eq!(dir, PathBuf::from("/home/user/.local/share/wsp"));
    }

    #[test]
    fn test_data_dir_no_xdg_uses_home() {
        let dir = data_dir_with(None, Some(Path::new("/home/user"))).unwrap();
        assert_eq!(dir, PathBuf::from("/home/user/.local/share/wsp"));
    }

    #[test]
    fn test_data_dir_no_home_errors() {
        assert!(data_dir_with(None, None).is_err());
    }

    #[test]
    fn test_config_path() {
        // Uses real env, just verify it ends with the right suffix
        let p = data_dir_with(Some("/custom/data"), None)
            .unwrap()
            .join("config.yaml");
        assert_eq!(p, PathBuf::from("/custom/data/wsp/config.yaml"));
    }

    #[test]
    fn test_mirrors_dir() {
        let dir = data_dir_with(Some("/custom/data"), None)
            .unwrap()
            .join("mirrors");
        assert_eq!(dir, PathBuf::from("/custom/data/wsp/mirrors"));
    }

    #[test]
    fn test_default_workspaces_dir() {
        let dir = default_workspaces_dir_with(Some(Path::new("/home/user"))).unwrap();
        assert_eq!(dir, PathBuf::from("/home/user/dev/workspaces"));
    }

    #[test]
    fn test_load_save_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg_path = tmp.path().join("config.yaml");

        // Load should return empty config when file doesn't exist
        let mut cfg = Config::load_from(&cfg_path).unwrap();
        assert!(cfg.repos.is_empty());

        // Add data
        let now = Utc.with_ymd_and_hms(2025, 1, 15, 10, 0, 0).unwrap();
        cfg.repos.insert(
            "github.com/user/repo-a".into(),
            RepoEntry {
                url: "git@github.com:user/repo-a.git".into(),
                added: now,
            },
        );
        cfg.repos.insert(
            "github.com/user/repo-b".into(),
            RepoEntry {
                url: "git@github.com:user/repo-b.git".into(),
                added: now,
            },
        );

        cfg.save_to(&cfg_path).unwrap();

        // Verify file exists
        assert!(cfg_path.exists());

        // Load again
        let cfg2 = Config::load_from(&cfg_path).unwrap();
        assert_eq!(cfg2.repos.len(), 2);
        assert_eq!(
            cfg2.repos["github.com/user/repo-a"].url,
            "git@github.com:user/repo-a.git"
        );
        assert_eq!(cfg2.repos["github.com/user/repo-a"].added, now);
    }

    #[test]
    fn test_load_save_round_trip_with_language_integrations() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg_path = tmp.path().join("config.yaml");

        let mut cfg = Config::default();
        let mut li = BTreeMap::new();
        li.insert("go".into(), true);
        li.insert("npm".into(), false);
        cfg.language_integrations = Some(li);

        cfg.save_to(&cfg_path).unwrap();
        let cfg2 = Config::load_from(&cfg_path).unwrap();

        let li2 = cfg2.language_integrations.unwrap();
        assert_eq!(li2["go"], true);
        assert_eq!(li2["npm"], false);
    }

    #[test]
    fn test_backward_compat_no_language_integrations() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg_path = tmp.path().join("config.yaml");

        // Write a config without language_integrations field
        std::fs::write(&cfg_path, "branch_prefix: test\n").unwrap();

        let cfg = Config::load_from(&cfg_path).unwrap();
        assert!(cfg.language_integrations.is_none());
    }

    #[test]
    fn test_load_nonexistent_file() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg_path = tmp.path().join("config.yaml");

        let cfg = Config::load_from(&cfg_path).unwrap();
        assert!(cfg.repos.is_empty());
    }

    #[test]
    fn test_backward_compat_no_workspaces_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg_path = tmp.path().join("config.yaml");

        std::fs::write(&cfg_path, "branch_prefix: test\n").unwrap();

        let cfg = Config::load_from(&cfg_path).unwrap();
        assert!(cfg.workspaces_dir.is_none());
    }

    #[test]
    fn test_workspaces_dir_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg_path = tmp.path().join("config.yaml");

        let mut cfg = Config::default();
        cfg.workspaces_dir = Some("/home/user/projects".into());
        cfg.save_to(&cfg_path).unwrap();

        let cfg2 = Config::load_from(&cfg_path).unwrap();
        assert_eq!(cfg2.workspaces_dir.as_deref(), Some("/home/user/projects"));
    }

    #[test]
    fn test_resolve_with_workspaces_dir_override() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path().join("data");
        std::fs::create_dir_all(&data_dir).unwrap();
        let cfg_path = data_dir.join("config.yaml");

        let mut cfg = Config::default();
        cfg.workspaces_dir = Some("/custom/workspaces".into());
        cfg.save_to(&cfg_path).unwrap();

        // Simulate what Paths::resolve does: load config, use override
        let loaded = Config::load_from(&cfg_path).unwrap();
        let ws_dir = match loaded.workspaces_dir {
            Some(ref dir) => PathBuf::from(dir),
            None => PathBuf::from("/default/workspaces"),
        };
        assert_eq!(ws_dir, PathBuf::from("/custom/workspaces"));
    }

    #[test]
    fn test_resolve_without_workspaces_dir_uses_default() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path().join("data");
        std::fs::create_dir_all(&data_dir).unwrap();
        let cfg_path = data_dir.join("config.yaml");

        let cfg = Config::default();
        cfg.save_to(&cfg_path).unwrap();

        let loaded = Config::load_from(&cfg_path).unwrap();
        assert!(loaded.workspaces_dir.is_none());

        let ws_dir = default_workspaces_dir_with(Some(Path::new("/home/user"))).unwrap();
        assert_eq!(ws_dir, PathBuf::from("/home/user/dev/workspaces"));
    }

    #[test]
    fn test_version_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg_path = tmp.path().join("config.yaml");

        let cfg = Config::default();
        assert_eq!(cfg.version, 0);
        cfg.save_to(&cfg_path).unwrap();

        // version 0 should be omitted from YAML via skip_serializing_if
        let yaml = std::fs::read_to_string(&cfg_path).unwrap();
        assert!(
            !yaml.contains("version"),
            "version 0 should be omitted from YAML"
        );

        let loaded = Config::load_from(&cfg_path).unwrap();
        assert_eq!(loaded.version, 0);
    }

    #[test]
    fn test_backward_compat_no_version() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg_path = tmp.path().join("config.yaml");

        std::fs::write(&cfg_path, "branch_prefix: test\n").unwrap();

        let cfg = Config::load_from(&cfg_path).unwrap();
        assert_eq!(cfg.version, 0);
    }

    #[test]
    fn test_unknown_version_loads() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg_path = tmp.path().join("config.yaml");

        std::fs::write(&cfg_path, "version: 99\nbranch_prefix: test\n").unwrap();

        let cfg = Config::load_from(&cfg_path).unwrap();
        assert_eq!(cfg.version, 99);
        assert_eq!(cfg.branch_prefix.as_deref(), Some("test"));
    }

    #[test]
    fn test_default_git_config() {
        let defaults = Config::default_git_config();
        assert_eq!(defaults.get("push.autoSetupRemote").unwrap(), "true");
        assert_eq!(defaults.get("push.default").unwrap(), "current");
        assert_eq!(defaults.get("rerere.enabled").unwrap(), "true");
        assert_eq!(defaults.get("branch.sort").unwrap(), "-committerdate");
    }

    #[test]
    fn test_effective_git_config_defaults_only() {
        let cfg = Config::default();
        let effective = cfg.effective_git_config();
        assert_eq!(effective, Config::default_git_config());
    }

    #[test]
    fn test_effective_git_config_with_overrides() {
        let mut cfg = Config::default();
        let mut overrides = BTreeMap::new();
        overrides.insert("push.autoSetupRemote".into(), "false".into());
        overrides.insert("merge.conflictstyle".into(), "zdiff3".into());
        cfg.git_config = Some(overrides);

        let effective = cfg.effective_git_config();
        // Override wins
        assert_eq!(effective.get("push.autoSetupRemote").unwrap(), "false");
        // Custom key included
        assert_eq!(effective.get("merge.conflictstyle").unwrap(), "zdiff3");
        // Other defaults preserved
        assert_eq!(effective.get("push.default").unwrap(), "current");
    }

    #[test]
    fn test_git_config_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg_path = tmp.path().join("config.yaml");

        let mut cfg = Config::default();
        let mut gc = BTreeMap::new();
        gc.insert("push.autoSetupRemote".into(), "false".into());
        cfg.git_config = Some(gc);
        cfg.save_to(&cfg_path).unwrap();

        let loaded = Config::load_from(&cfg_path).unwrap();
        let gc = loaded.git_config.unwrap();
        assert_eq!(gc["push.autoSetupRemote"], "false");
    }

    #[test]
    fn test_backward_compat_no_git_config() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg_path = tmp.path().join("config.yaml");
        std::fs::write(&cfg_path, "branch_prefix: test\n").unwrap();

        let cfg = Config::load_from(&cfg_path).unwrap();
        assert!(cfg.git_config.is_none());
        // Effective still returns defaults
        let effective = cfg.effective_git_config();
        assert_eq!(effective.get("push.autoSetupRemote").unwrap(), "true");
    }

    #[test]
    fn test_experimental_default_none() {
        let cfg = Config::default();
        assert!(cfg.experimental.is_none());
    }

    #[test]
    fn test_experimental_is_feature_enabled() {
        let mut exp = ExperimentalConfig::default();
        // Gate off, feature off → false
        assert!(!exp.is_feature_enabled("shell-prompt"));

        // Gate off, feature on → false
        exp.features
            .insert("shell-prompt".into(), ExperimentalValue::Bool(true));
        assert!(!exp.is_feature_enabled("shell-prompt"));

        // Gate on, feature on → true
        exp.enabled = true;
        assert!(exp.is_feature_enabled("shell-prompt"));

        // Gate on, feature off → false
        assert!(!exp.is_feature_enabled("shell-tmux"));
    }

    #[test]
    fn test_experimental_string_value() {
        let mut exp = ExperimentalConfig::default();
        exp.enabled = true;
        exp.features.insert(
            "shell-tmux".into(),
            ExperimentalValue::String("window-title".into()),
        );
        assert!(exp.is_feature_enabled("shell-tmux"));
        assert_eq!(exp.feature_value("shell-tmux"), Some("window-title"));
    }

    #[test]
    fn test_shell_tmux_mode_new_key() {
        let mut exp = ExperimentalConfig::default();
        exp.enabled = true;
        exp.features.insert(
            "shell-tmux".into(),
            ExperimentalValue::String("window-title".into()),
        );
        assert_eq!(exp.shell_tmux_mode(), Some("window-title"));
    }

    #[test]
    fn test_shell_tmux_mode_deprecated_key() {
        let mut exp = ExperimentalConfig::default();
        exp.enabled = true;
        exp.features
            .insert("shell-tmux-title".into(), ExperimentalValue::Bool(true));
        assert_eq!(exp.shell_tmux_mode(), Some("window-title"));
    }

    #[test]
    fn test_shell_tmux_mode_new_key_overrides_deprecated() {
        let mut exp = ExperimentalConfig::default();
        exp.enabled = true;
        exp.features.insert(
            "shell-tmux".into(),
            ExperimentalValue::String("false".into()),
        );
        exp.features
            .insert("shell-tmux-title".into(), ExperimentalValue::Bool(true));
        // New key wins even if deprecated key is true
        assert_eq!(exp.shell_tmux_mode(), None);
    }

    #[test]
    fn test_experimental_migration() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg_path = tmp.path().join("config.yaml");

        // Write old-format config with experimental section directly
        fs::write(
            &cfg_path,
            "experimental:\n  enabled: true\n  shell-prompt: true\n  shell-tmux: window-title\n",
        )
        .unwrap();

        // load_from migrates experimental values to top-level fields
        let loaded = Config::load_from(&cfg_path).unwrap();
        assert!(loaded.shell_prompt_enabled());
        assert_eq!(loaded.shell_tmux_mode(), Some("window-title"));
        // Top-level fields populated
        assert_eq!(loaded.shell_prompt, Some(true));
        assert_eq!(loaded.shell_tmux.as_deref(), Some("window-title"));
    }

    #[test]
    fn test_experimental_not_written_on_save() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg_path = tmp.path().join("config.yaml");

        let mut cfg = Config::default();
        cfg.shell_tmux = Some("window-title".into());
        cfg.shell_prompt = Some(true);
        cfg.save_to(&cfg_path).unwrap();

        let yaml = fs::read_to_string(&cfg_path).unwrap();
        assert!(
            !yaml.contains("experimental"),
            "experimental section should not be written"
        );
        assert!(
            yaml.contains("shell_tmux"),
            "shell_tmux should be in output"
        );
        assert!(
            yaml.contains("shell_prompt"),
            "shell_prompt should be in output"
        );
    }

    #[test]
    fn test_backward_compat_no_experimental() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg_path = tmp.path().join("config.yaml");
        fs::write(&cfg_path, "branch_prefix: test\n").unwrap();

        let cfg = Config::load_from(&cfg_path).unwrap();
        assert!(cfg.experimental.is_none());
    }

    #[test]
    fn validate_git_config_key_allows_safe_keys() {
        for key in &[
            "push.default",
            "push.autoSetupRemote",
            "pull.rebase",
            "user.email",
        ] {
            assert!(
                validate_git_config_key(key).is_ok(),
                "expected '{}' to be allowed",
                key
            );
        }
    }

    #[test]
    fn validate_git_config_key_blocks_dangerous_keys() {
        let dangerous = [
            "core.sshCommand",
            "core.SSHCOMMAND", // case-insensitive
            "core.hooksPath",
            "core.pager",
            "core.fsmonitor",
            "core.askpass",
            "diff.external",
            "credential.helper",
            "filter.lfs.clean", // prefix match
            "filter.lfs.smudge",
            "mergetool.vimdiff.cmd",
            "difftool.vimdiff.cmd",
            "uploadpack.packObjectsHook",
            "remote.origin.preFetch",
            "url.https://evil.com.insteadOf",
        ];
        for key in &dangerous {
            assert!(
                validate_git_config_key(key).is_err(),
                "expected '{}' to be blocked",
                key
            );
        }
    }

    // R018: case-insensitive "false" comparisons

    #[test]
    fn test_experimental_value_is_truthy_false_case_insensitive() {
        // "FALSE", "False", "FALSE" should all be treated as falsy
        for variant in &["FALSE", "False", "fAlSe"] {
            let v = ExperimentalValue::String((*variant).to_string());
            assert!(
                !v.is_truthy(),
                "expected {:?} to be falsy (case-insensitive)",
                variant
            );
        }
        // "false" stays falsy
        assert!(!ExperimentalValue::String("false".into()).is_truthy());
        // non-"false" strings remain truthy
        assert!(ExperimentalValue::String("window-title".into()).is_truthy());
    }

    #[test]
    fn test_shell_tmux_mode_false_case_insensitive() {
        // Config::shell_tmux_mode should treat "FALSE"/"False" as disabled
        for variant in &["FALSE", "False", "fAlSe"] {
            let cfg = Config {
                shell_tmux: Some((*variant).to_string()),
                ..Config::default()
            };
            assert_eq!(
                cfg.shell_tmux_mode(),
                None,
                "expected shell_tmux={:?} to resolve to None",
                variant
            );
        }
        // Regular value still works
        let cfg = Config {
            shell_tmux: Some("window-title".into()),
            ..Config::default()
        };
        assert_eq!(cfg.shell_tmux_mode(), Some("window-title"));
    }

    #[test]
    fn test_experimental_shell_tmux_mode_false_case_insensitive() {
        // ExperimentalConfig::shell_tmux_mode should treat "FALSE" as disabled
        for variant in &["FALSE", "False"] {
            let mut exp = ExperimentalConfig::default();
            exp.enabled = true;
            exp.features.insert(
                "shell-tmux".into(),
                ExperimentalValue::String((*variant).to_string()),
            );
            assert_eq!(
                exp.shell_tmux_mode(),
                None,
                "expected shell-tmux={:?} to resolve to None",
                variant
            );
        }
    }
}
