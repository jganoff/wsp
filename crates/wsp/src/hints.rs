//! Contextual hints (git-style `advice.*` system).
//!
//! Hints are suppressed globally with `hints = false` or individually with
//! `advice.<key> = false`. No hint fires unless it is contextually relevant
//! to the command that just ran.

use wsp_core::config::Config;

/// All known `advice.*` keys. Used to validate user input in `wsp config set advice.<key>`.
pub const KNOWN_ADVICE_KEYS: &[&str] = &["branchPrefix", "setupCommands", "registrySetupCommands"];

/// Evaluate contextual hints for the completed command.
///
/// `command` is the effective subcommand path: `"new"`, `"repo/add"`, etc.
/// Returns hint strings to print to stderr. Empty when all hints are suppressed.
pub fn evaluate(command: &str, cfg: &Config) -> Vec<&'static str> {
    if !cfg.hints.unwrap_or(true) {
        return vec![];
    }

    let mut hints = Vec::new();

    // branchPrefix: after `wsp new`, if no branch prefix is configured
    if command == "new" && cfg.branch_prefix.is_none() && hint_enabled(cfg, "branchPrefix") {
        hints.push(
            "hint: set a branch prefix so workspace branches are namespaced under your name:\n  \
             wsp config set branch-prefix <name>\n  \
             (suppress: wsp config set advice.branchPrefix false)",
        );
    }

    // setupCommands: after `wsp new` or `wsp repo add`, suggest `wsp init`
    if (command == "new" || command == "repo/add") && hint_enabled(cfg, "setupCommands") {
        hints.push(
            "hint: repos can declare post-clone setup commands via .wsp.yaml.\n  \
             Run `wsp init` in a repo root to configure them. See `wsp help wsp.yaml`.\n  \
             (suppress: wsp config set advice.setupCommands false)",
        );
    }

    // registrySetupCommands: after `wsp repo setup-commands add` to registry scope,
    // warn when the registry has setup commands (they affect all workspaces, past and future).
    // The command path has a scope suffix when an explicit flag was used; skip the hint
    // for --workspace and --repo to avoid false positives when the user already scoped narrowly.
    if matches!(
        command,
        "repo/setup-commands/add" | "repo/setup-commands/add/registry"
    ) && cfg
        .repos
        .values()
        .any(|e| e.setup_commands.as_ref().is_some_and(|v| !v.is_empty()))
        && hint_enabled(cfg, "registrySetupCommands")
    {
        hints.push(
            "hint: registry setup commands run in every workspace that contains this repo.\n  \
             Use --workspace to limit commands to the current workspace only.\n  \
             (suppress: wsp config set advice.registrySetupCommands false)",
        );
    }

    hints
}

fn hint_enabled(cfg: &Config, key: &str) -> bool {
    cfg.advice
        .as_ref()
        .and_then(|m| m.get(key))
        .copied()
        .unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn cfg_with_prefix() -> Config {
        Config {
            branch_prefix: Some("jg".into()),
            ..Config::default()
        }
    }

    #[test]
    fn no_hints_when_disabled_globally() {
        let mut cfg = Config::default();
        cfg.hints = Some(false);
        assert!(evaluate("new", &cfg).is_empty());
    }

    #[test]
    fn branch_prefix_hint_fires_on_new_without_prefix() {
        let cfg = Config::default(); // branch_prefix is None
        let hints = evaluate("new", &cfg);
        assert!(
            hints.iter().any(|h| h.contains("branchPrefix")),
            "expected branchPrefix hint, got: {:?}",
            hints
        );
    }

    #[test]
    fn branch_prefix_hint_suppressed_when_prefix_set() {
        let cfg = cfg_with_prefix();
        let hints = evaluate("new", &cfg);
        assert!(
            !hints.iter().any(|h| h.contains("branchPrefix")),
            "branchPrefix hint should not fire when prefix is set"
        );
    }

    #[test]
    fn branch_prefix_hint_suppressed_via_advice() {
        let mut cfg = Config::default();
        cfg.advice = Some({
            let mut m = BTreeMap::new();
            m.insert("branchPrefix".into(), false);
            m
        });
        let hints = evaluate("new", &cfg);
        assert!(
            !hints.iter().any(|h| h.contains("branchPrefix")),
            "branchPrefix hint should be suppressed by advice key"
        );
    }

    #[test]
    fn setup_commands_hint_fires_on_new_and_repo_add() {
        let cfg = Config::default();
        for cmd in &["new", "repo/add"] {
            let hints = evaluate(cmd, &cfg);
            assert!(
                hints.iter().any(|h| h.contains("setupCommands")),
                "expected setupCommands hint for command {:?}, got: {:?}",
                cmd,
                hints
            );
        }
    }

    #[test]
    fn setup_commands_hint_suppressed_via_advice() {
        let mut cfg = Config::default();
        cfg.advice = Some({
            let mut m = BTreeMap::new();
            m.insert("setupCommands".into(), false);
            m
        });
        let hints = evaluate("new", &cfg);
        assert!(
            !hints.iter().any(|h| h.contains("setupCommands")),
            "setupCommands hint should be suppressed by advice key"
        );
    }

    #[test]
    fn no_hints_for_unrelated_command() {
        let cfg = Config::default();
        let hints = evaluate("ls", &cfg);
        assert!(hints.is_empty(), "ls should produce no hints");
    }

    // -----------------------------------------------------------------------
    // registrySetupCommands hint
    // -----------------------------------------------------------------------

    use wsp_core::config::RepoEntry;

    fn cfg_with_registry_setup(identity: &str) -> Config {
        let mut repos = BTreeMap::new();
        repos.insert(
            identity.to_string(),
            RepoEntry {
                url: format!("git@test.local:user/{identity}.git"),
                added: chrono::Utc::now(),
                setup_commands: Some(vec!["make deps".into()]),
            },
        );
        Config {
            repos,
            ..Default::default()
        }
    }

    #[test]
    fn registry_setup_hint_fires_after_inferred_add_when_registry_has_commands() {
        let cfg = cfg_with_registry_setup("myrepo");
        let hints = evaluate("repo/setup-commands/add", &cfg);
        assert!(
            hints.iter().any(|h| h.contains("registrySetupCommands")),
            "expected registrySetupCommands hint for inferred add, got: {hints:?}"
        );
    }

    #[test]
    fn registry_setup_hint_fires_after_explicit_registry_add() {
        let cfg = cfg_with_registry_setup("myrepo");
        let hints = evaluate("repo/setup-commands/add/registry", &cfg);
        assert!(
            hints.iter().any(|h| h.contains("registrySetupCommands")),
            "expected registrySetupCommands hint for explicit --registry add, got: {hints:?}"
        );
    }

    #[test]
    fn registry_setup_hint_does_not_fire_for_workspace_or_repo_scope() {
        let cfg = cfg_with_registry_setup("myrepo");
        for cmd in &[
            "repo/setup-commands/add/workspace",
            "repo/setup-commands/add/repo",
        ] {
            let hints = evaluate(cmd, &cfg);
            assert!(
                !hints.iter().any(|h| h.contains("registrySetupCommands")),
                "registrySetupCommands hint should not fire for {cmd:?}, got: {hints:?}"
            );
        }
    }

    #[test]
    fn registry_setup_hint_does_not_fire_on_other_commands() {
        let cfg = cfg_with_registry_setup("myrepo");
        for cmd in &["repo/setup-commands/rm", "repo/setup-commands/ls", "new"] {
            let hints = evaluate(cmd, &cfg);
            assert!(
                !hints.iter().any(|h| h.contains("registrySetupCommands")),
                "registrySetupCommands hint should not fire for {cmd:?}, got: {hints:?}"
            );
        }
    }

    #[test]
    fn registry_setup_hint_does_not_fire_when_registry_empty() {
        // Registry has the repo entry but no setup_commands.
        let mut repos = BTreeMap::new();
        repos.insert(
            "myrepo".to_string(),
            RepoEntry {
                url: "git@test.local:user/myrepo.git".into(),
                added: chrono::Utc::now(),
                setup_commands: None,
            },
        );
        let cfg = Config {
            repos,
            ..Default::default()
        };
        let hints = evaluate("repo/setup-commands/add", &cfg);
        assert!(
            !hints.iter().any(|h| h.contains("registrySetupCommands")),
            "hint should not fire when registry has no setup commands"
        );
    }

    #[test]
    fn registry_setup_hint_suppressed_via_advice() {
        let mut cfg = cfg_with_registry_setup("myrepo");
        cfg.advice = Some({
            let mut m = BTreeMap::new();
            m.insert("registrySetupCommands".into(), false);
            m
        });
        let hints = evaluate("repo/setup-commands/add", &cfg);
        assert!(
            !hints.iter().any(|h| h.contains("registrySetupCommands")),
            "registrySetupCommands hint should be suppressed by advice key"
        );
    }
}
