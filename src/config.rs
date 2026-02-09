use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoEntry {
    pub url: String,
    pub added: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupEntry {
    pub repos: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch_prefix: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub repos: BTreeMap<String, RepoEntry>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub groups: BTreeMap<String, GroupEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language_integrations: Option<BTreeMap<String, bool>>,
}

impl Config {
    pub fn load_from(path: &Path) -> Result<Config> {
        if !path.exists() {
            return Ok(Config::default());
        }

        let data = fs::read_to_string(path)?;
        let cfg: Config = serde_yml::from_str(&data)?;
        Ok(cfg)
    }

    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }

        let data = serde_yml::to_string(self)?;
        fs::write(path, data)?;
        Ok(())
    }
}

pub struct Paths {
    pub config_path: PathBuf,
    pub mirrors_dir: PathBuf,
    pub workspaces_dir: PathBuf,
}

impl Paths {
    /// Resolve paths from environment (XDG_DATA_HOME / HOME). Called once at startup.
    pub fn resolve() -> Result<Paths> {
        let data = data_dir()?;
        let workspaces_dir = default_workspaces_dir()?;
        Ok(Paths {
            config_path: data.join("config.yaml"),
            mirrors_dir: data.join("mirrors"),
            workspaces_dir,
        })
    }

    /// Construct paths from explicit directories. Used in tests.
    #[cfg(test)]
    pub fn from_dirs(data_dir: &Path, workspaces_dir: &Path) -> Paths {
        Paths {
            config_path: data_dir.join("config.yaml"),
            mirrors_dir: data_dir.join("mirrors"),
            workspaces_dir: workspaces_dir.to_path_buf(),
        }
    }
}

/// Resolves the ws data directory. Accepts injectable overrides for testing.
pub fn data_dir_with(xdg_data_home: Option<&str>, home: Option<&Path>) -> Result<PathBuf> {
    if let Some(xdg) = xdg_data_home.filter(|s| !s.is_empty()) {
        return Ok(PathBuf::from(xdg).join("ws"));
    }
    let home = home.context("cannot determine home directory")?;
    Ok(home.join(".local").join("share").join("ws"))
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
        assert_eq!(dir, PathBuf::from("/custom/data/ws"));
    }

    #[test]
    fn test_data_dir_xdg_empty_falls_back_to_home() {
        let dir = data_dir_with(Some(""), Some(Path::new("/home/user"))).unwrap();
        assert_eq!(dir, PathBuf::from("/home/user/.local/share/ws"));
    }

    #[test]
    fn test_data_dir_no_xdg_uses_home() {
        let dir = data_dir_with(None, Some(Path::new("/home/user"))).unwrap();
        assert_eq!(dir, PathBuf::from("/home/user/.local/share/ws"));
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
        assert_eq!(p, PathBuf::from("/custom/data/ws/config.yaml"));
    }

    #[test]
    fn test_mirrors_dir() {
        let dir = data_dir_with(Some("/custom/data"), None)
            .unwrap()
            .join("mirrors");
        assert_eq!(dir, PathBuf::from("/custom/data/ws/mirrors"));
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
        assert!(cfg.groups.is_empty());

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
        cfg.groups.insert(
            "backend".into(),
            GroupEntry {
                repos: vec![
                    "github.com/user/repo-a".into(),
                    "github.com/user/repo-b".into(),
                ],
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
        assert_eq!(cfg2.groups.len(), 1);
        assert_eq!(
            cfg2.groups["backend"].repos,
            vec!["github.com/user/repo-a", "github.com/user/repo-b"]
        );
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
        assert!(cfg.groups.is_empty());
    }
}
