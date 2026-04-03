use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Instant;

use anyhow::{Result, bail};
use clap::{Arg, ArgMatches, Command};
use clap_complete::engine::ArgValueCandidates;

use wsp_core::config::{self, Paths};
use wsp_core::discovery;
use wsp_core::git;
use wsp_core::giturl;
use wsp_core::mirror;
use wsp_core::output::{MutationOutput, Output};
use wsp_core::template;
use wsp_core::workspace;

use super::completers;

pub fn cmd() -> Command {
    Command::new("new")
        .about("Create a new workspace")
        .long_about(
            "Create a new workspace.\n\n\
             Sets up a directory with local clones of the specified repos, all sharing a \
             single feature branch. Clones are bootstrapped from local bare mirrors, so \
             creation is fast and works offline once mirrors exist.\n\n\
             When -b is given, the workspace checks out an existing remote branch instead \
             of creating a new one. The workspace name may be omitted; it is derived from \
             the last segment of the branch name. Repos that have the branch remotely track \
             it; repos that don't start fresh from the default branch. If the computed \
             branch name already exists remotely (no -b needed), wsp detects this and \
             tracks it automatically.\n\n\
             When run inside an existing workspace with no repos specified, automatically \
             copies the repo list from the current workspace. This makes it easy to spin up \
             parallel workspaces for related features.\n\n\
             Use --empty to create a workspace with no repos and add them later with \
             `wsp repo add`. --empty suppresses the implicit copy when run inside a workspace.",
        )
        .arg(Arg::new("workspace").required(false))
        .arg(
            Arg::new("branch")
                .short('b')
                .long("branch")
                .help("Check out an existing remote branch (name derived from last segment if workspace omitted)"),
        )
        .arg(
            Arg::new("repos")
                .num_args(0..)
                .add(ArgValueCandidates::new(completers::complete_repos)),
        )
        .arg(
            Arg::new("template")
                .short('t')
                .long("template")
                .help("Create from a template")
                .add(ArgValueCandidates::new(completers::complete_templates)),
        )
        .arg(
            Arg::new("from-workspace")
                .short('w')
                .long("workspace")
                .help("Clone repos from an existing workspace")
                .add(ArgValueCandidates::new(completers::complete_workspaces)),
        )
        .arg(
            Arg::new("file")
                .short('f')
                .long("file")
                .help("Create from a template file (.yaml)")
                .value_hint(clap::ValueHint::FilePath),
        )
        .arg(
            Arg::new("empty")
                .long("empty")
                .action(clap::ArgAction::SetTrue)
                .conflicts_with("repos")
                .help("Create workspace with no repos (add them later with `wsp repo add`)"),
        )
        .group(
            clap::ArgGroup::new("source")
                .args(["template", "from-workspace", "file", "empty"])
                .required(false),
        )
        .arg(
            Arg::new("no-fetch")
                .long("no-fetch")
                .action(clap::ArgAction::SetTrue)
                .help("Skip fetching mirrors before cloning"),
        )
        .arg(
            Arg::new("description")
                .short('d')
                .long("description")
                .help("Purpose of the workspace"),
        )
        .arg(
            Arg::new("no-discover")
                .long("no-discover")
                .action(clap::ArgAction::SetTrue)
                .help("Skip template discovery in cloned repos"),
        )
}

pub fn run(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let branch_override = matches.get_one::<String>("branch").map(|s| s.as_str());
    let ws_name_arg = matches.get_one::<String>("workspace");

    let repo_args: Vec<&String> = matches
        .get_many::<String>("repos")
        .map(|v| v.collect())
        .unwrap_or_default();

    // Derive workspace name: explicit arg takes precedence; fall back to last
    // segment of the branch name when -b is given.
    let derived_name: String;
    let ws_name: &str = if let Some(name) = ws_name_arg {
        name.as_str()
    } else if let Some(branch) = branch_override {
        let segment = branch.rsplit('/').next().unwrap_or(branch);
        if segment.is_empty() {
            bail!(
                "cannot derive workspace name from branch {:?}: the last segment is empty; \
                 specify a name explicitly with `wsp new <name> -b {}`",
                branch,
                branch
            );
        }
        derived_name = segment.to_string();
        &derived_name
    } else {
        bail!("workspace name is required (or use -b <branch> to derive it from the branch name)");
    };

    // Validate the branch name before any expensive I/O.
    if let Some(b) = branch_override {
        git::validate_branch_name(b)?;
    }
    let template_source = matches.get_one::<String>("template");
    let from_workspace = matches.get_one::<String>("from-workspace");
    let from_file = matches.get_one::<String>("file");
    let no_fetch = matches.get_flag("no-fetch");
    let empty = matches.get_flag("empty");
    let description = matches.get_one::<String>("description");

    let mut cfg = config::Config::load_from(&paths.config_path)
        .map_err(|e| anyhow::anyhow!("loading config: {}", e))?;

    let mut repo_refs: BTreeMap<String, String> = BTreeMap::new();
    let mut created_from: Option<String> = None;
    let mut loaded_template: Option<template::Template> = None;

    // Add repos from template name
    if let Some(source) = template_source {
        let tmpl = template::load(&paths.templates_dir, source)?;

        // Auto-register unknown repos from template
        template::auto_register(&tmpl, &mut cfg, paths)?;

        let identities = tmpl.identities()?;
        for id in identities {
            repo_refs.insert(id, String::new());
        }
        created_from = Some(format!("template:{}", source));
        loaded_template = Some(tmpl);
    }

    // Add repos from file (-f)
    if let Some(file_path) = from_file {
        let path = std::path::Path::new(file_path);
        let tmpl = template::load_from_file(path)?;

        tmpl.print_customizations();

        template::auto_register(&tmpl, &mut cfg, paths)?;

        let identities = tmpl.identities()?;
        for id in identities {
            repo_refs.insert(id, String::new());
        }
        created_from = Some(format!("file:{}", file_path));
        loaded_template = Some(tmpl);
    }

    // Add repos from existing workspace (-w)
    if let Some(source_ws) = from_workspace {
        let tmpl = template::from_workspace(paths, source_ws)?;

        template::auto_register(&tmpl, &mut cfg, paths)?;

        let identities = tmpl.identities()?;
        for id in identities {
            repo_refs.insert(id, String::new());
        }
        created_from = Some(format!("workspace:{}", source_ws));
        loaded_template = Some(tmpl);
    }

    // Add individual repos
    let identities: Vec<String> = cfg.repos.keys().cloned().collect();

    // When -b is given with an explicit workspace name, guard against the
    // common mistake of `wsp new -b branch repo` where clap's positional
    // parsing consumes `repo` as the workspace name (since workspace is
    // the first positional). Detect this by checking whether the
    // "workspace name" resolves as a known repo identity.
    if let (Some(branch), Some(name)) = (branch_override, ws_name_arg)
        && giturl::resolve(giturl::parse_repo_ref(name), &identities).is_ok()
    {
        bail!(
            "{:?} looks like a repo identity, not a workspace name \
             (clap parses the first positional as workspace);\n\
             use an explicit name: `wsp new <name> -b {} {}`",
            name.as_str(),
            branch,
            name.as_str()
        );
    }

    for rn in &repo_args {
        let name = giturl::parse_repo_ref(rn);
        let id = giturl::resolve(name, &identities)?;
        repo_refs.insert(id, String::new());
    }

    // Implicit -w: if no repos specified and we're inside a workspace, copy its repos.
    // Skipped when --empty is given (user explicitly wants zero repos).
    if repo_refs.is_empty()
        && repo_args.is_empty()
        && template_source.is_none()
        && from_workspace.is_none()
        && from_file.is_none()
        && !empty
    {
        let cwd = std::env::current_dir()?;
        if let Ok(ws_dir) = workspace::detect(&cwd) {
            let meta = workspace::load_metadata(&ws_dir)?;
            let source_name = &meta.name;
            let tmpl = template::from_workspace(paths, source_name)?;

            template::auto_register(&tmpl, &mut cfg, paths)?;

            let identities = tmpl.identities()?;
            let count = identities.len();
            for id in identities {
                repo_refs.insert(id, String::new());
            }
            eprintln!(
                "Copying {} repo{} from workspace {}",
                count,
                if count == 1 { "" } else { "s" },
                source_name,
            );
            created_from = Some(format!("workspace:{}", source_name));
            loaded_template = Some(tmpl);
        } else {
            bail!("no repos specified (use repo args, -t, -w, -f, or --empty)");
        }
    }

    // Validate early before expensive I/O
    workspace::validate_name(ws_name)?;
    let ws_dir = workspace::dir(&paths.workspaces_dir, ws_name);
    if ws_dir.exists() {
        if ws_name_arg.is_none() {
            // Name was derived from the branch; help the user pick a different one.
            bail!(
                "workspace {:?} already exists (name derived from branch {:?}); \
                 provide an explicit name: `wsp new <name> -b {}`",
                ws_name,
                branch_override.unwrap_or(""),
                branch_override.unwrap_or("")
            );
        }
        bail!("workspace {:?} already exists", ws_name);
    }

    // Build upstream URL map from config
    let mut upstream_urls: BTreeMap<String, String> = BTreeMap::new();
    for identity in repo_refs.keys() {
        if let Some(url) = cfg.upstream_url(identity) {
            upstream_urls.insert(identity.clone(), url.to_string());
        }
    }

    let start = Instant::now();

    // Build mirror list (needed for pre-fetch and branch validation).
    let mirrors: Vec<(String, std::path::PathBuf)> = repo_refs
        .keys()
        .filter_map(|id| {
            giturl::Parsed::from_identity(id)
                .ok()
                .map(|p| (id.clone(), mirror::dir(&paths.mirrors_dir, &p)))
        })
        .collect();

    // With -b, every repo must be in the mirror list for branch validation to
    // be complete. A malformed identity would silently pass validation and then
    // fail later during cloning — catch it now with a clear error.
    if branch_override.is_some() && mirrors.len() != repo_refs.len() {
        let unparseable: Vec<&str> = repo_refs
            .keys()
            .filter(|id| giturl::Parsed::from_identity(id).is_err())
            .map(|s| s.as_str())
            .collect();
        bail!(
            "cannot validate branch for {} repo{} with unparseable identit{}:\n{}",
            unparseable.len(),
            if unparseable.len() == 1 { "" } else { "s" },
            if unparseable.len() == 1 { "y" } else { "ies" },
            unparseable
                .iter()
                .map(|id| format!("  {}", id))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    // Pre-fetch mirrors (parallel) unless --no-fetch
    if !no_fetch && !mirrors.is_empty() {
        eprintln!("Fetching {} mirrors...", mirrors.len());
        let progress = Mutex::new(());
        std::thread::scope(|s| {
            let handles: Vec<_> = mirrors
                .iter()
                .map(|(id, mirror_dir)| {
                    let progress = &progress;
                    s.spawn(move || {
                        let result = git::fetch(mirror_dir, true);
                        let _lock = progress.lock().unwrap_or_else(|e| e.into_inner());
                        match &result {
                            Ok(()) => eprintln!("  ok    {}", id),
                            Err(e) => eprintln!("  FAIL  {} ({})", id, e),
                        }
                    })
                })
                .collect();
            for h in handles {
                let _ = h.join();
            }
        });
    }

    let branch_prefix = cfg.branch_prefix.as_deref();

    // Auto-detect: if -b was not given, compute the branch name that
    // workspace::create would use and check whether it already exists
    // remotely in any mirror. If so, treat it as an implicit -b.
    let auto_tracked_branch: Option<String> = if branch_override.is_none() && !mirrors.is_empty() {
        let computed = match branch_prefix.filter(|p| !p.is_empty()) {
            Some(prefix) => format!("{}/{}", prefix, ws_name),
            None => ws_name.to_string(),
        };
        let remote_ref = format!("refs/remotes/origin/{}", computed);
        if mirrors
            .iter()
            .any(|(_, mirror_dir)| git::ref_exists(mirror_dir, &remote_ref))
        {
            Some(computed)
        } else {
            None
        }
    } else {
        None
    };

    // Effective branch override: explicit -b takes precedence, then auto-detected.
    let effective_override: Option<&str> = branch_override.or(auto_tracked_branch.as_deref());
    let is_auto_detected = auto_tracked_branch.is_some();

    // Pre-compute per-repo tracking outcomes.
    // clone_from_mirror tracks where origin/<branch> exists; creates a fresh
    // local branch from origin/default elsewhere. This drives both the
    // zero-repos error and the post-create mixed summary.
    let outcomes: Option<(Vec<&str>, Vec<&str>)> = effective_override.map(|branch| {
        let remote_ref = format!("refs/remotes/origin/{}", branch);
        let mut tracked = Vec::new();
        let mut fresh = Vec::new();
        for (id, mirror_dir) in &mirrors {
            if git::ref_exists(mirror_dir, &remote_ref) {
                tracked.push(id.as_str());
            } else {
                fresh.push(id.as_str());
            }
        }
        (tracked, fresh)
    });

    // Error when the branch exists nowhere — likely a typo.
    if let Some((ref tracked, _)) = outcomes
        && tracked.is_empty()
    {
        let hint = if no_fetch {
            "hint: mirrors may be stale; retry without --no-fetch".to_string()
        } else {
            "hint: verify the branch name, or omit -b to start fresh in all repos".to_string()
        };
        bail!(
            "branch {:?} not found in any repo\n{}",
            effective_override.unwrap(),
            hint
        );
    }

    // For auto-detect: print a note before creating so the user knows tracking
    // is happening implicitly. Omit for explicit -b (user asked for it).
    if is_auto_detected {
        let branch = effective_override.unwrap();
        if let Some((ref tracked, ref fresh)) = outcomes {
            if fresh.is_empty() {
                // All repos track — simple one-liner.
                eprintln!(
                    "note: branch {:?} already exists remotely; tracking it",
                    branch
                );
            } else {
                // Mixed: list the tracked repos by name (the interesting case).
                eprintln!(
                    "note: branch {:?} exists remotely in {} of {} repos; tracking it in:",
                    branch,
                    tracked.len(),
                    mirrors.len()
                );
                for id in tracked {
                    eprintln!("  {}", id);
                }
            }
        }
    }

    eprintln!(
        "Creating workspace {:?} with {} repos...",
        ws_name,
        repo_refs.len()
    );
    workspace::create(
        paths,
        ws_name,
        &repo_refs,
        branch_prefix,
        effective_override,
        &upstream_urls,
        description.map(|s| s.as_str()),
        created_from.as_deref(),
    )?;

    let ws_dir = workspace::dir(&paths.workspaces_dir, ws_name);
    let meta_result = workspace::load_metadata(&ws_dir);

    // Apply template settings over global config for integrations
    let effective_cfg = match &loaded_template {
        Some(tmpl) => tmpl.apply_config(&cfg),
        None => cfg.clone(),
    };

    // Apply git config defaults to all clones
    if let Ok(ref meta) = meta_result {
        let git_config = effective_cfg.effective_git_config();
        workspace::apply_git_config(&ws_dir, meta, &git_config, None);
    }

    match &meta_result {
        Ok(meta) => wsp_core::lang::run_integrations(&ws_dir, meta, &effective_cfg),
        Err(e) => eprintln!("warning: skipping language integrations: {}", e),
    }
    // Seed AGENTS.md with template's agent_md content before auto-generation.
    // agentmd::update() will append the marked section, preserving this content.
    // Only seed if agent_md generation is enabled — otherwise we'd create a
    // half-baked AGENTS.md with no markers, no symlink, and no skills.
    if cfg.agent_md.unwrap_or(true)
        && let Some(ref tmpl) = loaded_template
        && let Some(ref content) = tmpl.agent_md
    {
        // Warn and show the full agent_md content so users can review prompt
        // instructions before they are written to AGENTS.md. A malicious template
        // could inject arbitrary instructions; visibility is the defense.
        eprintln!(
            "warning: this template includes agent instructions (agent_md). Review before proceeding:"
        );
        eprintln!("--- agent_md content ---");
        eprintln!("{}", content);
        eprintln!("--- end agent_md ---");

        let agents_path = ws_dir.join("AGENTS.md");
        if let Err(e) = std::fs::write(&agents_path, format!("{}\n\n", content)) {
            eprintln!("warning: could not write template agent content: {}", e);
        }
    }

    if cfg.agent_md.unwrap_or(true)
        && let Ok(meta) = &meta_result
        && let Err(e) = wsp_core::agentmd::update(&ws_dir, meta)
    {
        eprintln!("warning: AGENTS.md generation failed: {}", e);
    }

    // Template discovery: scan cloned repos for .wsp.yaml files
    let no_discover = matches.get_flag("no-discover");
    if !no_discover && let Ok(ref meta) = meta_result {
        let repo_infos = meta.repo_infos(&ws_dir);
        let mut all_discovered = Vec::new();
        for info in &repo_infos {
            if info.error.is_some() {
                continue;
            }
            let discovered =
                discovery::scan_repo_dir(&info.clone_dir, &info.identity, &paths.templates_dir);
            all_discovered.extend(discovered);
        }
        if let Err(e) = discovery::prompt_and_import(&all_discovered, &paths.templates_dir) {
            eprintln!("warning: template discovery failed: {}", e);
        }
    }

    // Run per-repo setup commands resolved from all layers:
    //   registry → template → repo .wsp.yaml → workspace
    if let Ok(ref meta) = meta_result {
        for info in meta.repo_infos(&ws_dir) {
            if info.error.is_some() {
                continue;
            }
            let resolved = wsp_core::setup_commands::resolve_for_repo(
                &cfg,
                loaded_template.as_ref(),
                Some(meta),
                &info.identity,
                Some(&info.clone_dir),
            )
            .dedup();
            if resolved.is_empty() {
                continue;
            }
            if let Err(e) = wsp_core::setup_runner::maybe_run_resolved(
                paths.data_dir(),
                &info.clone_dir,
                &info.identity,
                &resolved,
            ) {
                eprintln!("warning: setup commands for {}: {}", info.identity, e);
            }
        }
    }

    let duration_ms = start.elapsed().as_millis() as u64;

    // Use the authoritative branch from metadata (workspace::create computes it).
    let branch = meta_result
        .as_ref()
        .map(|m| m.branch.as_str())
        .unwrap_or(ws_name);

    Ok(Output::Mutation(
        MutationOutput::new(format!("Workspace created: {}", ws_dir.display()))
            .with_duration(duration_ms)
            .with_workspace(ws_name, ws_dir.display().to_string(), branch),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_conflicts_with_repos() {
        let err = cmd()
            .try_get_matches_from(["new", "myws", "--empty", "some/repo"])
            .unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn test_empty_conflicts_with_template() {
        let err = cmd()
            .try_get_matches_from(["new", "myws", "--empty", "-t", "mytmpl"])
            .unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn test_empty_conflicts_with_from_workspace() {
        let err = cmd()
            .try_get_matches_from(["new", "myws", "--empty", "-w", "otherws"])
            .unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn test_empty_conflicts_with_file() {
        let err = cmd()
            .try_get_matches_from(["new", "myws", "--empty", "-f", "foo.yaml"])
            .unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }
}
