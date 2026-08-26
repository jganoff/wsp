// Test completions from the command line with:
//   _CLAP_IFS=$'\n' _CLAP_COMPLETE_INDEX=<N> COMPLETE=zsh target/release/wsp -- wsp <words...>
// where N is the 0-based index of the word to complete.
// Completers read ambient state (paths, argv, cwd) only in the thin wrapper
// clap calls. The `_in` function below each wrapper takes those values as
// parameters, so tests drive them directly instead of mutating the process
// environment. Keep it that way: `std::env::set_var` is unsafe in edition 2024
// and both crate roots deny unsafe_code, so no unit test can mutate the
// environment. That guard stops at the crate root -- `crates/wsp/tests/*.rs`
// are separate crates without the deny -- and it bars mutation, not access:
// reading `$HOME` through `Paths::resolve()` needs no unsafe at all.
use std::path::Path;

use clap_complete::engine::CompletionCandidate;

use wsp_core::config::{Config, Paths};
use wsp_core::giturl;
use wsp_core::template;
use wsp_core::workspace;

pub fn complete_templates() -> Vec<CompletionCandidate> {
    let Ok(paths) = Paths::resolve() else {
        return Vec::new();
    };
    complete_templates_in(&paths)
}

fn complete_templates_in(paths: &Paths) -> Vec<CompletionCandidate> {
    let Ok(names) = template::list(&paths.templates_dir) else {
        return Vec::new();
    };
    names.into_iter().map(CompletionCandidate::new).collect()
}

pub fn complete_repos() -> Vec<CompletionCandidate> {
    let Ok(paths) = Paths::resolve() else {
        return Vec::new();
    };
    complete_repos_in(&paths)
}

fn complete_repos_in(paths: &Paths) -> Vec<CompletionCandidate> {
    let Ok(cfg) = Config::load_from(&paths.config_path) else {
        return Vec::new();
    };
    repos_to_candidates(cfg.repos.keys().cloned().collect())
}

/// Complete only repos in the current workspace (for `ws repo rm`).
pub fn complete_workspace_repos() -> Vec<CompletionCandidate> {
    let Ok(cwd) = crate::shellcd::invocation_dir() else {
        return Vec::new();
    };
    complete_workspace_repos_in(&cwd)
}

fn complete_workspace_repos_in(cwd: &Path) -> Vec<CompletionCandidate> {
    let Ok(ws_dir) = workspace::detect(cwd) else {
        return Vec::new();
    };
    let Ok(meta) = workspace::load_metadata(&ws_dir) else {
        return Vec::new();
    };
    repos_to_candidates(meta.repos.keys().cloned().collect())
}

/// Complete repos in a named template (for `template repo rm`).
pub fn complete_template_repos() -> Vec<CompletionCandidate> {
    let args: Vec<String> = std::env::args().collect();
    let Ok(paths) = Paths::resolve() else {
        return Vec::new();
    };
    complete_template_repos_in(&paths, &args)
}

fn complete_template_repos_in(paths: &Paths, args: &[String]) -> Vec<CompletionCandidate> {
    let Some(name) = template_name_from(args) else {
        return Vec::new();
    };
    let Ok(tmpl) = template::load(&paths.templates_dir, &name) else {
        return Vec::new();
    };
    let identities: Vec<String> = tmpl
        .repos
        .iter()
        .filter_map(|r| giturl::parse(&r.url).ok().map(|p| p.identity()))
        .collect();
    repos_to_candidates(identities)
}

/// Complete valid template config key prefixes.
pub fn complete_template_config_keys() -> Vec<CompletionCandidate> {
    vec![
        CompletionCandidate::new("sync-strategy"),
        CompletionCandidate::new("lang."),
        CompletionCandidate::new("git."),
    ]
}

/// Complete config keys for `wsp config get/set/unset`.
pub fn complete_config_keys() -> Vec<CompletionCandidate> {
    let mut keys: Vec<CompletionCandidate> = vec![
        CompletionCandidate::new("branch-prefix"),
        CompletionCandidate::new("workspaces-dir"),
        CompletionCandidate::new("sync-strategy"),
        CompletionCandidate::new("agent-md"),
        CompletionCandidate::new("gc.retention-days"),
        CompletionCandidate::new("shell.tmux"),
        CompletionCandidate::new("shell.prompt"),
    ];

    // lang.<name> keys
    for name in wsp_core::lang::integration_names() {
        keys.push(CompletionCandidate::new(format!("lang.{}", name)));
    }

    // git.* — show defaults as suggestions
    for key in wsp_core::config::Config::default_git_config().keys() {
        keys.push(CompletionCandidate::new(format!("git.{}", key)));
    }

    keys.push(CompletionCandidate::new("clone.protocol"));
    keys.push(CompletionCandidate::new("pr.source"));
    // hints / advice.*
    keys.push(CompletionCandidate::new("hints"));
    keys.push(CompletionCandidate::new("hints-cooldown-days"));
    for key in crate::hints::KNOWN_ADVICE_KEYS {
        keys.push(CompletionCandidate::new(format!("advice.{}", key)));
    }

    keys
}

/// Complete config values for `wsp config set` based on the key being set.
pub fn complete_config_values() -> Vec<CompletionCandidate> {
    let args: Vec<String> = std::env::args().collect();
    complete_config_values_in(&args)
}

fn complete_config_values_in(args: &[String]) -> Vec<CompletionCandidate> {
    // Pattern: wsp config set <key> <value>
    let key = args
        .iter()
        .position(|a| a == "set")
        .and_then(|i| args.get(i + 1))
        .map(|s| s.as_str());

    match key {
        Some("sync-strategy") => vec![
            CompletionCandidate::new("rebase"),
            CompletionCandidate::new("merge"),
        ],
        Some("agent-md" | "shell.prompt") => bool_candidates(),
        Some("clone.protocol") => wsp_core::config::CLONE_PROTOCOL_VALUES
            .iter()
            .map(|v| CompletionCandidate::new(*v))
            .collect(),
        Some("pr.source") => vec![
            CompletionCandidate::new("github"),
            CompletionCandidate::new("false"),
        ],
        Some("shell.tmux") => wsp_core::config::SHELL_TMUX_VALUES
            .iter()
            .map(|v| CompletionCandidate::new(*v))
            .collect(),
        Some(k) if k.starts_with("lang.") => bool_candidates(),
        _ => Vec::new(),
    }
}

fn bool_candidates() -> Vec<CompletionCandidate> {
    vec![
        CompletionCandidate::new("true"),
        CompletionCandidate::new("false"),
    ]
}

pub fn complete_workspaces() -> Vec<CompletionCandidate> {
    let Ok(paths) = Paths::resolve() else {
        return Vec::new();
    };
    complete_workspaces_in(&paths)
}

fn complete_workspaces_in(paths: &Paths) -> Vec<CompletionCandidate> {
    let Ok(names) = workspace::list_all(&paths.workspaces_dir) else {
        return Vec::new();
    };
    names.into_iter().map(CompletionCandidate::new).collect()
}

pub fn complete_recoverable_workspaces() -> Vec<CompletionCandidate> {
    let Ok(paths) = Paths::resolve() else {
        return Vec::new();
    };
    complete_recoverable_workspaces_in(&paths)
}

fn complete_recoverable_workspaces_in(paths: &Paths) -> Vec<CompletionCandidate> {
    let Ok(entries) = wsp_core::gc::list(&paths.gc_dir) else {
        return Vec::new();
    };
    entries
        .into_iter()
        .map(|e| CompletionCandidate::new(e.name))
        .collect()
}

fn repos_to_candidates(identities: Vec<String>) -> Vec<CompletionCandidate> {
    let shortnames = giturl::shortnames(&identities);
    shortnames
        .into_iter()
        .map(|(identity, short)| CompletionCandidate::new(short).help(Some(identity.into())))
        .collect()
}

/// Complete existing setup commands for a repo (for `repo setup-commands rm`).
/// Reads the repo arg already on the command line and returns setup commands
/// from both registry and workspace scopes so the user can pick one to remove.
pub fn complete_repo_setup_commands() -> Vec<CompletionCandidate> {
    let args: Vec<String> = std::env::args().collect();
    let Ok(paths) = Paths::resolve() else {
        return Vec::new();
    };
    let cwd = crate::shellcd::invocation_dir().ok();
    complete_repo_setup_commands_in(&paths, &args, cwd.as_deref()).unwrap_or_default()
}

fn complete_repo_setup_commands_in(
    paths: &Paths,
    args: &[String],
    cwd: Option<&Path>,
) -> Option<Vec<CompletionCandidate>> {
    // Command line: wsp repo setup-commands rm <repo> <cmd>
    // Locate <repo> as the token two positions after "setup-commands".
    let pos = args.iter().position(|a| a == "setup-commands")?;
    let repo_arg = args.get(pos + 2).filter(|a| !a.starts_with('-'))?;

    let cfg = Config::load_from(&paths.config_path).ok()?;
    let identities: Vec<String> = cfg.repos.keys().cloned().collect();
    let identity =
        wsp_core::giturl::resolve(wsp_core::giturl::parse_repo_ref(repo_arg), &identities).ok()?;

    let mut cmds: std::collections::HashSet<String> = std::collections::HashSet::new();
    if let Some(entry) = cfg.repos.get(&identity)
        && let Some(ref c) = entry.setup_commands
    {
        cmds.extend(c.iter().cloned());
    }
    if let Some(cwd) = cwd
        && let Ok(ws_dir) = workspace::detect(cwd)
        && let Ok(meta) = workspace::load_metadata(&ws_dir)
        && let Some(ws_cmds) = meta.setup_commands.get(&identity)
    {
        cmds.extend(ws_cmds.iter().cloned());
    }
    Some(cmds.into_iter().map(CompletionCandidate::new).collect())
}

/// Complete existing setup commands for a repo in a template
/// (for `template setup-commands rm`).
pub fn complete_template_repo_setup_commands() -> Vec<CompletionCandidate> {
    let args: Vec<String> = std::env::args().collect();
    let Ok(paths) = Paths::resolve() else {
        return Vec::new();
    };
    complete_template_repo_setup_commands_in(&paths, &args).unwrap_or_default()
}

fn complete_template_repo_setup_commands_in(
    paths: &Paths,
    args: &[String],
) -> Option<Vec<CompletionCandidate>> {
    // Command line: wsp template setup-commands rm <name> <repo> <cmd>
    // Locate <name> as pos+2 and <repo> as pos+3 after "setup-commands".
    let pos = args.iter().position(|a| a == "setup-commands")?;
    let tmpl_name = args.get(pos + 2).filter(|a| !a.starts_with('-'))?;
    let repo_arg = args.get(pos + 3).filter(|a| !a.starts_with('-'))?;

    let tmpl = template::load(&paths.templates_dir, tmpl_name).ok()?;
    let repo = tmpl.repos.iter().find(|r| {
        r.url == *repo_arg
            || wsp_core::giturl::parse(&r.url)
                .map(|p| p.identity())
                .unwrap_or_default()
                == *repo_arg
    })?;
    let cmds = repo.setup_commands.as_deref().unwrap_or(&[]);
    Some(cmds.iter().map(CompletionCandidate::new).collect())
}

/// Extract the template name from `["template", "repo"|"config"|"agent-md", "add"|"rm"|"set"|"get"|"unset", <name>]`.
fn template_name_from(args: &[String]) -> Option<String> {
    let pos = args.iter().position(|a| a == "template")?;
    // template <sub-noun> <verb> <name>
    args.get(pos + 3).filter(|a| !a.starts_with('-')).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    use wsp_core::testutil::make_test_paths;

    /// Build an argv the way the completer sees it on a real command line.
    fn argv(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| w.to_string()).collect()
    }

    /// Extract the raw string value from a CompletionCandidate.
    fn candidate_values(candidates: &[CompletionCandidate]) -> Vec<String> {
        candidates
            .iter()
            .map(|c| c.get_value().to_string_lossy().into_owned())
            .collect()
    }

    // -----------------------------------------------------------------------
    // complete_template_config_keys — pure, no I/O
    // -----------------------------------------------------------------------

    #[test]
    fn template_config_keys_returns_expected_prefixes() {
        let candidates = complete_template_config_keys();
        let values = candidate_values(&candidates);
        assert!(
            values.contains(&"sync-strategy".to_string()),
            "should contain 'sync-strategy'; got {:?}",
            values
        );
        assert!(
            values.contains(&"lang.".to_string()),
            "should contain 'lang.'; got {:?}",
            values
        );
        assert!(
            values.contains(&"git.".to_string()),
            "should contain 'git.'; got {:?}",
            values
        );
        assert_eq!(
            values.len(),
            3,
            "should have exactly 3 candidates; got {:?}",
            values
        );
    }

    // -----------------------------------------------------------------------
    // complete_config_keys — calls integration_names + default_git_config
    // -----------------------------------------------------------------------

    #[test]
    fn config_keys_contains_all_static_keys() {
        let candidates = complete_config_keys();
        let values = candidate_values(&candidates);

        let expected_static = [
            "branch-prefix",
            "workspaces-dir",
            "sync-strategy",
            "agent-md",
            "clone.protocol",
            "gc.retention-days",
            "shell.tmux",
            "shell.prompt",
            "pr.source",
            "hints",
            "hints-cooldown-days",
            "advice.branchPrefix",
            "advice.crossDevice",
            "advice.setupCommands",
            "advice.registrySetupCommands",
            "advice.whatsnew",
        ];
        for key in &expected_static {
            assert!(
                values.contains(&key.to_string()),
                "config keys should contain '{}'; got {:?}",
                key,
                values
            );
        }
    }

    #[test]
    fn config_keys_contains_lang_integration_names() {
        let candidates = complete_config_keys();
        let values = candidate_values(&candidates);

        // Verify at least one lang.* key exists.
        let lang_keys: Vec<&String> = values.iter().filter(|v| v.starts_with("lang.")).collect();
        assert!(
            !lang_keys.is_empty(),
            "config keys should contain at least one 'lang.*' key; got {:?}",
            values
        );

        // Verify every known integration name has a corresponding key.
        for name in wsp_core::lang::integration_names() {
            let expected = format!("lang.{}", name);
            assert!(
                values.contains(&expected),
                "config keys should contain '{}'; got {:?}",
                expected,
                values
            );
        }
    }

    #[test]
    fn config_keys_contains_git_config_defaults() {
        let candidates = complete_config_keys();
        let values = candidate_values(&candidates);

        for key in wsp_core::config::Config::default_git_config().keys() {
            let expected = format!("git.{}", key);
            assert!(
                values.contains(&expected),
                "config keys should contain '{}'; got {:?}",
                expected,
                values
            );
        }
    }

    // -----------------------------------------------------------------------
    // complete_config_values — reads std::env::args; in tests args lack "set"
    // -----------------------------------------------------------------------

    #[test]
    fn config_values_does_not_panic_when_no_set_in_args() {
        // In a test binary, process args are the test runner args, which do not
        // contain a bare "set" token followed by a key. The function should
        // return Vec::new() without panicking.
        let result = complete_config_values();
        // Accept any result — what matters is no panic.
        let _ = result;
    }

    #[test]
    fn shell_tmux_values_constant_contains_known_values() {
        // Verify the constant used in complete_config_values matches expectations.
        assert!(
            wsp_core::config::SHELL_TMUX_VALUES.contains(&"window-title"),
            "SHELL_TMUX_VALUES should contain 'window-title'"
        );
        assert!(
            wsp_core::config::SHELL_TMUX_VALUES.contains(&"false"),
            "SHELL_TMUX_VALUES should contain 'false'"
        );
    }

    // -----------------------------------------------------------------------
    // complete_template_repos — reads args for template name
    // -----------------------------------------------------------------------

    #[test]
    fn template_repos_returns_empty_when_no_template_in_args() {
        // Test runner args don't contain "template" as a positional arg, so
        // template_name_from_args() returns None and the function returns Vec::new().
        let result = complete_template_repos();
        assert!(
            result.is_empty(),
            "should return empty when 'template' is not in process args; got {:?}",
            candidate_values(&result)
        );
    }

    // -----------------------------------------------------------------------
    // complete_workspace_repos — reads cwd; test runner is not inside a workspace
    // -----------------------------------------------------------------------

    #[test]
    fn workspace_repos_does_not_panic_when_not_in_workspace() {
        // workspace::detect walks up from cwd looking for .wsp.yaml. The test
        // runner directory normally has no .wsp.yaml, so detect() returns an
        // error and the function falls back to Vec::new(). We just verify no panic.
        let result = complete_workspace_repos();
        let _ = result;
    }

    // -----------------------------------------------------------------------
    // complete_templates — reads Paths::resolve (XDG_DATA_HOME injectable)
    // -----------------------------------------------------------------------

    #[test]
    fn templates_returns_empty_when_no_templates_dir_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = make_test_paths(&tmp);
        let result = complete_templates_in(&paths);

        assert!(
            result.is_empty(),
            "should return empty when templates dir does not exist; got {:?}",
            candidate_values(&result)
        );
    }

    #[test]
    fn templates_returns_names_of_yaml_files_in_templates_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = make_test_paths(&tmp);
        std::fs::create_dir_all(&paths.templates_dir).unwrap();
        // Write minimal valid template files.
        std::fs::write(paths.templates_dir.join("mytemplate.yaml"), "repos: []\n").unwrap();
        std::fs::write(paths.templates_dir.join("alpha.yaml"), "repos: []\n").unwrap();

        let result = complete_templates_in(&paths);

        let values = candidate_values(&result);
        assert!(
            values.contains(&"mytemplate".to_string()),
            "should contain 'mytemplate'; got {:?}",
            values
        );
        assert!(
            values.contains(&"alpha".to_string()),
            "should contain 'alpha'; got {:?}",
            values
        );
        assert_eq!(
            values.len(),
            2,
            "should have exactly 2 templates; got {:?}",
            values
        );
    }

    // -----------------------------------------------------------------------
    // complete_repos — reads Paths::resolve + Config (XDG_DATA_HOME injectable)
    // -----------------------------------------------------------------------

    #[test]
    fn repos_returns_empty_when_config_has_no_repos() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = make_test_paths(&tmp);
        // No config.yaml written — Config::load_from returns Config::default() with empty repos.
        let result = complete_repos_in(&paths);

        assert!(
            result.is_empty(),
            "should return empty when config has no repos; got {:?}",
            candidate_values(&result)
        );
    }

    #[test]
    fn repos_returns_shortnames_for_repos_in_config() {
        use chrono::Utc;
        use wsp_core::config::{Config, RepoEntry};

        let tmp = tempfile::tempdir().unwrap();
        let paths = make_test_paths(&tmp);
        std::fs::create_dir_all(paths.data_dir()).unwrap();

        let now = Utc::now();
        let mut cfg = Config::default();
        cfg.repos.insert(
            "github.com/acme/api".into(),
            RepoEntry {
                url: "git@github.com:acme/api.git".into(),
                added: now,
                setup_commands: None,
            },
        );
        cfg.repos.insert(
            "github.com/acme/web".into(),
            RepoEntry {
                url: "git@github.com:acme/web.git".into(),
                added: now,
                setup_commands: None,
            },
        );
        cfg.save_to(&paths.config_path).unwrap();

        let result = complete_repos_in(&paths);

        let values = candidate_values(&result);
        assert_eq!(values.len(), 2, "should return 2 repos; got {:?}", values);
        // Shortnames: "api" and "web" (unique suffixes for the two identities).
        assert!(
            values.contains(&"api".to_string()),
            "should contain shortname 'api'; got {:?}",
            values
        );
        assert!(
            values.contains(&"web".to_string()),
            "should contain shortname 'web'; got {:?}",
            values
        );
    }

    // -----------------------------------------------------------------------
    // complete_workspaces — reads Paths::resolve + list_all (XDG_DATA_HOME injectable)
    // -----------------------------------------------------------------------

    #[test]
    fn workspaces_returns_empty_when_workspaces_dir_does_not_exist() {
        let tmp = tempfile::tempdir().unwrap();
        // Point workspaces_dir at a path that does not exist. make_test_paths
        // creates its own eagerly, so override that one field.
        let paths = Paths {
            workspaces_dir: tmp.path().join("no-such-workspaces"),
            ..make_test_paths(&tmp)
        };

        let result = complete_workspaces_in(&paths);

        assert!(
            result.is_empty(),
            "should return empty when workspaces dir does not exist; got {:?}",
            candidate_values(&result)
        );
    }

    #[test]
    fn workspaces_returns_names_of_dirs_with_metadata_file() {
        use std::collections::BTreeMap;
        use wsp_core::workspace::{Metadata, save_metadata};

        let tmp = tempfile::tempdir().unwrap();
        let paths = make_test_paths(&tmp);

        // Create two workspace directories each containing a .wsp.yaml.
        for name in &["alpha", "beta"] {
            let ws_dir = paths.workspaces_dir.join(name);
            std::fs::create_dir_all(&ws_dir).unwrap();
            let meta = Metadata {
                version: 0,
                name: name.to_string(),
                branch: format!("test/{}", name),
                repos: BTreeMap::new(),
                created: chrono::Utc::now(),
                description: None,
                last_used: None,
                created_from: None,
                dirs: BTreeMap::new(),
                config: None,
                setup_commands: std::collections::BTreeMap::new(),
            };
            save_metadata(&ws_dir, &meta).unwrap();
        }

        let result = complete_workspaces_in(&paths);

        let values = candidate_values(&result);
        assert_eq!(
            values.len(),
            2,
            "should return 2 workspace names; got {:?}",
            values
        );
        assert!(
            values.contains(&"alpha".to_string()),
            "should contain 'alpha'; got {:?}",
            values
        );
        assert!(
            values.contains(&"beta".to_string()),
            "should contain 'beta'; got {:?}",
            values
        );
    }

    // -----------------------------------------------------------------------
    // complete_repo_setup_commands — reads config + optional workspace state
    // -----------------------------------------------------------------------

    #[test]
    fn repo_setup_commands_returns_empty_when_no_commands_configured() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = make_test_paths(&tmp);
        std::fs::create_dir_all(paths.data_dir()).unwrap();

        let mut cfg = wsp_core::config::Config::default();
        cfg.repos.insert(
            "github.com/acme/api".to_string(),
            wsp_core::config::RepoEntry {
                url: "git@github.com:acme/api.git".to_string(),
                added: chrono::Utc::now(),
                setup_commands: None,
            },
        );
        cfg.save_to(&paths.config_path).unwrap();

        let args = argv(&["wsp", "repo", "setup-commands", "rm", "github.com/acme/api"]);
        let result = complete_repo_setup_commands_in(&paths, &args, None).unwrap_or_default();

        // No commands configured → should return empty (no panic, no error).
        assert!(
            result.is_empty(),
            "should return empty when no commands configured; got {:?}",
            candidate_values(&result)
        );
    }

    #[test]
    fn repo_setup_commands_returns_empty_when_args_do_not_match() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = make_test_paths(&tmp);

        // No "setup-commands" token on the command line at all.
        let args = argv(&["wsp", "st"]);
        let result = complete_repo_setup_commands_in(&paths, &args, None).unwrap_or_default();

        assert!(
            result.is_empty(),
            "should degrade to empty when the command line does not match; got {:?}",
            candidate_values(&result)
        );
    }

    #[test]
    fn repo_setup_commands_returns_commands_from_registry() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = make_test_paths(&tmp);
        std::fs::create_dir_all(paths.data_dir()).unwrap();

        let mut cfg = wsp_core::config::Config::default();
        cfg.repos.insert(
            "github.com/acme/api".to_string(),
            wsp_core::config::RepoEntry {
                url: "git@github.com:acme/api.git".to_string(),
                added: chrono::Utc::now(),
                setup_commands: Some(vec![
                    "make setup".to_string(),
                    "lefthook install".to_string(),
                ]),
            },
        );
        cfg.save_to(&paths.config_path).unwrap();

        let args = argv(&["wsp", "repo", "setup-commands", "rm", "github.com/acme/api"]);
        let result = complete_repo_setup_commands_in(&paths, &args, None).unwrap_or_default();

        let mut values = candidate_values(&result);
        values.sort();
        assert_eq!(
            values,
            vec!["lefthook install".to_string(), "make setup".to_string()],
            "should return the repo's configured setup commands"
        );
    }

    // -----------------------------------------------------------------------
    // complete_template_repo_setup_commands — reads template from disk
    // -----------------------------------------------------------------------

    #[test]
    fn template_repo_setup_commands_returns_empty_when_no_commands() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = make_test_paths(&tmp);

        let tmpl = wsp_core::template::Template {
            repos: vec![wsp_core::template::TemplateRepo {
                url: "git@github.com:acme/api.git".to_string(),
                setup_commands: None,
            }],
            ..Default::default()
        };
        wsp_core::template::save(&paths.templates_dir, "mytemplate", &tmpl).unwrap();

        let args = argv(&[
            "wsp",
            "template",
            "setup-commands",
            "rm",
            "mytemplate",
            "github.com/acme/api",
        ]);
        let result = complete_template_repo_setup_commands_in(&paths, &args).unwrap_or_default();

        assert!(
            result.is_empty(),
            "should return empty when no setup commands in template; got {:?}",
            candidate_values(&result)
        );
    }

    #[test]
    fn template_repo_setup_commands_returns_commands_from_template() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = make_test_paths(&tmp);

        let tmpl = wsp_core::template::Template {
            repos: vec![wsp_core::template::TemplateRepo {
                url: "git@github.com:acme/api.git".to_string(),
                setup_commands: Some(vec!["make setup".to_string()]),
            }],
            ..Default::default()
        };
        wsp_core::template::save(&paths.templates_dir, "mytemplate", &tmpl).unwrap();

        let args = argv(&[
            "wsp",
            "template",
            "setup-commands",
            "rm",
            "mytemplate",
            "github.com/acme/api",
        ]);
        let result = complete_template_repo_setup_commands_in(&paths, &args).unwrap_or_default();

        assert_eq!(
            candidate_values(&result),
            vec!["make setup".to_string()],
            "should return the template repo's setup commands"
        );
    }
}
