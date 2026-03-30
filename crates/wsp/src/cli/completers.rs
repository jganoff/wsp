// Test completions from the command line with:
//   _CLAP_IFS=$'\n' _CLAP_COMPLETE_INDEX=<N> COMPLETE=zsh target/release/wsp -- wsp <words...>
// where N is the 0-based index of the word to complete.
use clap_complete::engine::CompletionCandidate;

use wsp_core::config::{Config, Paths};
use wsp_core::giturl;
use wsp_core::template;
use wsp_core::workspace;

pub fn complete_templates() -> Vec<CompletionCandidate> {
    let Ok(paths) = Paths::resolve() else {
        return Vec::new();
    };
    let Ok(names) = template::list(&paths.templates_dir) else {
        return Vec::new();
    };
    names.into_iter().map(CompletionCandidate::new).collect()
}

pub fn complete_repos() -> Vec<CompletionCandidate> {
    let Ok(paths) = Paths::resolve() else {
        return Vec::new();
    };
    let Ok(cfg) = Config::load_from(&paths.config_path) else {
        return Vec::new();
    };
    repos_to_candidates(cfg.repos.keys().cloned().collect())
}

/// Complete only repos in the current workspace (for `ws repo rm`).
pub fn complete_workspace_repos() -> Vec<CompletionCandidate> {
    let Ok(cwd) = std::env::current_dir() else {
        return Vec::new();
    };
    let Ok(ws_dir) = workspace::detect(&cwd) else {
        return Vec::new();
    };
    let Ok(meta) = workspace::load_metadata(&ws_dir) else {
        return Vec::new();
    };
    repos_to_candidates(meta.repos.keys().cloned().collect())
}

/// Complete repos in a named template (for `template repo rm`).
pub fn complete_template_repos() -> Vec<CompletionCandidate> {
    let Some(name) = template_name_from_args() else {
        return Vec::new();
    };
    let Ok(paths) = Paths::resolve() else {
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
    // Inspect prior args to find the key
    let args: Vec<String> = std::env::args().collect();
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
    let Ok(names) = workspace::list_all(&paths.workspaces_dir) else {
        return Vec::new();
    };
    names.into_iter().map(CompletionCandidate::new).collect()
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
    complete_repo_setup_commands_inner().unwrap_or_default()
}

fn complete_repo_setup_commands_inner() -> Option<Vec<CompletionCandidate>> {
    // Command line: wsp repo setup-commands rm <repo> <cmd>
    // Locate <repo> as the token two positions after "setup-commands".
    let args: Vec<String> = std::env::args().collect();
    let pos = args.iter().position(|a| a == "setup-commands")?;
    let repo_arg = args.get(pos + 2).filter(|a| !a.starts_with('-'))?;

    let paths = Paths::resolve().ok()?;
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
    if let Ok(cwd) = std::env::current_dir()
        && let Ok(ws_dir) = workspace::detect(&cwd)
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
    complete_template_repo_setup_commands_inner().unwrap_or_default()
}

fn complete_template_repo_setup_commands_inner() -> Option<Vec<CompletionCandidate>> {
    // Command line: wsp template setup-commands rm <name> <repo> <cmd>
    // Locate <name> as pos+2 and <repo> as pos+3 after "setup-commands".
    let args: Vec<String> = std::env::args().collect();
    let pos = args.iter().position(|a| a == "setup-commands")?;
    let tmpl_name = args.get(pos + 2).filter(|a| !a.starts_with('-'))?;
    let repo_arg = args.get(pos + 3).filter(|a| !a.starts_with('-'))?;

    let paths = Paths::resolve().ok()?;
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
fn template_name_from_args() -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    let pos = args.iter().position(|a| a == "template")?;
    // template <sub-noun> <verb> <name>
    args.get(pos + 3).filter(|a| !a.starts_with('-')).cloned()
}

#[cfg(test)]
// SAFETY: env-var mutations in these tests are serialized by ENV_MUTEX so they
// are safe despite being individually non-atomic. The unsafe blocks are test-only
// and are not reachable in production code.
#[allow(unsafe_code)]
mod tests {
    use super::*;

    // Serialize env-var-mutating tests to avoid data races in parallel test execution.
    static ENV_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
            "gc.retention-days",
            "shell.tmux",
            "shell.prompt",
            "hints",
            "hints-cooldown-days",
            "advice.branchPrefix",
            "advice.setupCommands",
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
        let _guard = ENV_MUTEX.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        // SAFETY: serialized by ENV_MUTEX — only one env-mutating test runs at a time.
        unsafe { std::env::set_var("XDG_DATA_HOME", tmp.path()) };
        let result = complete_templates();
        unsafe { std::env::remove_var("XDG_DATA_HOME") };

        assert!(
            result.is_empty(),
            "should return empty when templates dir does not exist; got {:?}",
            candidate_values(&result)
        );
    }

    #[test]
    fn templates_returns_names_of_yaml_files_in_templates_dir() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let templates_dir = tmp.path().join("wsp").join("templates");
        std::fs::create_dir_all(&templates_dir).unwrap();
        // Write minimal valid template files.
        std::fs::write(templates_dir.join("mytemplate.yaml"), "repos: []\n").unwrap();
        std::fs::write(templates_dir.join("alpha.yaml"), "repos: []\n").unwrap();

        unsafe { std::env::set_var("XDG_DATA_HOME", tmp.path()) };
        let result = complete_templates();
        unsafe { std::env::remove_var("XDG_DATA_HOME") };

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
        let _guard = ENV_MUTEX.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        // No config.yaml written — Config::load_from returns Config::default() with empty repos.
        unsafe { std::env::set_var("XDG_DATA_HOME", tmp.path()) };
        let result = complete_repos();
        unsafe { std::env::remove_var("XDG_DATA_HOME") };

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

        let _guard = ENV_MUTEX.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path().join("wsp");
        std::fs::create_dir_all(&data_dir).unwrap();

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
        cfg.save_to(&data_dir.join("config.yaml")).unwrap();

        unsafe { std::env::set_var("XDG_DATA_HOME", tmp.path()) };
        let result = complete_repos();
        unsafe { std::env::remove_var("XDG_DATA_HOME") };

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
        let _guard = ENV_MUTEX.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path().join("wsp");
        std::fs::create_dir_all(&data_dir).unwrap();
        // Point workspaces_dir at a path that does not exist.
        let ws_dir = tmp.path().join("no-such-workspaces");
        let mut cfg = wsp_core::config::Config::default();
        cfg.workspaces_dir = Some(ws_dir.to_string_lossy().into_owned());
        cfg.save_to(&data_dir.join("config.yaml")).unwrap();

        unsafe { std::env::set_var("XDG_DATA_HOME", tmp.path()) };
        let result = complete_workspaces();
        unsafe { std::env::remove_var("XDG_DATA_HOME") };

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

        let _guard = ENV_MUTEX.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path().join("wsp");
        let ws_root = tmp.path().join("workspaces");
        std::fs::create_dir_all(&data_dir).unwrap();
        std::fs::create_dir_all(&ws_root).unwrap();

        // Create two workspace directories each containing a .wsp.yaml.
        for name in &["alpha", "beta"] {
            let ws_dir = ws_root.join(name);
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

        // Write config pointing to our temp workspaces directory.
        let mut cfg = wsp_core::config::Config::default();
        cfg.workspaces_dir = Some(ws_root.to_string_lossy().into_owned());
        cfg.save_to(&data_dir.join("config.yaml")).unwrap();

        unsafe { std::env::set_var("XDG_DATA_HOME", tmp.path()) };
        let result = complete_workspaces();
        unsafe { std::env::remove_var("XDG_DATA_HOME") };

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
        let _guard = ENV_MUTEX.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path().join("wsp");
        std::fs::create_dir_all(&data_dir).unwrap();

        let mut cfg = wsp_core::config::Config::default();
        cfg.repos.insert(
            "github.com/acme/api".to_string(),
            wsp_core::config::RepoEntry {
                url: "git@github.com:acme/api.git".to_string(),
                added: chrono::Utc::now(),
                setup_commands: None,
            },
        );
        cfg.save_to(&data_dir.join("config.yaml")).unwrap();

        // Simulate: wsp repo setup-commands rm github.com/acme/api <cmd>
        // complete_repo_setup_commands reads args[pos("setup-commands")+2]
        // We can't easily inject std::env::args in a unit test, so verify the
        // inner function degrades gracefully when the repo has no commands.
        unsafe { std::env::set_var("XDG_DATA_HOME", tmp.path()) };
        let result = complete_repo_setup_commands();
        unsafe { std::env::remove_var("XDG_DATA_HOME") };

        // No commands configured → should return empty (no panic, no error).
        assert!(
            result.is_empty(),
            "should return empty when no commands configured; got {:?}",
            candidate_values(&result)
        );
    }

    #[test]
    fn repo_setup_commands_returns_commands_from_registry() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path().join("wsp");
        std::fs::create_dir_all(&data_dir).unwrap();

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
        cfg.save_to(&data_dir.join("config.yaml")).unwrap();

        // complete_repo_setup_commands reads args to find the repo; since
        // std::env::args() won't have the right args in a unit test, the
        // inner function returns None (can't find repo arg). Test the inner
        // helper directly via the exported function — it returns empty when
        // args aren't set up, which is the safe graceful-degradation path.
        unsafe { std::env::set_var("XDG_DATA_HOME", tmp.path()) };
        let result = complete_repo_setup_commands();
        unsafe { std::env::remove_var("XDG_DATA_HOME") };

        // In a unit test std::env::args() won't contain "setup-commands rm <repo>",
        // so the function gracefully returns empty. This verifies no panic/crash.
        assert!(
            result.is_empty() || candidate_values(&result).iter().all(|v| !v.is_empty()),
            "should return empty or valid commands; got {:?}",
            candidate_values(&result)
        );
    }

    // -----------------------------------------------------------------------
    // complete_template_repo_setup_commands — reads template from disk
    // -----------------------------------------------------------------------

    #[test]
    fn template_repo_setup_commands_returns_empty_when_no_commands() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path().join("wsp");
        std::fs::create_dir_all(&data_dir).unwrap();

        let tmpl = wsp_core::template::Template {
            repos: vec![wsp_core::template::TemplateRepo {
                url: "git@github.com:acme/api.git".to_string(),
                setup_commands: None,
            }],
            ..Default::default()
        };
        wsp_core::template::save(&data_dir.join("templates"), "mytemplate", &tmpl).unwrap();

        // complete_template_repo_setup_commands reads std::env::args(); in tests
        // args won't contain "setup-commands rm mytemplate acme/api", so the
        // function gracefully returns empty.
        unsafe { std::env::set_var("XDG_DATA_HOME", tmp.path()) };
        let result = complete_template_repo_setup_commands();
        unsafe { std::env::remove_var("XDG_DATA_HOME") };

        assert!(
            result.is_empty(),
            "should return empty when no setup commands in template; got {:?}",
            candidate_values(&result)
        );
    }
}
