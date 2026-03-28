use std::path::Path;

use anyhow::{Result, bail};
use clap::{Arg, ArgMatches, Command};
use clap_complete::engine::ArgValueCandidates;

use wsp_core::config::{self, Paths};
use wsp_core::filelock;
use wsp_core::giturl;
use wsp_core::output::{MutationOutput, Output, SetupCommandEntry, SetupCommandsOutput};
use wsp_core::workspace;

use super::completers;

pub fn cmd() -> Command {
    Command::new("setup-commands")
        .about("Manage per-repo setup commands")
        .long_about(
            "View or manage setup commands for a repo.\n\n\
             `ls` shows the merged commands from all layers (registry, template, repo \
             .wsp.yaml, workspace) with provenance labels.\n\n\
             `add`, `rm`, and `clear` target a specific scope. Scope flags: --registry, \
             --workspace, --repo. Default: repo scope (.wsp.yaml) when inside a repo clone, \
             workspace scope when inside a workspace but not a clone, registry otherwise.",
        )
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(ls_cmd())
        .subcommand(add_cmd())
        .subcommand(rm_cmd())
        .subcommand(clear_cmd())
}

fn ls_cmd() -> Command {
    Command::new("ls")
        .visible_alias("list")
        .about("List merged setup commands with provenance [read-only]")
        .arg(
            Arg::new("repo")
                .required(false)
                .help("Repo identity, URL, or shortname (inferred from cwd if omitted)")
                .add(ArgValueCandidates::new(completers::complete_repos)),
        )
}

fn add_cmd() -> Command {
    Command::new("add")
        .about("Add a setup command for a repo")
        .arg(
            Arg::new("repo")
                .required(false)
                .help("Repo identity, URL, or shortname (inferred from cwd if omitted)")
                .add(ArgValueCandidates::new(completers::complete_repos)),
        )
        .arg(
            Arg::new("cmd")
                .required(true)
                .num_args(1..)
                .last(true)
                .allow_hyphen_values(true)
                .help(
                    "Shell command to add; use -- to pass multi-word commands: add -- npm install",
                ),
        )
        .arg(scope_registry_arg())
        .arg(scope_workspace_arg())
        .arg(scope_repo_arg())
}

fn rm_cmd() -> Command {
    Command::new("rm")
        .visible_alias("remove")
        .about("Remove a setup command for a repo")
        .arg(
            Arg::new("repo")
                .required(false)
                .help("Repo identity, URL, or shortname (inferred from cwd if omitted)")
                .add(ArgValueCandidates::new(completers::complete_repos)),
        )
        .arg(
            Arg::new("cmd")
                .required(true)
                .num_args(1..)
                .last(true)
                .allow_hyphen_values(true)
                .help("Shell command to remove; use -- to pass multi-word commands: rm -- npm install")
                .add(ArgValueCandidates::new(
                    completers::complete_repo_setup_commands,
                )),
        )
        .arg(scope_registry_arg())
        .arg(scope_workspace_arg())
        .arg(scope_repo_arg())
}

fn clear_cmd() -> Command {
    Command::new("clear")
        .about("Clear all setup commands for a repo at a scope")
        .arg(
            Arg::new("repo")
                .required(false)
                .help("Repo identity, URL, or shortname (inferred from cwd if omitted)")
                .add(ArgValueCandidates::new(completers::complete_repos)),
        )
        .arg(scope_registry_arg())
        .arg(scope_workspace_arg())
        .arg(scope_repo_arg())
}

fn scope_registry_arg() -> Arg {
    Arg::new("registry")
        .long("registry")
        .action(clap::ArgAction::SetTrue)
        .help("Target the global registry scope")
        .conflicts_with("workspace")
        .conflicts_with("repo-scope")
}

fn scope_workspace_arg() -> Arg {
    Arg::new("workspace")
        .long("workspace")
        .action(clap::ArgAction::SetTrue)
        .help("Target the current workspace scope")
        .conflicts_with("repo-scope")
}

fn scope_repo_arg() -> Arg {
    Arg::new("repo-scope")
        .long("repo")
        .action(clap::ArgAction::SetTrue)
        .help("Target the repo scope (writes to .wsp.yaml in the clone directory)")
}

pub fn run(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    match matches.subcommand() {
        Some(("ls", m)) => run_ls(m, paths),
        Some(("add", m)) => run_add(m, paths),
        Some(("rm", m)) => run_rm(m, paths),
        Some(("clear", m)) => run_clear(m, paths),
        _ => unreachable!(),
    }
}

// ---------------------------------------------------------------------------
// Repo resolution (explicit arg or CWD inference)
// ---------------------------------------------------------------------------

/// Resolve a repo identity from an explicit arg, or fall back to CWD detection.
/// Accepts a pre-loaded config to avoid redundant I/O when the caller already has one.
fn resolve_repo(repo_arg: Option<&String>, paths: &Paths) -> Result<String> {
    let cfg = config::Config::load_from(&paths.config_path)
        .map_err(|e| anyhow::anyhow!("loading config: {}", e))?;
    resolve_repo_with_cfg(repo_arg, &cfg)
}

fn resolve_repo_with_cfg(repo_arg: Option<&String>, cfg: &config::Config) -> Result<String> {
    let identities: Vec<String> = cfg.repos.keys().cloned().collect();

    if let Some(arg) = repo_arg {
        return giturl::resolve(giturl::parse_repo_ref(arg), &identities);
    }

    // No explicit repo — try to infer from CWD.
    let cwd = std::env::current_dir()?;
    let ws_dir = workspace::detect(&cwd)
        .map_err(|_| anyhow::anyhow!("not in a workspace; specify a repo name"))?;
    let meta = workspace::load_metadata(&ws_dir)?;
    workspace::repo_from_cwd(&ws_dir, &meta, &cwd).ok_or_else(|| {
        anyhow::anyhow!("could not detect repo from current directory; specify a repo name")
    })
}

// ---------------------------------------------------------------------------
// ls
// ---------------------------------------------------------------------------

fn run_ls(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let repo_arg = matches.get_one::<String>("repo");
    let cfg = config::Config::load_from(&paths.config_path)
        .map_err(|e| anyhow::anyhow!("loading config: {}", e))?;
    let identity = resolve_repo_with_cfg(repo_arg, &cfg)?;
    run_list(&cfg, &identity)
}

fn run_list(cfg: &config::Config, identity: &str) -> Result<Output> {
    let cwd = std::env::current_dir()?;
    let (meta, ws_dir) = match workspace::detect(&cwd) {
        Ok(ws_dir) => (Some(workspace::load_metadata(&ws_dir)?), Some(ws_dir)),
        Err(_) => (None, None),
    };

    let clone_dir = ws_dir.as_ref().and_then(|ws| {
        meta.as_ref()
            .and_then(|m| m.dir_name(identity).ok())
            .map(|d| ws.join(d))
    });

    let resolved = wsp_core::setup_commands::resolve_for_repo(
        cfg,
        None, // no template context in list mode
        meta.as_ref(),
        identity,
        clone_dir.as_deref(),
    );

    let commands = resolved
        .provenance
        .into_iter()
        .map(|(command, source)| SetupCommandEntry {
            command,
            source: source.to_string(),
        })
        .collect();

    Ok(Output::SetupCommands(SetupCommandsOutput {
        repo: identity.to_string(),
        commands,
    }))
}

// ---------------------------------------------------------------------------
// add
// ---------------------------------------------------------------------------

fn run_add(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let repo_arg = matches.get_one::<String>("repo");
    let cmd = matches
        .get_many::<String>("cmd")
        .unwrap()
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");
    let cwd = std::env::current_dir()?;
    let identity = resolve_repo(repo_arg, paths)?;

    match resolve_scope(matches, repo_arg.is_none(), &cwd)? {
        Scope::Registry => run_registry_add(paths, &identity, &cmd),
        Scope::Workspace => run_workspace_add(paths, &identity, &cmd),
        Scope::Repo => run_repo_add(&identity, &cmd, &cwd),
    }
}

// ---------------------------------------------------------------------------
// rm
// ---------------------------------------------------------------------------

fn run_rm(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let repo_arg = matches.get_one::<String>("repo");
    let cmd = matches
        .get_many::<String>("cmd")
        .unwrap()
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");
    let cwd = std::env::current_dir()?;
    let identity = resolve_repo(repo_arg, paths)?;

    match resolve_scope(matches, repo_arg.is_none(), &cwd)? {
        Scope::Registry => run_registry_rm(paths, &identity, &cmd),
        Scope::Workspace => run_workspace_rm(paths, &identity, &cmd),
        Scope::Repo => run_repo_rm(&identity, &cmd, &cwd),
    }
}

// ---------------------------------------------------------------------------
// clear
// ---------------------------------------------------------------------------

fn run_clear(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let repo_arg = matches.get_one::<String>("repo");
    let cwd = std::env::current_dir()?;
    let identity = resolve_repo(repo_arg, paths)?;

    match resolve_scope(matches, repo_arg.is_none(), &cwd)? {
        Scope::Registry => run_registry_clear(paths, &identity),
        Scope::Workspace => run_workspace_clear(paths, &identity),
        Scope::Repo => run_repo_clear(&identity, &cwd),
    }
}

// ---------------------------------------------------------------------------
// Scope resolution
// ---------------------------------------------------------------------------

enum Scope {
    Registry,
    Workspace,
    /// Write to the repo's own `.wsp.yaml` (committed to source control).
    Repo,
}

/// Resolve the target scope for a mutation.
///
/// Priority order:
/// 1. Explicit `--registry` / `--workspace` / `--repo` flags
/// 2. Smart default when `repo_inferred` is true (no explicit `<repo>` arg):
///    - CWD is inside a repo clone in a workspace → `Scope::Repo`
///    - CWD is inside a workspace (but not a specific clone) → `Scope::Workspace`
/// 3. CWD is inside a workspace (explicit repo arg) → `Scope::Workspace`
/// 4. Outside a workspace → `Scope::Registry`
fn resolve_scope(matches: &ArgMatches, repo_inferred: bool, cwd: &Path) -> Result<Scope> {
    let registry_flag = matches.get_flag("registry");
    let workspace_flag = matches.get_flag("workspace");
    let repo_flag = matches.get_flag("repo-scope");

    if registry_flag {
        return Ok(Scope::Registry);
    }
    if workspace_flag {
        return Ok(Scope::Workspace);
    }
    if repo_flag {
        return Ok(Scope::Repo);
    }

    match workspace::detect(cwd) {
        Ok(ws_dir) => {
            // When repo was inferred from CWD, check if we're inside a specific clone.
            // If so, default to repo scope so the command writes to the repo's .wsp.yaml.
            if repo_inferred
                && let Ok(meta) = workspace::load_metadata(&ws_dir)
                && workspace::repo_from_cwd(&ws_dir, &meta, cwd).is_some()
            {
                return Ok(Scope::Repo);
            }
            Ok(Scope::Workspace)
        }
        Err(_) => Ok(Scope::Registry),
    }
}

fn clone_dir_for(identity: &str, cwd: &Path) -> Result<std::path::PathBuf> {
    let ws_dir = workspace::detect(cwd)?;
    let meta = workspace::load_metadata(&ws_dir)?;
    let dir_name = meta
        .dir_name(identity)
        .map_err(|_| anyhow::anyhow!("repo {:?} not in workspace metadata", identity))?;
    Ok(ws_dir.join(dir_name))
}

// ---------------------------------------------------------------------------
// Registry mutations
// ---------------------------------------------------------------------------

fn validate_command(cmd: &str) -> Result<()> {
    if cmd.trim().is_empty() {
        bail!("setup command cannot be empty or whitespace-only");
    }
    Ok(())
}

fn run_registry_add(paths: &Paths, identity: &str, cmd: &str) -> Result<Output> {
    validate_command(cmd)?;
    filelock::with_config(&paths.config_path, |cfg| {
        let entry = cfg
            .repos
            .get_mut(identity)
            .ok_or_else(|| anyhow::anyhow!("repo {:?} not in registry", identity))?;
        let cmds = entry.setup_commands.get_or_insert_with(Vec::new);
        if cmds.contains(&cmd.to_string()) {
            bail!("command already exists: {:?}", cmd);
        }
        cmds.push(cmd.to_string());
        Ok(())
    })?;

    Ok(Output::Mutation(MutationOutput::new(format!(
        "registry: added setup command for {}",
        identity
    ))))
}

fn run_registry_rm(paths: &Paths, identity: &str, cmd: &str) -> Result<Output> {
    filelock::with_config(&paths.config_path, |cfg| {
        let entry = cfg
            .repos
            .get_mut(identity)
            .ok_or_else(|| anyhow::anyhow!("repo {:?} not in registry", identity))?;
        let cmds = entry.setup_commands.get_or_insert_with(Vec::new);
        let before = cmds.len();
        cmds.retain(|c| c != cmd);
        if cmds.len() == before {
            bail!("command not found: {:?}", cmd);
        }
        if cmds.is_empty() {
            entry.setup_commands = None;
        }
        Ok(())
    })?;

    Ok(Output::Mutation(MutationOutput::new(format!(
        "registry: removed setup command for {}",
        identity
    ))))
}

fn run_registry_clear(paths: &Paths, identity: &str) -> Result<Output> {
    filelock::with_config(&paths.config_path, |cfg| {
        let entry = cfg
            .repos
            .get_mut(identity)
            .ok_or_else(|| anyhow::anyhow!("repo {:?} not in registry", identity))?;
        entry.setup_commands = None;
        Ok(())
    })?;

    Ok(Output::Mutation(MutationOutput::new(format!(
        "registry: cleared setup commands for {}",
        identity
    ))))
}

// ---------------------------------------------------------------------------
// Workspace mutations
// ---------------------------------------------------------------------------

fn run_workspace_add(_paths: &Paths, identity: &str, cmd: &str) -> Result<Output> {
    validate_command(cmd)?;
    let cwd = std::env::current_dir()?;
    let ws_dir = workspace::detect(&cwd)?;

    filelock::with_metadata(&ws_dir, |meta| {
        if !meta.repos.contains_key(identity) {
            bail!("repo {:?} not in this workspace", identity);
        }
        let cmds = meta.setup_commands.entry(identity.to_string()).or_default();
        if cmds.contains(&cmd.to_string()) {
            bail!("command already exists: {:?}", cmd);
        }
        cmds.push(cmd.to_string());
        Ok(())
    })?;

    Ok(Output::Mutation(MutationOutput::new(format!(
        "workspace: added setup command for {}",
        identity
    ))))
}

fn run_workspace_rm(_paths: &Paths, identity: &str, cmd: &str) -> Result<Output> {
    let cwd = std::env::current_dir()?;
    let ws_dir = workspace::detect(&cwd)?;

    filelock::with_metadata(&ws_dir, |meta| {
        let cmds = meta
            .setup_commands
            .get_mut(identity)
            .ok_or_else(|| anyhow::anyhow!("no workspace setup commands for {:?}", identity))?;
        let before = cmds.len();
        cmds.retain(|c| c != cmd);
        if cmds.len() == before {
            bail!("command not found: {:?}", cmd);
        }
        if cmds.is_empty() {
            meta.setup_commands.remove(identity);
        }
        Ok(())
    })?;

    Ok(Output::Mutation(MutationOutput::new(format!(
        "workspace: removed setup command for {}",
        identity
    ))))
}

fn run_workspace_clear(_paths: &Paths, identity: &str) -> Result<Output> {
    let cwd = std::env::current_dir()?;
    let ws_dir = workspace::detect(&cwd)?;

    filelock::with_metadata(&ws_dir, |meta| {
        meta.setup_commands.remove(identity);
        Ok(())
    })?;

    Ok(Output::Mutation(MutationOutput::new(format!(
        "workspace: cleared setup commands for {}",
        identity
    ))))
}

// ---------------------------------------------------------------------------
// Repo (.wsp.yaml) mutations
// ---------------------------------------------------------------------------

fn run_repo_add(identity: &str, cmd: &str, cwd: &Path) -> Result<Output> {
    validate_command(cmd)?;
    let clone_dir = clone_dir_for(identity, cwd)?;
    let wsp_yaml = clone_dir.join(".wsp.yaml");

    filelock::with_repo_wsp_yaml(&wsp_yaml, |commands| {
        if commands.contains(&cmd.to_string()) {
            bail!("command already exists: {:?}", cmd);
        }
        commands.push(cmd.to_string());
        Ok(())
    })?;
    wsp_core::template::ensure_gitignore(&clone_dir)?;

    Ok(Output::Mutation(MutationOutput::new(format!(
        "repo: added setup command for {}",
        identity
    ))))
}

fn run_repo_rm(identity: &str, cmd: &str, cwd: &Path) -> Result<Output> {
    let clone_dir = clone_dir_for(identity, cwd)?;
    let wsp_yaml = clone_dir.join(".wsp.yaml");

    filelock::with_repo_wsp_yaml(&wsp_yaml, |commands| {
        if commands.is_empty() {
            bail!("no repo setup commands for {:?}", identity);
        }
        let before = commands.len();
        commands.retain(|c| c != cmd);
        if commands.len() == before {
            bail!("command not found: {:?}", cmd);
        }
        Ok(())
    })?;

    Ok(Output::Mutation(MutationOutput::new(format!(
        "repo: removed setup command for {}",
        identity
    ))))
}

fn run_repo_clear(identity: &str, cwd: &Path) -> Result<Output> {
    let clone_dir = clone_dir_for(identity, cwd)?;
    let wsp_yaml = clone_dir.join(".wsp.yaml");

    filelock::with_repo_wsp_yaml(&wsp_yaml, |commands| {
        commands.clear();
        Ok(())
    })?;

    Ok(Output::Mutation(MutationOutput::new(format!(
        "repo: cleared setup commands for {}",
        identity
    ))))
}
