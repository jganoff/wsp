//! Contextual hints (git-style `advice.*` system).
//!
//! Hints are suppressed globally with `hints = false` or individually with
//! `advice.<key> = false`. No hint fires unless it is contextually relevant
//! to the command that just ran.

use wsp_core::config::Config;

/// All known `advice.*` keys. Used to validate user input in `wsp config set advice.<key>`.
pub const KNOWN_ADVICE_KEYS: &[&str] = &["branchPrefix", "setupCommands"];

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
}
