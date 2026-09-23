use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use anyhow::{Result, bail};
use chrono::Utc;
use clap::{Arg, ArgMatches, Command};
use clap_complete::engine::ArgValueCandidates;

use wsp_core::config::{self, Paths, RepoEntry};
use wsp_core::output::{MutationOutput, Output, RepoAddResult};
use wsp_core::{discovery, filelock, gc, giturl, mirror, template, workspace, workspace_add};

use super::completers;
use crate::context::InvocationContext;

pub fn cmd() -> Command {
    Command::new("add")
        .about("Add repos to current workspace")
        .long_about("Add repos to current workspace.\n\nClones repositories onto the workspace branch. Full Git URLs work in isolated workspaces without registering globally. With global access, new URLs are registered automatically. Repeating an existing member preserves its clone and does not register it or replay setup.\n\nExisting directory mappings stay fixed. Setup and template imports are skipped when global state is unavailable. --no-fetch skips mirror refresh; direct clones may still contact their URL.")
        .arg(Arg::new("repos").num_args(0..).add(ArgValueCandidates::new(completers::complete_repos)))
        .arg(Arg::new("template").short('t').long("template").help("Add repos from a template").add(ArgValueCandidates::new(completers::complete_templates)))
        .arg(Arg::new("no-discover").long("no-discover").action(clap::ArgAction::SetTrue).help("Skip template discovery in added repos"))
        .arg(Arg::new("no-fetch").long("no-fetch").action(clap::ArgAction::SetTrue).help("Skip fetching mirrors before cloning"))
}

pub fn run(matches: &ArgMatches, context: &InvocationContext) -> Result<Output> {
    let ws = context.workspace_dir(None)?;
    gc::check_workspace(&ws, false)?;
    let local = context.is_workspace_local();
    let cfg = &context.config;
    let meta = workspace::load_metadata(&ws)?;
    let identities: Vec<String> = cfg
        .repos
        .keys()
        .chain(meta.repos.keys())
        .cloned()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut requests: BTreeMap<String, (String, String)> = BTreeMap::new();
    if let Some(source) = matches.get_one::<String>("template") {
        if local {
            bail!(
                "workspace-local repo add cannot import global templates; pass explicit repository URLs"
            );
        }
        let paths = context.require_host_paths()?;
        for repo in template::load(&paths.templates_dir, source)?.repos {
            let url = giturl::parse_repo_ref(&repo.url);
            requests.insert(
                giturl::parse(url)?.identity(),
                (
                    url.into(),
                    giturl::parse_repo_ref_branch(&repo.url)
                        .unwrap_or("")
                        .into(),
                ),
            );
        }
    }
    for input in matches.get_many::<String>("repos").into_iter().flatten() {
        let name = giturl::parse_repo_ref(input);
        let requested_branch = giturl::parse_repo_ref_branch(input)
            .unwrap_or("")
            .to_string();
        let (identity, url) = match giturl::resolve(name, &identities) {
            Ok(id) => {
                let url = cfg.upstream_url(&id).unwrap_or("").to_string();
                (id, url)
            }
            Err(_) => {
                let parsed = giturl::parse(name).map_err(|_| anyhow::anyhow!("repo {:?} cannot be resolved from available workspace/registry state; pass a full Git URL", name))?;
                (parsed.identity(), name.into())
            }
        };
        if let Some((_, prior)) = requests.get(&identity)
            && prior != &requested_branch
        {
            bail!("conflicting branch requests for {}", identity);
        }
        requests.insert(identity, (url, requested_branch));
    }
    if requests.is_empty() {
        bail!("no repos specified (use repo args or --template)");
    }

    // Validate existing memberships before any global effects, including mixed
    // batches containing both existing and genuinely new URLs.
    let mut pending = Vec::new();
    let mut results = Vec::new();
    for (identity, (url, branch)) in requests {
        if let Some(result) = workspace_add::member(&ws, &identity, &branch)? {
            results.push(result);
        } else {
            if url.is_empty() {
                bail!("no available URL for {}; pass a full Git URL", identity);
            }
            pending.push((identity, url, branch));
        }
    }
    // Refresh mirrors for already registered members as one concurrent batch.
    // A failed refresh is only a warning: a populated mirror remains a valid
    // offline source for the workspace clone. New URLs still register and fetch
    // their mirror before they are cloned below.
    if !local && !matches.get_flag("no-fetch") {
        prefetch_registered_mirrors(context.require_host_paths()?, &pending)?;
    }

    for (identity, url, branch) in pending {
        let mut result = if local {
            workspace_add::add(&ws, &identity, &url, &branch, workspace_add::Source::Direct)
        } else {
            let paths = context.require_host_paths()?;
            // Recheck before registration: another invocation may have completed
            // this member while earlier repositories in the batch were cloned.
            match workspace_add::existing(&ws, &identity, &branch) {
                Ok(Some(result)) => {
                    results.push(result);
                    continue;
                }
                Err(e) => {
                    let mut result = RepoAddResult::pending(&identity);
                    result.error = Some(format!("{e:#}"));
                    results.push(result);
                    continue;
                }
                Ok(None) => {
                    wsp_core::crash_barrier!(
                        wsp_core::crash_barrier::Operation::Add,
                        &identity,
                        wsp_core::crash_barrier::Point::AddAdmitted,
                        false,
                    )?;
                }
            }
            match ensure_registered(paths, &identity, &url) {
                Ok(()) => workspace_add::add(
                    &ws,
                    &identity,
                    &url,
                    &branch,
                    workspace_add::Source::Mirror(&paths.mirrors_dir),
                ),
                Err(e) => {
                    let mut result = RepoAddResult::pending(&identity);
                    result.error = Some(format!("{e:#}"));
                    result
                }
            }
        };
        if result.error.is_none() && result.clone == "created" {
            let latest = workspace::load_metadata(&ws)?;
            let effective = latest.apply_workspace_config(cfg);
            workspace::apply_git_config(
                &ws,
                &latest,
                &effective.effective_git_config(),
                Some(std::slice::from_ref(&identity)),
            );
            if !local {
                host_extras(
                    &ws,
                    context.require_host_paths()?,
                    cfg,
                    &latest,
                    &identity,
                    matches,
                    &mut result,
                );
            }
        }
        results.push(result);
    }

    // Guidance uses the latest membership and is serialized with all workspace
    // writers, including retries after membership committed but generation failed.
    let guidance = (|| -> Result<()> {
        let _lock = filelock::FileLock::acquire(
            &ws.join(workspace::METADATA_FILE),
            Duration::from_secs(30),
        )?;
        let latest = workspace::load_metadata(&ws)?;
        wsp_core::crash_barrier!(
            wsp_core::crash_barrier::Operation::Add,
            &format!("workspace/{}", latest.name),
            wsp_core::crash_barrier::Point::GuidanceSnapshot,
            true,
        )?;
        if cfg.agent_md.unwrap_or(true) {
            wsp_core::agentmd::update_after_agents(&ws, &latest, || {
                wsp_core::crash_barrier!(
                    wsp_core::crash_barrier::Operation::Add,
                    &format!("workspace/{}", latest.name),
                    wsp_core::crash_barrier::Point::GuidanceAgentsCommitted,
                    true,
                )
            })?;
        }
        wsp_core::crash_barrier!(
            wsp_core::crash_barrier::Operation::Add,
            &format!("workspace/{}", latest.name),
            wsp_core::crash_barrier::Point::GuidanceComplete,
            true,
        )?;
        wsp_core::lang::run_integrations(&ws, &latest, &latest.apply_workspace_config(cfg));
        Ok(())
    })();
    let latest = workspace::load_metadata(&ws)?;
    for result in &mut results {
        if result.membership == "updated" || result.membership == "unchanged" {
            match &guidance {
                Ok(()) => result.guidance = "updated".into(),
                Err(e) => {
                    result.guidance = "failed".into();
                    result.error = Some(format!("guidance: {e:#}"));
                }
            }
            if local || result.clone != "created" {
                let resolved = wsp_core::setup_commands::resolve_for_repo(
                    cfg,
                    None,
                    Some(&latest),
                    &result.identity,
                    Some(Path::new(&result.path)),
                )
                .dedup();
                result.setup = if resolved.is_empty() {
                    "not_configured"
                } else {
                    "skipped"
                }
                .into();
                result.setup_reason = if local {
                    "workspace_local_policy"
                } else {
                    "existing_clone_preserved"
                }
                .into();
            }
        }
    }
    let mut out = MutationOutput::new("Done.");
    out.ok = results.iter().all(|r| r.error.is_none());
    if !out.ok {
        out.message =
            "Repo add partially completed; inspect per-repository outcomes and retry.".into();
    }
    out.repos = results;
    out.context = local.then(|| context.output_context(&ws));
    Ok(Output::Mutation(out))
}

/// Refresh all registered pending members concurrently. Fetch errors are reported
/// by `prefetch_mirrors` on stderr and leave their existing mirrors available for
/// offline cloning.
fn prefetch_registered_mirrors(paths: &Paths, pending: &[(String, String, String)]) -> Result<()> {
    let cfg = config::Config::load_from(&paths.config_path)?;
    let mirrors: Vec<_> = pending
        .iter()
        .filter(|(identity, _, _)| cfg.repos.contains_key(identity))
        .map(|(identity, _, _)| {
            let parsed = giturl::Parsed::from_identity(identity)?;
            Ok((identity.clone(), mirror::dir(&paths.mirrors_dir, &parsed)))
        })
        .collect::<Result<_>>()?;
    super::fetch::prefetch_mirrors(&mirrors);
    Ok(())
}

fn ensure_registered(paths: &Paths, identity: &str, url: &str) -> Result<()> {
    let parsed = giturl::parse(url)?;
    let cfg = config::Config::load_from(&paths.config_path)?;
    if !cfg.repos.contains_key(identity) {
        eprintln!("Registering {}...", identity);
        mirror::clone(&paths.mirrors_dir, &parsed, url)?;
        mirror::fetch(&paths.mirrors_dir, &parsed)?;
        wsp_core::crash_barrier!(
            wsp_core::crash_barrier::Operation::Add,
            identity,
            wsp_core::crash_barrier::Point::MirrorPrepared,
            false,
        )?;
        filelock::with_config_after_save(
            &paths.config_path,
            |cfg| {
                cfg.repos
                    .entry(identity.into())
                    .or_insert_with(|| RepoEntry {
                        url: url.into(),
                        added: Utc::now(),
                        setup_commands: None,
                    });
                Ok(())
            },
            |_| {
                wsp_core::crash_barrier!(
                    wsp_core::crash_barrier::Operation::Add,
                    identity,
                    wsp_core::crash_barrier::Point::RegistryCommitted,
                    true,
                )
            },
        )?;
    }
    Ok(())
}

fn host_extras(
    ws: &Path,
    paths: &Paths,
    cfg: &config::Config,
    meta: &workspace::Metadata,
    identity: &str,
    matches: &ArgMatches,
    result: &mut RepoAddResult,
) {
    let clone = ws.join(meta.dir_name(identity).expect("validated member"));
    if !matches.get_flag("no-discover") {
        let found = discovery::scan_repo_dir(&clone, identity, &paths.templates_dir);
        match discovery::prompt_and_import(&found, &paths.templates_dir) {
            Ok(_) => result.template_import = "completed".into(),
            Err(e) => {
                result.template_import = "failed".into();
                result.error = Some(format!("template discovery: {e:#}"));
            }
        }
    }
    let resolved =
        wsp_core::setup_commands::resolve_for_repo(cfg, None, Some(meta), identity, Some(&clone))
            .dedup();
    if resolved.is_empty() {
        result.setup = "not_configured".into();
        result.setup_reason = "no_commands".into();
    } else {
        match wsp_core::setup_runner::maybe_run_resolved(
            paths.data_dir(),
            &clone,
            identity,
            &resolved,
        ) {
            Ok(ran) => {
                result.setup = if ran { "ran" } else { "skipped" }.into();
                result.setup_reason = "host_approval_policy".into();
            }
            Err(e) => {
                result.setup = "failed".into();
                result.setup_reason = e.to_string();
                result.error = Some(format!("setup: {e:#}"));
            }
        }
    }
}
