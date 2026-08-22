//! Multi-layer setup command resolution.
//!
//! Setup commands can be declared at four levels. When a repo is cloned into a
//! workspace, all layers are gathered and concatenated in this order:
//!
//! 1. **Registry** — personal per-repo commands in `~/.local/share/wsp/config.yaml`
//! 2. **Template** — per-repo commands declared on a `TemplateRepo`
//! 3. **Repo**     — committed `setup_commands` in the repo's own `.wsp.yaml`
//! 4. **Workspace** — per-repo overrides in workspace metadata
//!
//! All commands from all layers are included — no deduplication. Setup commands
//! are expected to be idempotent (e.g. `just setup`, `npm install`), so running
//! the same command from two layers is safe and intentional. The final list is
//! what the user approves; its content hash determines whether re-approval is needed.

use std::path::Path;

use crate::config;
use crate::template::{self, Template};
use crate::workspace;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A single layer contributing setup commands.
#[derive(Debug, Clone)]
pub struct SetupSource {
    /// Human-readable label for display: `"registry"`, `"template"`, `"repo"`, `"workspace"`.
    pub label: &'static str,
    /// Commands from this layer.
    pub commands: Vec<String>,
}

/// The merged result of resolving setup commands across all layers.
#[derive(Debug, Clone)]
pub struct ResolvedSetup {
    /// Final command list, in layer order (all layers, no deduplication).
    pub commands: Vec<String>,
    /// Provenance: each entry is `(command, source_label)` in the same order as `commands`.
    pub provenance: Vec<(String, &'static str)>,
}

impl ResolvedSetup {
    /// Returns `true` if there are no commands after resolution.
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    /// Return a new `ResolvedSetup` with duplicate commands removed, keeping
    /// the first occurrence (and its provenance label).
    ///
    /// This is the default behavior for the approval/run flow. Pass the
    /// original (undeduped) value when `--all` is requested.
    pub fn dedup(&self) -> Self {
        use std::collections::HashSet;
        let mut seen = HashSet::new();
        let mut commands = Vec::new();
        let mut provenance = Vec::new();
        for (cmd, label) in &self.provenance {
            if seen.insert(cmd.clone()) {
                commands.push(cmd.clone());
                provenance.push((cmd.clone(), *label));
            }
        }
        Self {
            commands,
            provenance,
        }
    }
}

// ---------------------------------------------------------------------------
// Resolution
// ---------------------------------------------------------------------------

/// Resolve setup commands from multiple layers into a single list.
///
/// Sources are consumed in order; all commands from all layers are included.
/// No deduplication is performed — setup commands are expected to be idempotent.
pub fn resolve(sources: Vec<SetupSource>) -> ResolvedSetup {
    let mut commands = Vec::new();
    let mut provenance = Vec::new();

    for source in sources {
        for cmd in source.commands {
            provenance.push((cmd.clone(), source.label));
            commands.push(cmd);
        }
    }

    ResolvedSetup {
        commands,
        provenance,
    }
}

/// Gather setup commands from all four layers for a single repo and resolve.
///
/// - `cfg`: global config (for registry-level commands on `RepoEntry`)
/// - `tmpl`: the template used to create this workspace (if any)
/// - `meta`: workspace metadata (for workspace-level overrides)
/// - `identity`: the repo identity string
/// - `clone_dir`: the repo's clone directory (to read committed `.wsp.yaml`)
pub fn resolve_for_repo(
    cfg: &config::Config,
    tmpl: Option<&Template>,
    meta: Option<&workspace::Metadata>,
    identity: &str,
    clone_dir: Option<&Path>,
) -> ResolvedSetup {
    let mut sources = Vec::new();

    // 1. Registry
    if let Some(entry) = cfg.repos.get(identity)
        && let Some(ref cmds) = entry.setup_commands
        && !cmds.is_empty()
    {
        sources.push(SetupSource {
            label: "registry",
            commands: cmds.clone(),
        });
    }

    // 2. Template (per-repo)
    if let Some(tmpl) = tmpl {
        for repo in &tmpl.repos {
            if let Some(ref cmds) = repo.setup_commands {
                // Match template repo to this identity by parsing its URL.
                let repo_identity = crate::giturl::parse(&repo.url)
                    .map(|p| p.identity())
                    .unwrap_or_default();
                if repo_identity == identity && !cmds.is_empty() {
                    sources.push(SetupSource {
                        label: "template",
                        commands: cmds.clone(),
                    });
                    break;
                }
            }
        }
    }

    // 3. Repo committed .wsp.yaml (only available when we have a clone dir)
    if let Some(dir) = clone_dir
        && let Some(cmds) = template::read_setup_commands(&dir.join(".wsp.yaml"))
    {
        sources.push(SetupSource {
            label: "repo",
            commands: cmds,
        });
    }

    // 4. Workspace overrides (only available when we have workspace metadata)
    if let Some(meta) = meta
        && let Some(cmds) = meta.setup_commands.get(identity)
        && !cmds.is_empty()
    {
        sources.push(SetupSource {
            label: "workspace",
            commands: cmds.clone(),
        });
    }

    resolve(sources)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_sources() {
        let resolved = resolve(vec![]);
        assert!(resolved.is_empty());
        assert!(resolved.provenance.is_empty());
    }

    #[test]
    fn single_source() {
        let resolved = resolve(vec![SetupSource {
            label: "repo",
            commands: vec!["make deps".into(), "npm install".into()],
        }]);
        assert_eq!(resolved.commands, vec!["make deps", "npm install"]);
        assert_eq!(
            resolved.provenance,
            vec![("make deps".into(), "repo"), ("npm install".into(), "repo"),]
        );
    }

    #[test]
    fn multiple_sources_concatenated_in_order() {
        let resolved = resolve(vec![
            SetupSource {
                label: "registry",
                commands: vec!["lefthook install".into()],
            },
            SetupSource {
                label: "template",
                commands: vec!["npm install".into()],
            },
            SetupSource {
                label: "repo",
                commands: vec!["make deps".into()],
            },
            SetupSource {
                label: "workspace",
                commands: vec!["./extra-setup.sh".into()],
            },
        ]);
        assert_eq!(
            resolved.commands,
            vec![
                "lefthook install",
                "npm install",
                "make deps",
                "./extra-setup.sh"
            ]
        );
        assert_eq!(
            resolved.provenance[0],
            ("lefthook install".into(), "registry")
        );
        assert_eq!(resolved.provenance[1], ("npm install".into(), "template"));
        assert_eq!(resolved.provenance[2], ("make deps".into(), "repo"));
        assert_eq!(
            resolved.provenance[3],
            ("./extra-setup.sh".into(), "workspace")
        );
    }

    #[test]
    fn same_command_in_multiple_layers_runs_twice() {
        let resolved = resolve(vec![
            SetupSource {
                label: "registry",
                commands: vec!["make deps".into(), "lefthook install".into()],
            },
            SetupSource {
                label: "repo",
                commands: vec!["make deps".into(), "npm install".into()],
            },
        ]);
        // "make deps" appears in both layers — no dedup, runs twice.
        assert_eq!(
            resolved.commands,
            vec!["make deps", "lefthook install", "make deps", "npm install"]
        );
        assert_eq!(resolved.provenance[0], ("make deps".into(), "registry"));
        assert_eq!(
            resolved.provenance[1],
            ("lefthook install".into(), "registry")
        );
        assert_eq!(resolved.provenance[2], ("make deps".into(), "repo"));
        assert_eq!(resolved.provenance[3], ("npm install".into(), "repo"));
    }

    #[test]
    fn empty_sources_skipped() {
        let resolved = resolve(vec![
            SetupSource {
                label: "registry",
                commands: vec![],
            },
            SetupSource {
                label: "repo",
                commands: vec!["make deps".into()],
            },
            SetupSource {
                label: "workspace",
                commands: vec![],
            },
        ]);
        assert_eq!(resolved.commands, vec!["make deps"]);
        assert_eq!(resolved.provenance, vec![("make deps".into(), "repo")]);
    }

    #[test]
    fn same_command_in_all_layers_runs_four_times() {
        let resolved = resolve(vec![
            SetupSource {
                label: "registry",
                commands: vec!["npm install".into()],
            },
            SetupSource {
                label: "template",
                commands: vec!["npm install".into()],
            },
            SetupSource {
                label: "repo",
                commands: vec!["npm install".into()],
            },
            SetupSource {
                label: "workspace",
                commands: vec!["npm install".into()],
            },
        ]);
        assert_eq!(
            resolved.commands,
            vec!["npm install", "npm install", "npm install", "npm install"]
        );
        assert_eq!(resolved.provenance[0], ("npm install".into(), "registry"));
        assert_eq!(resolved.provenance[1], ("npm install".into(), "template"));
        assert_eq!(resolved.provenance[2], ("npm install".into(), "repo"));
        assert_eq!(resolved.provenance[3], ("npm install".into(), "workspace"));
    }

    // -----------------------------------------------------------------------
    // resolve_for_repo tests
    // -----------------------------------------------------------------------

    use std::collections::BTreeMap;

    use crate::config::{Config, RepoEntry};
    use crate::template::{Template, TemplateRepo};
    use crate::workspace::Metadata;

    fn make_config(identity: &str, cmds: Option<Vec<String>>) -> Config {
        let mut repos = BTreeMap::new();
        repos.insert(
            identity.to_string(),
            RepoEntry {
                url: format!(
                    "git@test.local:user/{}.git",
                    identity.split('/').next_back().unwrap()
                ),
                added: chrono::Utc::now(),
                setup_commands: cmds,
            },
        );
        Config {
            repos,
            ..Default::default()
        }
    }

    fn make_metadata(identity: &str, cmds: Option<Vec<String>>) -> Metadata {
        let mut repos = BTreeMap::new();
        repos.insert(identity.to_string(), None);
        let mut setup_commands = BTreeMap::new();
        if let Some(cmds) = cmds {
            setup_commands.insert(identity.to_string(), cmds);
        }
        Metadata {
            version: 0,
            name: "test".into(),
            branch: "test/test".into(),
            repos,
            created: chrono::Utc::now(),
            description: None,
            last_used: None,
            created_from: None,
            dirs: BTreeMap::new(),
            config: None,
            setup_commands,
        }
    }

    fn make_template(url: &str, cmds: Option<Vec<String>>) -> Template {
        Template {
            repos: vec![TemplateRepo {
                url: url.to_string(),
                setup_commands: cmds,
            }],
            ..Default::default()
        }
    }

    #[test]
    fn resolve_for_repo_registry_only() {
        let identity = "test.local/user/repo";
        let cfg = make_config(identity, Some(vec!["make deps".into()]));
        let meta = make_metadata(identity, None);
        let dir = std::path::PathBuf::from("/nonexistent");

        let resolved = resolve_for_repo(&cfg, None, Some(&meta), identity, Some(&dir));
        assert_eq!(resolved.commands, vec!["make deps"]);
        assert_eq!(resolved.provenance[0].1, "registry");
    }

    #[test]
    fn resolve_for_repo_workspace_only() {
        let identity = "test.local/user/repo";
        let cfg = make_config(identity, None);
        let meta = make_metadata(identity, Some(vec!["./setup.sh".into()]));
        let dir = std::path::PathBuf::from("/nonexistent");

        let resolved = resolve_for_repo(&cfg, None, Some(&meta), identity, Some(&dir));
        assert_eq!(resolved.commands, vec!["./setup.sh"]);
        assert_eq!(resolved.provenance[0].1, "workspace");
    }

    #[test]
    fn resolve_for_repo_template_only() {
        let identity = "test.local/user/repo";
        let cfg = make_config(identity, None);
        let meta = make_metadata(identity, None);
        let tmpl = make_template(
            "git@test.local:user/repo.git",
            Some(vec!["npm install".into()]),
        );
        let dir = std::path::PathBuf::from("/nonexistent");

        let resolved = resolve_for_repo(&cfg, Some(&tmpl), Some(&meta), identity, Some(&dir));
        assert_eq!(resolved.commands, vec!["npm install"]);
        assert_eq!(resolved.provenance[0].1, "template");
    }

    #[test]
    fn resolve_for_repo_all_layers() {
        let identity = "test.local/user/repo";
        let cfg = make_config(identity, Some(vec!["registry-cmd".into()]));
        let meta = make_metadata(identity, Some(vec!["workspace-cmd".into()]));
        let tmpl = make_template(
            "git@test.local:user/repo.git",
            Some(vec!["template-cmd".into()]),
        );
        // No committed .wsp.yaml (nonexistent dir), so repo layer is empty.
        let dir = std::path::PathBuf::from("/nonexistent");

        let resolved = resolve_for_repo(&cfg, Some(&tmpl), Some(&meta), identity, Some(&dir));
        assert_eq!(
            resolved.commands,
            vec!["registry-cmd", "template-cmd", "workspace-cmd"]
        );
        assert_eq!(resolved.provenance[0], ("registry-cmd".into(), "registry"));
        assert_eq!(resolved.provenance[1], ("template-cmd".into(), "template"));
        assert_eq!(
            resolved.provenance[2],
            ("workspace-cmd".into(), "workspace")
        );
    }

    #[test]
    fn resolve_for_repo_same_command_across_layers_runs_twice() {
        let identity = "test.local/user/repo";
        let cfg = make_config(identity, Some(vec!["npm install".into()]));
        let meta = make_metadata(identity, Some(vec!["npm install".into(), "extra".into()]));
        let dir = std::path::PathBuf::from("/nonexistent");

        let resolved = resolve_for_repo(&cfg, None, Some(&meta), identity, Some(&dir));
        // "npm install" in both registry and workspace — no dedup, runs from both.
        assert_eq!(
            resolved.commands,
            vec!["npm install", "npm install", "extra"]
        );
        assert_eq!(resolved.provenance[0], ("npm install".into(), "registry"));
        assert_eq!(resolved.provenance[1], ("npm install".into(), "workspace"));
        assert_eq!(resolved.provenance[2], ("extra".into(), "workspace"));
    }

    #[test]
    fn resolve_for_repo_empty_all_layers() {
        let identity = "test.local/user/repo";
        let cfg = make_config(identity, None);
        let meta = make_metadata(identity, None);
        let dir = std::path::PathBuf::from("/nonexistent");

        let resolved = resolve_for_repo(&cfg, None, Some(&meta), identity, Some(&dir));
        assert!(resolved.is_empty());
    }

    // -----------------------------------------------------------------------
    // ResolvedSetup::dedup tests
    // -----------------------------------------------------------------------

    #[test]
    fn dedup_empty() {
        let r = resolve(vec![]);
        assert!(r.dedup().is_empty());
    }

    #[test]
    fn dedup_no_duplicates_unchanged() {
        let r = resolve(vec![
            SetupSource {
                label: "registry",
                commands: vec!["make deps".into()],
            },
            SetupSource {
                label: "repo",
                commands: vec!["npm install".into()],
            },
        ]);
        let d = r.dedup();
        assert_eq!(d.commands, vec!["make deps", "npm install"]);
        assert_eq!(d.provenance[0], ("make deps".into(), "registry"));
        assert_eq!(d.provenance[1], ("npm install".into(), "repo"));
    }

    #[test]
    fn dedup_keeps_first_occurrence() {
        let r = resolve(vec![
            SetupSource {
                label: "registry",
                commands: vec!["npm install".into(), "make deps".into()],
            },
            SetupSource {
                label: "repo",
                commands: vec!["npm install".into(), "extra".into()],
            },
        ]);
        let d = r.dedup();
        // "npm install" from registry is kept; repo occurrence dropped.
        assert_eq!(d.commands, vec!["npm install", "make deps", "extra"]);
        assert_eq!(d.provenance[0], ("npm install".into(), "registry"));
        assert_eq!(d.provenance[1], ("make deps".into(), "registry"));
        assert_eq!(d.provenance[2], ("extra".into(), "repo"));
    }

    #[test]
    fn dedup_all_duplicates_collapsed_to_one() {
        let r = resolve(vec![
            SetupSource {
                label: "registry",
                commands: vec!["npm install".into()],
            },
            SetupSource {
                label: "template",
                commands: vec!["npm install".into()],
            },
            SetupSource {
                label: "repo",
                commands: vec!["npm install".into()],
            },
            SetupSource {
                label: "workspace",
                commands: vec!["npm install".into()],
            },
        ]);
        let d = r.dedup();
        assert_eq!(d.commands, vec!["npm install"]);
        assert_eq!(d.provenance[0], ("npm install".into(), "registry"));
    }

    #[test]
    fn dedup_does_not_modify_original() {
        let r = resolve(vec![
            SetupSource {
                label: "registry",
                commands: vec!["cmd".into()],
            },
            SetupSource {
                label: "repo",
                commands: vec!["cmd".into()],
            },
        ]);
        let _d = r.dedup();
        // Original should still have 2 occurrences.
        assert_eq!(r.commands.len(), 2);
    }

    #[test]
    fn resolve_for_repo_template_url_mismatch_ignored() {
        let identity = "test.local/user/repo";
        let cfg = make_config(identity, None);
        let meta = make_metadata(identity, None);
        // Template has commands for a different repo.
        let tmpl = make_template(
            "git@test.local:user/other-repo.git",
            Some(vec!["should-not-appear".into()]),
        );
        let dir = std::path::PathBuf::from("/nonexistent");

        let resolved = resolve_for_repo(&cfg, Some(&tmpl), Some(&meta), identity, Some(&dir));
        assert!(resolved.is_empty());
    }

    // -----------------------------------------------------------------------
    // YAML round-trip tests for new struct fields
    // -----------------------------------------------------------------------

    #[test]
    fn repo_entry_setup_commands_yaml_roundtrip() {
        let entry = RepoEntry {
            url: "git@test.local:user/repo.git".into(),
            added: chrono::Utc::now(),
            setup_commands: Some(vec!["make deps".into(), "npm install".into()]),
        };
        let yaml = serde_yaml_ng::to_string(&entry).unwrap();
        assert!(yaml.contains("setup_commands"));
        assert!(yaml.contains("make deps"));

        let parsed: RepoEntry = serde_yaml_ng::from_str(&yaml).unwrap();
        assert_eq!(parsed.setup_commands, entry.setup_commands);
    }

    #[test]
    fn repo_entry_no_setup_commands_omitted() {
        let entry = RepoEntry {
            url: "git@test.local:user/repo.git".into(),
            added: chrono::Utc::now(),
            setup_commands: None,
        };
        let yaml = serde_yaml_ng::to_string(&entry).unwrap();
        assert!(!yaml.contains("setup_commands"));

        // And it can parse YAML without the field.
        let yaml_without = "url: git@test.local:user/repo.git\nadded: 2026-01-01T00:00:00Z\n";
        let parsed: RepoEntry = serde_yaml_ng::from_str(yaml_without).unwrap();
        assert_eq!(parsed.setup_commands, None);
    }

    #[test]
    fn template_repo_setup_commands_yaml_roundtrip() {
        let repo = TemplateRepo {
            url: "git@test.local:user/repo.git".into(),
            setup_commands: Some(vec!["task setup".into()]),
        };
        let yaml = serde_yaml_ng::to_string(&repo).unwrap();
        assert!(yaml.contains("setup_commands"));

        let parsed: TemplateRepo = serde_yaml_ng::from_str(&yaml).unwrap();
        assert_eq!(parsed.setup_commands, repo.setup_commands);
    }

    #[test]
    fn metadata_setup_commands_yaml_roundtrip() {
        let mut setup_commands = BTreeMap::new();
        setup_commands.insert(
            "test.local/user/repo".to_string(),
            vec!["./setup.sh".into()],
        );
        let meta = Metadata {
            version: 0,
            name: "test".into(),
            branch: "test/test".into(),
            repos: BTreeMap::new(),
            created: chrono::Utc::now(),
            description: None,
            last_used: None,
            created_from: None,
            dirs: BTreeMap::new(),
            config: None,
            setup_commands,
        };
        let yaml = serde_yaml_ng::to_string(&meta).unwrap();
        assert!(yaml.contains("setup_commands"));
        assert!(yaml.contains("./setup.sh"));

        let parsed: Metadata = serde_yaml_ng::from_str(&yaml).unwrap();
        assert_eq!(parsed.setup_commands, meta.setup_commands);
    }

    #[test]
    fn metadata_empty_setup_commands_omitted() {
        let meta = Metadata {
            version: 0,
            name: "test".into(),
            branch: "test/test".into(),
            repos: BTreeMap::new(),
            created: chrono::Utc::now(),
            description: None,
            last_used: None,
            created_from: None,
            dirs: BTreeMap::new(),
            config: None,
            setup_commands: BTreeMap::new(),
        };
        let yaml = serde_yaml_ng::to_string(&meta).unwrap();
        assert!(!yaml.contains("setup_commands"));
    }
}
