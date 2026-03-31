use std::collections::BTreeMap;

use anyhow::{Result, bail};
use chrono::Utc;
use clap::{Arg, ArgMatches, Command};
use clap_complete::engine::ArgValueCandidates;

use wsp_core::config::{self, Paths, RepoEntry};
use wsp_core::discovery;
use wsp_core::filelock;
use wsp_core::gc;
use wsp_core::git;
use wsp_core::giturl;
use wsp_core::mirror;
use wsp_core::output::{MutationOutput, Output};
use wsp_core::template;
use wsp_core::workspace;

use super::completers;

pub fn cmd() -> Command {
    Command::new("add")
        .about("Add repos to current workspace")
        .long_about(
            "Add repos to current workspace.\n\n\
             Clones the specified repos into the workspace directory, checking out the \
             workspace branch. Repos must be registered in the global registry first, or \
             specified as full git URLs to auto-register.\n\n\
             Repos that have the workspace branch remotely track it automatically; repos \
             that don't start fresh from the default branch. A note is printed when \
             outcomes differ across the added repos.",
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
                .help("Add repos from a template")
                .add(ArgValueCandidates::new(completers::complete_templates)),
        )
        .arg(
            Arg::new("no-discover")
                .long("no-discover")
                .action(clap::ArgAction::SetTrue)
                .help("Skip template discovery in added repos"),
        )
}

pub fn run(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let repo_args: Vec<&String> = matches
        .get_many::<String>("repos")
        .map(|v| v.collect())
        .unwrap_or_default();
    let template_source = matches.get_one::<String>("template");

    let cwd = std::env::current_dir()?;
    let ws_dir = workspace::detect(&cwd).map_err(|e| {
        // If the user passed a URL, they likely meant `wsp registry add`.
        let looks_like_url = repo_args.iter().any(|a| {
            a.starts_with("http")
                || a.starts_with("git@")
                || a.starts_with("ssh://")
                || a.contains("github.com")
                || a.ends_with(".git")
        });
        if looks_like_url {
            anyhow::anyhow!(
                "{}\n\nTo register a repo globally, use:\n  wsp registry add <url>",
                e
            )
        } else {
            e
        }
    })?;
    gc::check_workspace(&ws_dir, /* read_only */ false)?;

    let mut cfg = config::Config::load_from(&paths.config_path)
        .map_err(|e| anyhow::anyhow!("loading config: {}", e))?;

    let identities: Vec<String> = cfg.repos.keys().cloned().collect();

    let mut repo_refs: BTreeMap<String, String> = BTreeMap::new();

    // Add repos from template (-t)
    if let Some(source) = template_source {
        let tmpl = template::load(&paths.templates_dir, source)?;
        template::auto_register(&tmpl, &mut cfg, paths)?;
        let tmpl_identities = tmpl.identities()?;
        for id in tmpl_identities {
            repo_refs.insert(id, String::new());
        }
    }

    // Track URLs that need global registration (not yet in config.yaml)
    let mut to_register: Vec<(String, String)> = Vec::new(); // (identity, url)

    for rn in &repo_args {
        let name = giturl::parse_repo_ref(rn);
        let branch_override = giturl::parse_repo_ref_branch(rn).unwrap_or("").to_string();

        // Try resolving as a registered shortname first
        match giturl::resolve(name, &identities) {
            Ok(id) => {
                repo_refs.insert(id, branch_override);
            }
            Err(_) => {
                // Not a registered shortname — try parsing as a URL
                let parsed = giturl::parse(name).map_err(|_| {
                    anyhow::anyhow!("repo {:?} not found in config and is not a valid URL", name)
                })?;
                let identity = parsed.identity();
                to_register.push((identity.clone(), name.to_string()));
                repo_refs.insert(identity, branch_override);
            }
        }
    }

    if repo_refs.is_empty() {
        bail!("no repos specified (use repo args or --template)");
    }

    // Auto-register any unregistered repos (create mirror + add to config.yaml)
    for (identity, url) in &to_register {
        let parsed = giturl::parse(url)?;

        // Phase 1: check if already registered (race with concurrent add)
        let snapshot = filelock::read_config(&paths.config_path)?;
        if snapshot.repos.contains_key(identity) {
            continue; // another process registered it
        }

        // Phase 2: create mirror from upstream (slow, no lock)
        eprintln!("Registering {}...", identity);
        mirror::clone(&paths.mirrors_dir, &parsed, url)
            .map_err(|e| anyhow::anyhow!("cloning mirror for {}: {}", identity, e))?;
        mirror::fetch(&paths.mirrors_dir, &parsed)
            .map_err(|e| anyhow::anyhow!("fetching mirror for {}: {}", identity, e))?;

        // Phase 3: register under lock (fast, re-check)
        filelock::with_config(&paths.config_path, |cfg_mut| {
            if cfg_mut.repos.contains_key(identity) {
                // Another process registered it concurrently — desired state achieved.
                // Clean up the duplicate mirror we cloned in phase 2.
                let _ = mirror::remove(&paths.mirrors_dir, &parsed);
                return Ok(());
            }
            cfg_mut.repos.insert(
                identity.clone(),
                RepoEntry {
                    url: url.clone(),
                    added: Utc::now(),
                    setup_commands: None,
                },
            );
            Ok(())
        })?;
    }

    // Reload config to pick up newly registered repos
    let cfg = if to_register.is_empty() {
        cfg
    } else {
        config::Config::load_from(&paths.config_path)
            .map_err(|e| anyhow::anyhow!("reloading config: {}", e))?
    };

    // Build upstream URL map from config
    let mut upstream_urls: BTreeMap<String, String> = BTreeMap::new();
    for identity in repo_refs.keys() {
        if let Some(url) = cfg.upstream_url(identity) {
            upstream_urls.insert(identity.clone(), url.to_string());
        }
    }

    // Auto-detect per-repo tracking: check whether the workspace branch exists
    // remotely in each added repo's mirror. Repos with the remote branch track
    // it; repos without it get a fresh local branch from origin/default.
    // clone_from_mirror handles this gracefully when branch_tracks_remote=true.
    let ws_meta = workspace::load_metadata(&ws_dir)?;
    let ws_branch = ws_meta.branch.clone();
    let remote_ref = format!("refs/remotes/origin/{}", ws_branch);
    let mut fresh_repos: Vec<String> = Vec::new();
    for id in repo_refs.keys() {
        if let Ok(p) = giturl::Parsed::from_identity(id) {
            let mirror_dir = mirror::dir(&paths.mirrors_dir, &p);
            if !git::ref_exists(&mirror_dir, &remote_ref) {
                fresh_repos.push(id.clone());
            }
        }
    }

    eprintln!("Adding {} repos to workspace...", repo_refs.len());
    let new_ids: Vec<String> = repo_refs.keys().cloned().collect();
    workspace::add_repos(
        &paths.mirrors_dir,
        &ws_dir,
        &repo_refs,
        &upstream_urls,
        true, // always try to track; clone_from_mirror falls back to fresh when remote absent
    )?;

    // Print summary when some repos got a fresh branch instead of tracking.
    let tracked_count = repo_refs.len() - fresh_repos.len();
    if tracked_count > 0 && !fresh_repos.is_empty() {
        eprintln!(
            "note: branch {:?} not found remotely in {} repo{}; started from origin/default:",
            ws_branch,
            fresh_repos.len(),
            if fresh_repos.len() == 1 { "" } else { "s" }
        );
        for id in &fresh_repos {
            eprintln!("  {}", id);
        }
    }

    let meta_result = workspace::load_metadata(&ws_dir);

    // Apply git config defaults to newly added clones only
    if let Ok(ref meta) = meta_result {
        let git_config = cfg.effective_git_config();
        workspace::apply_git_config(&ws_dir, meta, &git_config, Some(&new_ids));
    }
    match &meta_result {
        Ok(meta) => wsp_core::lang::run_integrations(&ws_dir, meta, &cfg),
        Err(e) => eprintln!("warning: skipping language integrations: {}", e),
    }
    if cfg.agent_md.unwrap_or(true)
        && let Ok(meta) = &meta_result
        && let Err(e) = wsp_core::agentmd::update(&ws_dir, meta)
    {
        eprintln!("warning: AGENTS.md generation failed: {}", e);
    }

    // Template discovery: scan newly added repos for .wsp.yaml files
    if !matches.get_flag("no-discover") {
        let mut all_discovered = Vec::new();
        for id in &new_ids {
            if let Ok(ref meta) = meta_result {
                for info in meta.repo_infos(&ws_dir) {
                    if info.identity == *id && info.error.is_none() {
                        let discovered = discovery::scan_repo_dir(
                            &info.clone_dir,
                            &info.identity,
                            &paths.templates_dir,
                        );
                        all_discovered.extend(discovered);
                    }
                }
            }
        }
        if let Err(e) = discovery::prompt_and_import(&all_discovered, &paths.templates_dir) {
            eprintln!("warning: template discovery failed: {}", e);
        }
    }

    // Run per-repo setup commands resolved from all layers
    if let Ok(ref meta) = meta_result {
        for info in meta.repo_infos(&ws_dir) {
            if !new_ids.contains(&info.identity) || info.error.is_some() {
                continue;
            }
            let resolved = wsp_core::setup_commands::resolve_for_repo(
                &cfg,
                None, // no template context when adding repos
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

    Ok(Output::Mutation(MutationOutput::new("Done.")))
}
