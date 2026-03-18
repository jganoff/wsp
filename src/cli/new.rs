use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Instant;

use anyhow::{Result, bail};
use clap::{Arg, ArgMatches, Command};
use clap_complete::engine::ArgValueCandidates;

use crate::config::{self, Paths};
use crate::discovery;
use crate::git;
use crate::giturl;
use crate::mirror;
use crate::output::{MutationOutput, Output};
use crate::template;
use crate::util;
use crate::workspace;

use super::completers;

pub fn cmd() -> Command {
    Command::new("new")
        .about("Create a new workspace")
        .long_about(
            "Create a new workspace.\n\n\
             Sets up a directory with local clones of the specified repos, all sharing a \
             single feature branch. Clones are bootstrapped from local bare mirrors, so \
             creation is fast and works offline once mirrors exist.\n\n\
             When run inside an existing workspace with no repos specified, automatically \
             copies the repo list from the current workspace. This makes it easy to spin up \
             parallel workspaces for related features.",
        )
        .arg(Arg::new("workspace").required(true))
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
        .group(
            clap::ArgGroup::new("source")
                .args(["template", "from-workspace", "file"])
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
    let ws_name = matches.get_one::<String>("workspace").unwrap();
    let repo_args: Vec<&String> = matches
        .get_many::<String>("repos")
        .map(|v| v.collect())
        .unwrap_or_default();
    let template_source = matches.get_one::<String>("template");
    let from_workspace = matches.get_one::<String>("from-workspace");
    let from_file = matches.get_one::<String>("file");
    let no_fetch = matches.get_flag("no-fetch");
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
    for rn in &repo_args {
        let name = giturl::parse_repo_ref(rn);
        let id = giturl::resolve(name, &identities)?;
        repo_refs.insert(id, String::new());
    }

    // Implicit -w: if no repos specified and we're inside a workspace, copy its repos
    if repo_refs.is_empty()
        && repo_args.is_empty()
        && template_source.is_none()
        && from_workspace.is_none()
        && from_file.is_none()
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
            bail!("no repos specified (use repo args, -t, -w, or -f)");
        }
    }

    // Validate early before expensive I/O
    workspace::validate_name(ws_name)?;
    let ws_dir = workspace::dir(&paths.workspaces_dir, ws_name);
    if ws_dir.exists() {
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

    // Pre-fetch mirrors (parallel) unless --no-fetch
    if !no_fetch {
        let mirrors: Vec<(String, std::path::PathBuf)> = repo_refs
            .keys()
            .filter_map(|id| {
                giturl::Parsed::from_identity(id)
                    .ok()
                    .map(|p| (id.clone(), mirror::dir(&paths.mirrors_dir, &p)))
            })
            .collect();

        if !mirrors.is_empty() {
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
    }

    // Use the configured branch prefix if set. When not configured (None),
    // fall back to the currently signed-in GitHub user (`gh api user -q .login`).
    // An explicitly empty prefix (Some("")) means "no prefix" and is preserved.
    let gh_login_default;
    let branch_prefix: Option<&str> = match cfg.branch_prefix.as_deref() {
        Some(p) => Some(p),
        None => {
            gh_login_default = util::gh_login();
            gh_login_default.as_deref()
        }
    };
    let branch = match branch_prefix.filter(|p| !p.is_empty()) {
        Some(prefix) => format!("{}/{}", prefix, ws_name),
        None => ws_name.to_string(),
    };

    eprintln!(
        "Creating workspace {:?} (branch: {}) with {} repos...",
        ws_name,
        branch,
        repo_refs.len()
    );
    workspace::create(
        paths,
        ws_name,
        &repo_refs,
        branch_prefix,
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
        Ok(meta) => crate::lang::run_integrations(&ws_dir, meta, &effective_cfg),
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
        let agents_path = ws_dir.join("AGENTS.md");
        if let Err(e) = std::fs::write(&agents_path, format!("{}\n\n", content)) {
            eprintln!("warning: could not write template agent content: {}", e);
        }
    }

    if cfg.agent_md.unwrap_or(true)
        && let Ok(meta) = &meta_result
        && let Err(e) = crate::agentmd::update(&ws_dir, meta)
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

    let duration_ms = start.elapsed().as_millis() as u64;

    Ok(Output::Mutation(
        MutationOutput::new(format!("Workspace created: {}", ws_dir.display()))
            .with_duration(duration_ms)
            .with_workspace(ws_name, ws_dir.display().to_string(), &branch),
    ))
}
