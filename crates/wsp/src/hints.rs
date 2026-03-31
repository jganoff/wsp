//! Contextual hints (git-style `advice.*` system).
//!
//! Hints are suppressed globally with `hints = false` or individually with
//! `advice.<key> = false`. Each hint has a cooldown (default 7 days) so it
//! fires at most once per cooldown window even when the triggering condition
//! is met on every run. Set `hints-cooldown-days = 0` to always show hints.

use std::path::Path;
use std::time::Duration;

use wsp_core::config::{Config, Paths};

/// Default cooldown between repeat appearances of the same hint.
pub const DEFAULT_HINT_COOLDOWN_DAYS: u32 = 1;

/// All known `advice.*` keys. Used to validate user input in `wsp config set advice.<key>`.
pub const KNOWN_ADVICE_KEYS: &[&str] = &[
    "branchPrefix",
    "setupCommands",
    "registrySetupCommands",
    "whatsnew",
];

/// Evaluate contextual hints for the completed command.
///
/// `command` is the effective subcommand path: `"new"`, `"repo/add"`, etc.
/// Returns hint strings to print to stderr. Empty when all hints are suppressed.
pub fn evaluate(command: &str, cfg: &Config, paths: &Paths) -> Vec<&'static str> {
    if !cfg.hints.unwrap_or(true) {
        return vec![];
    }

    let cooldown_days = cfg
        .hints_cooldown_days
        .unwrap_or(DEFAULT_HINT_COOLDOWN_DAYS);
    let hints_dir = paths.data_dir().join("hints");

    let mut hints = Vec::new();

    // branchPrefix: after `wsp new`, if no branch prefix is configured
    if command == "new"
        && cfg.branch_prefix.is_none()
        && hint_enabled(cfg, "branchPrefix")
        && hint_ready(&hints_dir, "branchPrefix", cooldown_days)
    {
        hints.push(
            "hint: set a branch prefix so workspace branches are namespaced under your name:\n  \
             wsp config set branch-prefix <name>\n  \
             (suppress: wsp config set advice.branchPrefix false)",
        );
        touch_hint(&hints_dir, "branchPrefix");
    }

    // setupCommands: after `wsp new` or `wsp repo add`, suggest `wsp init`.
    // Auto-suppresses once any registry repo already has setup commands configured
    // (the user has discovered the feature).
    if (command == "new" || command == "repo/add")
        && !cfg
            .repos
            .values()
            .any(|e| e.setup_commands.as_ref().is_some_and(|v| !v.is_empty()))
        && hint_enabled(cfg, "setupCommands")
        && hint_ready(&hints_dir, "setupCommands", cooldown_days)
    {
        hints.push(
            "hint: repos can declare post-clone setup commands via .wsp.yaml.\n  \
             Run `wsp init` in a repo root to configure them. See `wsp help wsp.yaml`.\n  \
             (suppress: wsp config set advice.setupCommands false)",
        );
        touch_hint(&hints_dir, "setupCommands");
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
        && hint_ready(&hints_dir, "registrySetupCommands", cooldown_days)
    {
        hints.push(
            "hint: registry setup commands run in every workspace that contains this repo.\n  \
             Use --workspace to limit commands to the current workspace only.\n  \
             (suppress: wsp config set advice.registrySetupCommands false)",
        );
        touch_hint(&hints_dir, "registrySetupCommands");
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

/// Returns true if the hint has not been shown within the cooldown window.
fn hint_ready(hints_dir: &Path, key: &str, cooldown_days: u32) -> bool {
    if cooldown_days == 0 {
        return true;
    }
    let marker = hints_dir.join(format!("{}.last", key));
    if let Ok(meta) = std::fs::metadata(&marker)
        && let Ok(modified) = meta.modified()
    {
        let elapsed = modified.elapsed().unwrap_or(Duration::ZERO);
        if elapsed < Duration::from_secs(u64::from(cooldown_days) * 86_400) {
            return false;
        }
    }
    true
}

/// Records that a hint was shown by touching its marker file.
fn touch_hint(hints_dir: &Path, key: &str) {
    let _ = std::fs::create_dir_all(hints_dir);
    let _ = std::fs::write(hints_dir.join(format!("{}.last", key)), "");
}

// env mutation in tests is safe: tests that use XDG_DATA_HOME run single-threaded.
#[allow(unsafe_code)]
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use wsp_core::config::RepoEntry;

    struct TestPaths {
        _tmp: tempfile::TempDir,
        paths: Paths,
    }

    fn make_paths() -> TestPaths {
        let tmp = tempfile::tempdir().unwrap();
        // SAFETY: test-only single-threaded env mutation
        unsafe { std::env::set_var("XDG_DATA_HOME", tmp.path()) };
        let paths = Paths::resolve().unwrap();
        unsafe { std::env::remove_var("XDG_DATA_HOME") };
        TestPaths { _tmp: tmp, paths }
    }

    /// Backdate a hint marker file to a date guaranteed to be past any cooldown.
    /// Uses `touch -t 202401010000` (POSIX, works on macOS and Linux).
    fn backdate_hint_marker(hints_dir: &std::path::Path, key: &str) {
        let marker = hints_dir.join(format!("{}.last", key));
        std::fs::create_dir_all(hints_dir).unwrap();
        std::fs::write(&marker, "").unwrap();
        std::process::Command::new("touch")
            .args(["-t", "202401010000", marker.to_str().unwrap()])
            .status()
            .expect("touch -t should succeed");
    }

    fn cfg_with_prefix() -> Config {
        Config {
            branch_prefix: Some("jg".into()),
            ..Config::default()
        }
    }

    fn cfg_with_cooldown(days: u32) -> Config {
        Config {
            hints_cooldown_days: Some(days),
            ..Config::default()
        }
    }

    #[test]
    fn no_hints_when_disabled_globally() {
        let tp = make_paths();
        let paths = &tp.paths;
        let mut cfg = Config::default();
        cfg.hints = Some(false);
        assert!(evaluate("new", &cfg, &paths).is_empty());
    }

    #[test]
    fn branch_prefix_hint_fires_on_new_without_prefix() {
        let tp = make_paths();
        let paths = &tp.paths;
        let mut cfg = cfg_with_cooldown(0); // no cooldown so it always fires
        cfg.branch_prefix = None;
        let hints = evaluate("new", &cfg, &paths);
        assert!(
            hints.iter().any(|h| h.contains("branchPrefix")),
            "expected branchPrefix hint, got: {:?}",
            hints
        );
    }

    #[test]
    fn branch_prefix_hint_suppressed_when_prefix_set() {
        let tp = make_paths();
        let paths = &tp.paths;
        let cfg = cfg_with_prefix();
        let hints = evaluate("new", &cfg, &paths);
        assert!(
            !hints.iter().any(|h| h.contains("branchPrefix")),
            "branchPrefix hint should not fire when prefix is set"
        );
    }

    #[test]
    fn branch_prefix_hint_suppressed_via_advice() {
        let tp = make_paths();
        let paths = &tp.paths;
        let mut cfg = Config::default();
        cfg.advice = Some({
            let mut m = BTreeMap::new();
            m.insert("branchPrefix".into(), false);
            m
        });
        let hints = evaluate("new", &cfg, &paths);
        assert!(
            !hints.iter().any(|h| h.contains("branchPrefix")),
            "branchPrefix hint should be suppressed by advice key"
        );
    }

    #[test]
    fn setup_commands_hint_fires_on_new_and_repo_add() {
        let tp = make_paths();
        let paths = &tp.paths;
        let cfg = cfg_with_cooldown(0);
        for cmd in &["new", "repo/add"] {
            let hints = evaluate(cmd, &cfg, &paths);
            assert!(
                hints.iter().any(|h| h.contains("setupCommands")),
                "expected setupCommands hint for command {:?}, got: {:?}",
                cmd,
                hints
            );
        }
    }

    #[test]
    fn setup_commands_hint_suppressed_when_registry_has_commands() {
        let tp = make_paths();
        let paths = &tp.paths;
        let mut repos = BTreeMap::new();
        repos.insert(
            "github.com/user/repo".to_string(),
            RepoEntry {
                url: "git@test.local:user/repo.git".into(),
                added: chrono::Utc::now(),
                setup_commands: Some(vec!["make deps".into()]),
            },
        );
        let cfg = Config {
            hints_cooldown_days: Some(0),
            repos,
            ..Default::default()
        };
        for cmd in &["new", "repo/add"] {
            let hints = evaluate(cmd, &cfg, &paths);
            assert!(
                !hints.iter().any(|h| h.contains("setupCommands")),
                "setupCommands hint should not fire when registry already has setup commands ({cmd})"
            );
        }
    }

    #[test]
    fn setup_commands_hint_suppressed_via_advice() {
        let tp = make_paths();
        let paths = &tp.paths;
        let mut cfg = cfg_with_cooldown(0);
        cfg.advice = Some({
            let mut m = BTreeMap::new();
            m.insert("setupCommands".into(), false);
            m
        });
        let hints = evaluate("new", &cfg, &paths);
        assert!(
            !hints.iter().any(|h| h.contains("setupCommands")),
            "setupCommands hint should be suppressed by advice key"
        );
    }

    #[test]
    fn hint_cooldown_suppresses_repeat_within_window() {
        let tp = make_paths();
        let paths = &tp.paths;
        // 7-day cooldown (default)
        let cfg = Config::default();

        // First call: hint fires and records the marker
        let hints1 = evaluate("new", &cfg, &paths);
        assert!(
            hints1.iter().any(|h| h.contains("branchPrefix")),
            "hint should fire on first call"
        );

        // Second call (immediate): cooldown suppresses it
        let hints2 = evaluate("new", &cfg, &paths);
        assert!(
            !hints2.iter().any(|h| h.contains("branchPrefix")),
            "hint should be suppressed within cooldown window"
        );
    }

    #[test]
    fn hint_cooldown_zero_always_shows() {
        let tp = make_paths();
        let paths = &tp.paths;
        let cfg = cfg_with_cooldown(0);

        // Fire twice in a row; both times it should appear
        let h1 = evaluate("new", &cfg, &paths);
        let h2 = evaluate("new", &cfg, &paths);
        assert!(
            h1.iter().any(|h| h.contains("branchPrefix")),
            "first call should show hint"
        );
        assert!(
            h2.iter().any(|h| h.contains("branchPrefix")),
            "second call should still show hint when cooldown=0"
        );
    }

    #[test]
    fn hint_cooldown_fires_again_after_expiry() {
        let tp = make_paths();
        let paths = &tp.paths;
        let cfg = Config::default(); // 7-day cooldown

        // Pre-populate a stale marker (backdated to 2024-01-01, well past 7 days)
        let hints_dir = paths.data_dir().join("hints");
        backdate_hint_marker(&hints_dir, "branchPrefix");

        // Hint should fire again since the cooldown has expired
        let hints = evaluate("new", &cfg, paths);
        assert!(
            hints.iter().any(|h| h.contains("branchPrefix")),
            "hint should fire again after cooldown expiry"
        );
    }

    #[test]
    fn no_hints_for_unrelated_command() {
        let tp = make_paths();
        let paths = &tp.paths;
        let cfg = Config::default();
        let hints = evaluate("ls", &cfg, &paths);
        assert!(hints.is_empty(), "ls should produce no hints");
    }

    // -----------------------------------------------------------------------
    // registrySetupCommands hint
    // -----------------------------------------------------------------------

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
            hints_cooldown_days: Some(0),
            repos,
            ..Default::default()
        }
    }

    #[test]
    fn registry_setup_hint_fires_after_inferred_add_when_registry_has_commands() {
        let tp = make_paths();
        let paths = &tp.paths;
        let cfg = cfg_with_registry_setup("myrepo");
        let hints = evaluate("repo/setup-commands/add", &cfg, &paths);
        assert!(
            hints.iter().any(|h| h.contains("registrySetupCommands")),
            "expected registrySetupCommands hint for inferred add, got: {hints:?}"
        );
    }

    #[test]
    fn registry_setup_hint_fires_after_explicit_registry_add() {
        let tp = make_paths();
        let paths = &tp.paths;
        let cfg = cfg_with_registry_setup("myrepo");
        let hints = evaluate("repo/setup-commands/add/registry", &cfg, &paths);
        assert!(
            hints.iter().any(|h| h.contains("registrySetupCommands")),
            "expected registrySetupCommands hint for explicit --registry add, got: {hints:?}"
        );
    }

    #[test]
    fn registry_setup_hint_does_not_fire_for_workspace_or_repo_scope() {
        let tp = make_paths();
        let paths = &tp.paths;
        let cfg = cfg_with_registry_setup("myrepo");
        for cmd in &[
            "repo/setup-commands/add/workspace",
            "repo/setup-commands/add/repo",
        ] {
            let hints = evaluate(cmd, &cfg, &paths);
            assert!(
                !hints.iter().any(|h| h.contains("registrySetupCommands")),
                "registrySetupCommands hint should not fire for {cmd:?}, got: {hints:?}"
            );
        }
    }

    #[test]
    fn registry_setup_hint_does_not_fire_on_other_commands() {
        let tp = make_paths();
        let paths = &tp.paths;
        let cfg = cfg_with_registry_setup("myrepo");
        for cmd in &["repo/setup-commands/rm", "repo/setup-commands/ls", "new"] {
            let hints = evaluate(cmd, &cfg, &paths);
            assert!(
                !hints.iter().any(|h| h.contains("registrySetupCommands")),
                "registrySetupCommands hint should not fire for {cmd:?}, got: {hints:?}"
            );
        }
    }

    #[test]
    fn registry_setup_hint_does_not_fire_when_registry_empty() {
        let tp = make_paths();
        let paths = &tp.paths;
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
            hints_cooldown_days: Some(0),
            repos,
            ..Default::default()
        };
        let hints = evaluate("repo/setup-commands/add", &cfg, &paths);
        assert!(
            !hints.iter().any(|h| h.contains("registrySetupCommands")),
            "hint should not fire when registry has no setup commands"
        );
    }

    #[test]
    fn registry_setup_hint_suppressed_via_advice() {
        let tp = make_paths();
        let paths = &tp.paths;
        let mut cfg = cfg_with_registry_setup("myrepo");
        cfg.advice = Some({
            let mut m = BTreeMap::new();
            m.insert("registrySetupCommands".into(), false);
            m
        });
        let hints = evaluate("repo/setup-commands/add", &cfg, &paths);
        assert!(
            !hints.iter().any(|h| h.contains("registrySetupCommands")),
            "registrySetupCommands hint should be suppressed by advice key"
        );
    }
}
