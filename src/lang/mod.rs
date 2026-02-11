mod go;

use std::path::Path;

use anyhow::Result;

use crate::config::Config;
use crate::workspace::Metadata;

pub trait LanguageIntegration {
    fn name(&self) -> &str;
    fn detect(&self, ws_dir: &Path, metadata: &Metadata) -> bool;
    fn apply(&self, ws_dir: &Path, metadata: &Metadata) -> Result<()>;
}

fn all_integrations() -> Vec<Box<dyn LanguageIntegration>> {
    vec![Box::new(go::GoIntegration)]
}

/// Returns the names of all known language integrations.
pub fn integration_names() -> Vec<String> {
    all_integrations()
        .iter()
        .map(|i| i.name().to_string())
        .collect()
}

/// Runs all enabled language integrations for the given workspace.
/// Failures produce warnings via eprintln, never abort the workspace operation.
pub fn run_integrations(ws_dir: &Path, metadata: &Metadata, config: &Config) {
    for integration in all_integrations() {
        let name = integration.name();

        // Check config: absent key = enabled, explicit false = disabled
        let enabled = config
            .language_integrations
            .as_ref()
            .and_then(|m| m.get(name))
            .copied()
            .unwrap_or(true);

        if !enabled {
            continue;
        }

        if !integration.detect(ws_dir, metadata) {
            continue;
        }

        if let Err(e) = integration.apply(ws_dir, metadata) {
            eprintln!("warning: {} integration failed: {}", name, e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::fs;

    use chrono::Utc;

    use crate::workspace::Metadata;

    fn make_metadata(repos: &[&str]) -> Metadata {
        let mut map = BTreeMap::new();
        for id in repos {
            map.insert(id.to_string(), None);
        }
        Metadata {
            name: "test".into(),
            branch: "test".into(),
            repos: map,
            created: Utc::now(),
            dirs: BTreeMap::new(),
        }
    }

    #[test]
    fn test_run_integrations_default_enabled() {
        let tmp = tempfile::tempdir().unwrap();
        let ws_dir = tmp.path();

        let repo_dir = ws_dir.join("api-gateway");
        fs::create_dir_all(&repo_dir).unwrap();
        fs::write(
            repo_dir.join("go.mod"),
            "module example.com/api-gateway\n\ngo 1.22\n",
        )
        .unwrap();

        let meta = make_metadata(&["github.com/acme/api-gateway"]);
        let cfg = Config::default();

        run_integrations(ws_dir, &meta, &cfg);

        assert!(ws_dir.join("go.work").exists());
    }

    #[test]
    fn test_run_integrations_disabled() {
        let tmp = tempfile::tempdir().unwrap();
        let ws_dir = tmp.path();

        let repo_dir = ws_dir.join("api-gateway");
        fs::create_dir_all(&repo_dir).unwrap();
        fs::write(
            repo_dir.join("go.mod"),
            "module example.com/api-gateway\n\ngo 1.22\n",
        )
        .unwrap();

        let meta = make_metadata(&["github.com/acme/api-gateway"]);
        let mut cfg = Config::default();
        let mut li = BTreeMap::new();
        li.insert("go".into(), false);
        cfg.language_integrations = Some(li);

        run_integrations(ws_dir, &meta, &cfg);

        assert!(!ws_dir.join("go.work").exists());
    }

    #[test]
    fn test_run_integrations_explicit_enabled() {
        let tmp = tempfile::tempdir().unwrap();
        let ws_dir = tmp.path();

        let repo_dir = ws_dir.join("api-gateway");
        fs::create_dir_all(&repo_dir).unwrap();
        fs::write(
            repo_dir.join("go.mod"),
            "module example.com/api-gateway\n\ngo 1.22\n",
        )
        .unwrap();

        let meta = make_metadata(&["github.com/acme/api-gateway"]);
        let mut cfg = Config::default();
        let mut li = BTreeMap::new();
        li.insert("go".into(), true);
        cfg.language_integrations = Some(li);

        run_integrations(ws_dir, &meta, &cfg);

        assert!(ws_dir.join("go.work").exists());
    }

    #[test]
    fn test_run_integrations_no_detection() {
        let tmp = tempfile::tempdir().unwrap();
        let ws_dir = tmp.path();

        let repo_dir = ws_dir.join("frontend");
        fs::create_dir_all(&repo_dir).unwrap();
        fs::write(repo_dir.join("package.json"), "{}").unwrap();

        let meta = make_metadata(&["github.com/acme/frontend"]);
        let cfg = Config::default();

        run_integrations(ws_dir, &meta, &cfg);

        assert!(!ws_dir.join("go.work").exists());
    }
}
