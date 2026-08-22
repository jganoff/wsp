use std::fs;

use anyhow::Result;
use clap::{ArgMatches, Command};

use wsp_core::agentmd;
use wsp_core::config::{self, Paths};
use wsp_core::filelock;
use wsp_core::gc;
use wsp_core::git;
use wsp_core::giturl;
use wsp_core::lang;
use wsp_core::mirror;
use wsp_core::output::{CheckStatus, DoctorCheck, DoctorOutput, DoctorSummary, Output};
use wsp_core::symlink;
use wsp_core::template;
use wsp_core::workspace;

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

pub fn cmd() -> Command {
    Command::new("doctor")
        .about("Check workspace and global state for problems")
        .long_about(
            "Check workspace and global state for problems.\n\n\
             Validates config, mirrors, and workspace clones for invariant violations. \
             Run inside a workspace to also check that workspace's repos. Use --fix to \
             auto-repair fixable issues.",
        )
        .arg(
            clap::Arg::new("fix")
                .long("fix")
                .action(clap::ArgAction::SetTrue)
                .help("Auto-fix fixable problems"),
        )
}

pub fn run(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let fix = matches.get_flag("fix");
    let mut checks = Vec::new();
    let mut fixed = 0usize;

    // --- Global checks ---
    eprintln!("Checking global state...");

    // 1. Config parseable
    let cfg_result = config::Config::load_from(&paths.config_path);
    match &cfg_result {
        Ok(cfg) => {
            checks.push(DoctorCheck {
                scope: "global".into(),
                check: "config-parseable".into(),
                status: CheckStatus::Ok,
                message: format!("config is valid ({} registered repos)", cfg.repos.len()),
                fixable: false,
                details: None,
            });
            eprintln!("  ✓ config is valid ({} registered repos)", cfg.repos.len());
        }
        Err(e) => {
            checks.push(DoctorCheck {
                scope: "global".into(),
                check: "config-parseable".into(),
                status: CheckStatus::Error,
                message: format!("config failed to load: {}", e),
                fixable: false,
                details: None,
            });
            eprintln!("  ✗ config failed to load: {}", e);
            // Can't proceed without config
            return Ok(Output::Doctor(build_output(checks, fixed)));
        }
    };
    let cfg = cfg_result.unwrap();

    // G2. Config version skew
    check_config_version(&cfg, &mut checks);

    // G3. Branch prefix configured
    check_branch_prefix(&cfg, &mut checks);

    // 2. Mirrors exist for registered repos
    let mut missing_mirrors = Vec::new();
    for (identity, entry) in &cfg.repos {
        if let Ok(parsed) = giturl::parse(&entry.url)
            && !mirror::exists(&paths.mirrors_dir, &parsed)
        {
            missing_mirrors.push((identity.clone(), entry.url.clone()));
        }
    }
    if missing_mirrors.is_empty() {
        let mirror_count = cfg.repos.len();
        checks.push(DoctorCheck {
            scope: "global".into(),
            check: "mirrors-exist".into(),
            status: CheckStatus::Ok,
            message: format!("{} mirrors present", mirror_count),
            fixable: false,
            details: None,
        });
        eprintln!("  ✓ {} mirrors present", mirror_count);
    } else {
        for (identity, url) in &missing_mirrors {
            let fixable = true;
            if fix && let Ok(parsed) = giturl::parse(url) {
                match mirror::clone(&paths.mirrors_dir, &parsed, url) {
                    Ok(()) => {
                        checks.push(DoctorCheck {
                            scope: "global".into(),
                            check: "mirrors-exist".into(),
                            status: CheckStatus::Ok,
                            message: format!("{}: re-cloned mirror", identity),
                            fixable,
                            details: None,
                        });
                        eprintln!("  ✓ {}: re-cloned mirror", identity);
                        fixed += 1;
                        continue;
                    }
                    Err(e) => {
                        checks.push(DoctorCheck {
                            scope: "global".into(),
                            check: "mirrors-exist".into(),
                            status: CheckStatus::Error,
                            message: format!("{}: mirror missing, fix failed: {}", identity, e),
                            fixable,
                            details: None,
                        });
                        eprintln!("  ✗ {}: mirror missing, fix failed: {}", identity, e);
                        continue;
                    }
                }
            }
            checks.push(DoctorCheck {
                scope: "global".into(),
                check: "mirrors-exist".into(),
                status: CheckStatus::Warn,
                message: format!("{}: mirror missing", identity),
                fixable,
                details: None,
            });
            eprintln!("  ⚠ {}: mirror missing", identity);
        }
    }

    // G1. Orphaned mirrors — mirrors dir entries with no config entry
    check_orphaned_mirrors(paths, &cfg, fix, &mut checks, &mut fixed);

    // G4. GC stale entries — entries past retention that should have been purged
    check_gc_stale_entries(paths, &cfg, fix, &mut checks, &mut fixed);

    // G5. GC orphaned entries — dirs in gc/ without valid metadata
    check_gc_orphaned_entries(paths, &mut checks);

    // G6. GC disk usage — informational
    check_gc_disk_usage(paths, &mut checks);

    // G3. Workspaces dir exists
    check_workspaces_dir_exists(paths, fix, &mut checks, &mut fixed);

    // G7. Template repos parseable
    check_template_repos_parseable(paths, &mut checks);

    // G8. Template repos registered (have mirrors)
    check_template_repos_registered(paths, &cfg, fix, &mut checks, &mut fixed);

    // G10. Global wspignore defaults
    check_wspignore_defaults(paths, fix, &mut checks, &mut fixed);

    // G11. Deprecated config keys — old-format keys that should be migrated
    check_deprecated_config_keys(paths, &cfg, fix, &mut checks, &mut fixed);

    // --- Workspace checks (if inside one) ---
    let cwd = std::env::current_dir()?;
    if let Ok(ws_dir) = workspace::detect(&cwd) {
        let meta = workspace::load_metadata(&ws_dir)?;
        let ws_scope = format!("workspace/{}", meta.name);
        eprintln!("\nChecking workspace {:?}...", meta.name);

        // W1. Metadata version skew
        check_metadata_version(&meta, &ws_scope, &mut checks);

        // W3. Legacy ref field — stale @ref values in metadata
        check_legacy_ref_field(&ws_dir, &meta, &ws_scope, fix, &mut checks, &mut fixed);

        // W4. Stale dirs map — orphaned entries in dirs collision map
        check_stale_dirs_map(&ws_dir, &meta, &ws_scope, fix, &mut checks, &mut fixed);

        // W12. Unregistered repos — workspace repos not in global registry
        check_unregistered_repos(
            &ws_dir,
            &meta,
            &cfg,
            paths,
            &ws_scope,
            fix,
            &mut checks,
            &mut fixed,
        );

        // W9. AGENTS.md / CLAUDE.md validity
        check_agents_md_valid(&ws_dir, &meta, &ws_scope, fix, &mut checks, &mut fixed);

        // W5. Missing dirs map — collision disambiguation needed but missing
        check_missing_dirs_map(&ws_dir, &meta, &ws_scope, fix, &mut checks, &mut fixed);

        // W11. go.work validity
        check_go_work_valid(&ws_dir, &meta, &ws_scope, fix, &mut checks, &mut fixed);

        // W14. Git config drift — clone's local config differs from effective config
        let effective_cfg = meta.apply_workspace_config(&cfg);
        let effective_gc = effective_cfg.effective_git_config();
        check_git_config_drift(
            &ws_dir,
            &meta,
            &effective_gc,
            &ws_scope,
            fix,
            &mut checks,
            &mut fixed,
        );

        // Per-repo checks
        let repo_infos = meta.repo_infos(&ws_dir);
        for info in &repo_infos {
            let scope = format!("workspace/{}/{}", meta.name, info.dir_name);

            // Repo dir exists
            if info.error.is_some() || !info.clone_dir.exists() {
                checks.push(DoctorCheck {
                    scope: scope.clone(),
                    check: "repo-dir-exists".into(),
                    status: CheckStatus::Error,
                    message: format!("{}: directory missing", info.dir_name),
                    fixable: false,
                    details: None,
                });
                eprintln!("  ✗ {}: directory missing", info.dir_name);
                continue;
            }

            // Origin remote exists
            if !git::has_remote(&info.clone_dir, "origin") {
                checks.push(DoctorCheck {
                    scope: scope.clone(),
                    check: "origin-remote-exists".into(),
                    status: CheckStatus::Error,
                    message: format!("{}: no origin remote", info.dir_name),
                    fixable: false,
                    details: None,
                });
                eprintln!("  ✗ {}: no origin remote", info.dir_name);
                continue;
            }

            // W2. Legacy wsp-mirror remote
            check_legacy_wsp_mirror(
                &info.clone_dir,
                &info.dir_name,
                &scope,
                fix,
                &mut checks,
                &mut fixed,
            );

            // W7. In-progress git operation
            check_in_progress_op(&info.clone_dir, &info.dir_name, &scope, &mut checks);

            // Origin URL matches registered URL
            let clone_url = git::remote_get_url(&info.clone_dir, "origin")
                .unwrap_or_default()
                .trim()
                .to_string();
            let registered_url = cfg.upstream_url(&info.identity).unwrap_or("");

            if !urls_equivalent(&clone_url, registered_url) {
                let fixable = true;
                if fix && !registered_url.is_empty() {
                    match git::remote_set_url(&info.clone_dir, "origin", registered_url) {
                        Ok(()) => {
                            checks.push(DoctorCheck {
                                scope: scope.clone(),
                                check: "origin-url-match".into(),
                                status: CheckStatus::Ok,
                                message: format!(
                                    "{}: repointed origin to {}",
                                    info.dir_name, registered_url
                                ),
                                fixable,
                                details: None,
                            });
                            eprintln!(
                                "  ✓ {}: repointed origin to {}",
                                info.dir_name, registered_url
                            );
                            fixed += 1;
                            continue;
                        }
                        Err(e) => {
                            checks.push(DoctorCheck {
                                scope: scope.clone(),
                                check: "origin-url-match".into(),
                                status: CheckStatus::Warn,
                                message: format!(
                                    "{}: origin URL mismatch, fix failed: {}",
                                    info.dir_name, e
                                ),
                                fixable,
                                details: Some(serde_json::json!({
                                    "clone_url": clone_url,
                                    "registered_url": registered_url,
                                })),
                            });
                            eprintln!(
                                "  ⚠ {}: origin URL mismatch, fix failed: {}",
                                info.dir_name, e
                            );
                            continue;
                        }
                    }
                }
                checks.push(DoctorCheck {
                    scope: scope.clone(),
                    check: "origin-url-match".into(),
                    status: CheckStatus::Warn,
                    message: format!("{}: origin URL differs from registered URL", info.dir_name),
                    fixable,
                    details: Some(serde_json::json!({
                        "clone_url": clone_url,
                        "registered_url": registered_url,
                    })),
                });
                eprintln!(
                    "  ⚠ {}: origin URL differs from registered URL",
                    info.dir_name
                );
                eprintln!("      clone:      {}", clone_url);
                eprintln!("      registered: {}", registered_url);
                continue;
            }

            // Identity matches (origin URL resolves to same identity as .wsp.yaml)
            if let Ok(parsed) = giturl::parse(&clone_url) {
                let clone_identity = parsed.identity();
                if clone_identity != info.identity {
                    checks.push(DoctorCheck {
                        scope: scope.clone(),
                        check: "identity-match".into(),
                        status: CheckStatus::Warn,
                        message: format!(
                            "{}: origin URL resolves to {} but .wsp.yaml says {} — \
                             remove and re-add the repo: `wsp repo rm {}` then `wsp repo add {}`",
                            info.dir_name,
                            clone_identity,
                            info.identity,
                            info.dir_name,
                            clone_identity
                        ),
                        fixable: false,
                        details: Some(serde_json::json!({
                            "clone_identity": clone_identity,
                            "metadata_identity": info.identity,
                        })),
                    });
                    eprintln!(
                        "  ⚠ {}: identity mismatch (origin={}, metadata={})",
                        info.dir_name, clone_identity, info.identity
                    );
                    eprintln!(
                        "      fix: `wsp repo rm {}` then `wsp repo add {}`",
                        info.dir_name, clone_identity
                    );
                    continue;
                }
            }

            // W13. Mirror refspec
            check_mirror_refspec(
                &info.clone_dir,
                &info.dir_name,
                &scope,
                fix,
                &mut checks,
                &mut fixed,
            );

            // W15. Unapproved setup commands (resolved from all layers)
            check_unapproved_setup_commands(
                &info.clone_dir,
                &info.identity,
                &info.dir_name,
                &scope,
                paths,
                &cfg,
                &meta,
                fix,
                &mut checks,
                &mut fixed,
            );

            // All checks passed for this repo
            checks.push(DoctorCheck {
                scope,
                check: "repo-ok".into(),
                status: CheckStatus::Ok,
                message: format!("{}: ok", info.dir_name),
                fixable: false,
                details: None,
            });
            eprintln!("  ✓ {}: ok", info.dir_name);
        }
    }

    // --- Summary ---
    let output = build_output(checks, fixed);
    let summary = &output.summary;

    eprintln!();
    if summary.warn == 0 && summary.error == 0 {
        eprintln!("All checks passed.");
    } else {
        let mut parts = Vec::new();
        if summary.warn > 0 {
            parts.push(format!(
                "{} warning{}",
                summary.warn,
                if summary.warn == 1 { "" } else { "s" }
            ));
        }
        if summary.error > 0 {
            parts.push(format!(
                "{} error{}",
                summary.error,
                if summary.error == 1 { "" } else { "s" }
            ));
        }
        if fixed > 0 {
            parts.push(format!(
                "{} fix{} applied",
                fixed,
                if fixed == 1 { "" } else { "es" }
            ));
        }
        let msg = parts.join(", ");
        eprintln!("{}.", msg);
        let any_fixable = output
            .checks
            .iter()
            .any(|c| c.status == CheckStatus::Warn && c.fixable);
        if any_fixable && !fix {
            eprintln!("Run `wsp doctor --fix` to auto-fix.");
        }
    }

    Ok(Output::Doctor(output))
}

// ---------------------------------------------------------------------------
// Global check helpers
// ---------------------------------------------------------------------------

/// G2. Config version skew.
fn check_config_version(cfg: &config::Config, checks: &mut Vec<DoctorCheck>) {
    if cfg.version > config::CURRENT_CONFIG_VERSION {
        checks.push(DoctorCheck {
            scope: "global".into(),
            check: "config-version".into(),
            status: CheckStatus::Warn,
            message: format!(
                "config version {} is newer than supported version {}",
                cfg.version,
                config::CURRENT_CONFIG_VERSION
            ),
            fixable: false,
            details: Some(serde_json::json!({
                "config_version": cfg.version,
                "supported_version": config::CURRENT_CONFIG_VERSION,
            })),
        });
        eprintln!(
            "  ⚠ config version {} is newer than supported version {}",
            cfg.version,
            config::CURRENT_CONFIG_VERSION
        );
    } else {
        checks.push(DoctorCheck {
            scope: "global".into(),
            check: "config-version".into(),
            status: CheckStatus::Ok,
            message: format!("config version {}", cfg.version),
            fixable: false,
            details: None,
        });
        eprintln!("  ✓ config version {}", cfg.version);
    }
}

/// G3. Branch prefix not configured.
fn check_branch_prefix(cfg: &config::Config, checks: &mut Vec<DoctorCheck>) {
    if cfg.branch_prefix.is_none() {
        checks.push(DoctorCheck {
            scope: "global".into(),
            check: "branch-prefix".into(),
            status: CheckStatus::Warn,
            message: "branch-prefix not set — workspace branches will use the workspace name as-is"
                .into(),
            // Not auto-fixable: requires user judgment (what username to use).
            // Actionable guidance is in the details JSON and the eprintln below.
            fixable: false,
            details: Some(serde_json::json!({
                "action": "wsp config set branch-prefix <your-github-username>",
                "tip": "run `wsp setup` to auto-detect your GitHub username via gh"
            })),
        });
        eprintln!("  ⚠ branch-prefix not set");
        eprintln!("    Run `wsp setup` or: wsp config set branch-prefix <your-github-username>");
    } else {
        checks.push(DoctorCheck {
            scope: "global".into(),
            check: "branch-prefix".into(),
            status: CheckStatus::Ok,
            message: format!(
                "branch-prefix: {}",
                cfg.branch_prefix.as_deref().expect("checked is_some above")
            ),
            fixable: false,
            details: None,
        });
        eprintln!(
            "  ✓ branch-prefix: {}",
            cfg.branch_prefix.as_deref().expect("checked is_some above")
        );
    }
}

/// W1. Metadata version skew.
fn check_metadata_version(
    meta: &workspace::Metadata,
    ws_scope: &str,
    checks: &mut Vec<DoctorCheck>,
) {
    if meta.version > workspace::CURRENT_METADATA_VERSION {
        checks.push(DoctorCheck {
            scope: ws_scope.into(),
            check: "metadata-version".into(),
            status: CheckStatus::Warn,
            message: format!(
                "metadata version {} is newer than supported version {}",
                meta.version,
                workspace::CURRENT_METADATA_VERSION
            ),
            fixable: false,
            details: Some(serde_json::json!({
                "metadata_version": meta.version,
                "supported_version": workspace::CURRENT_METADATA_VERSION,
            })),
        });
        eprintln!(
            "  ⚠ metadata version {} is newer than supported version {}",
            meta.version,
            workspace::CURRENT_METADATA_VERSION
        );
    } else {
        checks.push(DoctorCheck {
            scope: ws_scope.into(),
            check: "metadata-version".into(),
            status: CheckStatus::Ok,
            message: format!("metadata version {}", meta.version),
            fixable: false,
            details: None,
        });
        eprintln!("  ✓ metadata version {}", meta.version);
    }
}

/// G1. Orphaned mirrors — mirror dirs with no corresponding config entry.
fn check_orphaned_mirrors(
    paths: &Paths,
    cfg: &config::Config,
    fix: bool,
    checks: &mut Vec<DoctorCheck>,
    fixed: &mut usize,
) {
    if !paths.mirrors_dir.exists() {
        return;
    }

    let mut orphaned = Vec::new();

    // Walk mirrors_dir/<host>/<owner>/<repo>.git
    let hosts = match fs::read_dir(&paths.mirrors_dir) {
        Ok(rd) => rd,
        Err(_) => return,
    };
    for host_entry in hosts.flatten() {
        if !host_entry.path().is_dir() {
            continue;
        }
        // Skip non-UTF8 dir names to avoid lossy conversion producing identity collisions
        let host = match host_entry.file_name().to_str() {
            Some(s) => s.to_string(),
            None => continue,
        };
        let owners = match fs::read_dir(host_entry.path()) {
            Ok(rd) => rd,
            Err(_) => continue,
        };
        for owner_entry in owners.flatten() {
            if !owner_entry.path().is_dir() {
                continue;
            }
            let owner = match owner_entry.file_name().to_str() {
                Some(s) => s.to_string(),
                None => continue,
            };
            let repos = match fs::read_dir(owner_entry.path()) {
                Ok(rd) => rd,
                Err(_) => continue,
            };
            for repo_entry in repos.flatten() {
                let name = match repo_entry.file_name().to_str() {
                    Some(s) => s.to_string(),
                    None => continue,
                };
                if !name.ends_with(".git") || !repo_entry.path().is_dir() {
                    continue;
                }
                let repo = name.trim_end_matches(".git");
                let identity = format!("{}/{}/{}", host, owner, repo);
                if !cfg.repos.contains_key(&identity) {
                    orphaned.push((identity, repo_entry.path()));
                }
            }
        }
    }

    if orphaned.is_empty() {
        checks.push(DoctorCheck {
            scope: "global".into(),
            check: "orphaned-mirrors".into(),
            status: CheckStatus::Ok,
            message: "no orphaned mirrors".into(),
            fixable: false,
            details: None,
        });
        eprintln!("  ✓ no orphaned mirrors");
    } else {
        for (identity, path) in &orphaned {
            let fixable = true;
            if fix {
                // Verify the path is still a real directory (not a symlink) to
                // narrow the TOCTOU window between scan and deletion.
                match fs::symlink_metadata(path) {
                    Ok(m) if m.file_type().is_symlink() => {
                        checks.push(DoctorCheck {
                            scope: "global".into(),
                            check: "orphaned-mirrors".into(),
                            status: CheckStatus::Warn,
                            message: format!(
                                "{}: orphaned mirror is a symlink, skipping removal",
                                identity
                            ),
                            fixable: false,
                            details: None,
                        });
                        eprintln!(
                            "  ⚠ {}: orphaned mirror is a symlink, skipping removal",
                            identity
                        );
                        continue;
                    }
                    Err(_) => continue, // vanished between scan and fix
                    _ => {}
                }
                match fs::remove_dir_all(path) {
                    Ok(()) => {
                        checks.push(DoctorCheck {
                            scope: "global".into(),
                            check: "orphaned-mirrors".into(),
                            status: CheckStatus::Ok,
                            message: format!("{}: removed orphaned mirror", identity),
                            fixable,
                            details: None,
                        });
                        eprintln!("  ✓ {}: removed orphaned mirror", identity);
                        *fixed += 1;
                        continue;
                    }
                    Err(e) => {
                        checks.push(DoctorCheck {
                            scope: "global".into(),
                            check: "orphaned-mirrors".into(),
                            status: CheckStatus::Warn,
                            message: format!(
                                "{}: orphaned mirror, removal failed: {}",
                                identity, e
                            ),
                            fixable,
                            details: None,
                        });
                        eprintln!("  ⚠ {}: orphaned mirror, removal failed: {}", identity, e);
                        continue;
                    }
                }
            }
            checks.push(DoctorCheck {
                scope: "global".into(),
                check: "orphaned-mirrors".into(),
                status: CheckStatus::Warn,
                message: format!("{}: mirror has no config entry", identity),
                fixable,
                details: None,
            });
            eprintln!("  ⚠ {}: mirror has no config entry", identity);
        }
    }
}

/// G4. GC stale entries — entries past retention that should have been purged.
fn check_gc_stale_entries(
    paths: &Paths,
    cfg: &config::Config,
    fix: bool,
    checks: &mut Vec<DoctorCheck>,
    fixed: &mut usize,
) {
    if !paths.gc_dir.exists() {
        checks.push(DoctorCheck {
            scope: "global".into(),
            check: "gc-stale-entries".into(),
            status: CheckStatus::Ok,
            message: "no gc entries".into(),
            fixable: false,
            details: None,
        });
        eprintln!("  ✓ no gc entries");
        return;
    }

    let retention_days = cfg.gc_retention_days.unwrap_or(gc::DEFAULT_RETENTION_DAYS);
    let cutoff = chrono::Utc::now() - chrono::Duration::days(retention_days as i64);

    let entries = match gc::list(&paths.gc_dir) {
        Ok(e) => e,
        Err(e) => {
            checks.push(DoctorCheck {
                scope: "global".into(),
                check: "gc-stale-entries".into(),
                status: CheckStatus::Warn,
                message: format!("failed to list gc entries: {}", e),
                fixable: false,
                details: None,
            });
            eprintln!("  ⚠ failed to list gc entries: {}", e);
            return;
        }
    };

    let stale: Vec<_> = entries.iter().filter(|e| e.trashed_at < cutoff).collect();

    if stale.is_empty() {
        checks.push(DoctorCheck {
            scope: "global".into(),
            check: "gc-stale-entries".into(),
            status: CheckStatus::Ok,
            message: format!(
                "{} gc entries, none past {}-day retention",
                entries.len(),
                retention_days
            ),
            fixable: false,
            details: None,
        });
        eprintln!(
            "  ✓ {} gc entries, none past {}-day retention",
            entries.len(),
            retention_days
        );
    } else {
        let fixable = true;
        if fix {
            match gc::purge(&paths.gc_dir, retention_days) {
                Ok(removed) => {
                    checks.push(DoctorCheck {
                        scope: "global".into(),
                        check: "gc-stale-entries".into(),
                        status: CheckStatus::Ok,
                        message: format!("purged {} stale gc entries", removed),
                        fixable,
                        details: None,
                    });
                    eprintln!("  ✓ purged {} stale gc entries", removed);
                    *fixed += 1;
                }
                Err(e) => {
                    checks.push(DoctorCheck {
                        scope: "global".into(),
                        check: "gc-stale-entries".into(),
                        status: CheckStatus::Warn,
                        message: format!("{} stale gc entries, purge failed: {}", stale.len(), e),
                        fixable,
                        details: None,
                    });
                    eprintln!("  ⚠ {} stale gc entries, purge failed: {}", stale.len(), e);
                }
            }
        } else {
            let names: Vec<_> = stale.iter().map(|e| e.name.as_str()).collect();
            checks.push(DoctorCheck {
                scope: "global".into(),
                check: "gc-stale-entries".into(),
                status: CheckStatus::Warn,
                message: format!(
                    "{} gc entries past {}-day retention",
                    stale.len(),
                    retention_days
                ),
                fixable,
                details: Some(serde_json::json!({ "stale_entries": names })),
            });
            eprintln!(
                "  ⚠ {} gc entries past {}-day retention",
                stale.len(),
                retention_days
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Workspace-level check helpers
// ---------------------------------------------------------------------------

/// W2. Legacy wsp-mirror remote — stale remote from pre-v0.5.
fn check_legacy_wsp_mirror(
    clone_dir: &std::path::Path,
    dir_name: &str,
    scope: &str,
    fix: bool,
    checks: &mut Vec<DoctorCheck>,
    fixed: &mut usize,
) {
    if !git::has_remote(clone_dir, "wsp-mirror") {
        return;
    }

    let fixable = true;
    if fix {
        match git::remove_remote(clone_dir, "wsp-mirror") {
            Ok(()) => {
                checks.push(DoctorCheck {
                    scope: scope.into(),
                    check: "legacy-wsp-mirror-remote".into(),
                    status: CheckStatus::Ok,
                    message: format!("{}: removed legacy wsp-mirror remote", dir_name),
                    fixable,
                    details: None,
                });
                eprintln!("  ✓ {}: removed legacy wsp-mirror remote", dir_name);
                *fixed += 1;
            }
            Err(e) => {
                checks.push(DoctorCheck {
                    scope: scope.into(),
                    check: "legacy-wsp-mirror-remote".into(),
                    status: CheckStatus::Warn,
                    message: format!(
                        "{}: legacy wsp-mirror remote, removal failed: {}",
                        dir_name, e
                    ),
                    fixable,
                    details: None,
                });
                eprintln!(
                    "  ⚠ {}: legacy wsp-mirror remote, removal failed: {}",
                    dir_name, e
                );
            }
        }
    } else {
        checks.push(DoctorCheck {
            scope: scope.into(),
            check: "legacy-wsp-mirror-remote".into(),
            status: CheckStatus::Warn,
            message: format!("{}: has legacy wsp-mirror remote", dir_name),
            fixable,
            details: None,
        });
        eprintln!("  ⚠ {}: has legacy wsp-mirror remote", dir_name);
    }
}

/// W7. In-progress git operation — interrupted rebase or merge.
fn check_in_progress_op(
    clone_dir: &std::path::Path,
    dir_name: &str,
    scope: &str,
    checks: &mut Vec<DoctorCheck>,
) {
    if let Some(op) = git::in_progress_op(clone_dir) {
        let (op_name, hint) = match op {
            git::InProgressOp::Rebase => (
                "rebase",
                "run `git rebase --continue` or `git rebase --abort`",
            ),
            git::InProgressOp::Merge => {
                ("merge", "run `git merge --continue` or `git merge --abort`")
            }
        };
        checks.push(DoctorCheck {
            scope: scope.into(),
            check: "in-progress-git-op".into(),
            status: CheckStatus::Warn,
            message: format!(
                "{}: interrupted {} in progress — {}",
                dir_name, op_name, hint
            ),
            fixable: false,
            details: Some(serde_json::json!({ "operation": op_name, "hint": hint })),
        });
        eprintln!("  ⚠ {}: interrupted {} in progress", dir_name, op_name);
        eprintln!("      {}", hint);
    }
}

/// W3. Legacy ref field — stale @ref values in metadata.
fn check_legacy_ref_field(
    ws_dir: &std::path::Path,
    meta: &workspace::Metadata,
    ws_scope: &str,
    fix: bool,
    checks: &mut Vec<DoctorCheck>,
    fixed: &mut usize,
) {
    let stale_refs: Vec<String> = meta
        .repos
        .iter()
        .filter_map(|(identity, ref_opt)| {
            if let Some(repo_ref) = ref_opt
                && !repo_ref.r#ref.is_empty()
            {
                return Some(identity.clone());
            }
            None
        })
        .collect();

    if stale_refs.is_empty() {
        return;
    }

    let fixable = true;
    if fix {
        match wsp_core::filelock::with_metadata(ws_dir, |m| {
            for repo_ref in m.repos.values_mut().flatten() {
                repo_ref.r#ref = String::new();
            }
            Ok(())
        }) {
            Ok(_) => {
                checks.push(DoctorCheck {
                    scope: ws_scope.into(),
                    check: "legacy-ref-field".into(),
                    status: CheckStatus::Ok,
                    message: format!("cleared {} stale ref values", stale_refs.len()),
                    fixable,
                    details: None,
                });
                eprintln!("  ✓ cleared {} stale ref values", stale_refs.len());
                *fixed += 1;
            }
            Err(e) => {
                checks.push(DoctorCheck {
                    scope: ws_scope.into(),
                    check: "legacy-ref-field".into(),
                    status: CheckStatus::Warn,
                    message: format!(
                        "{} repos have stale ref values, fix failed: {}",
                        stale_refs.len(),
                        e
                    ),
                    fixable,
                    details: Some(serde_json::json!({ "identities": stale_refs })),
                });
                eprintln!(
                    "  ⚠ {} repos have stale ref values, fix failed: {}",
                    stale_refs.len(),
                    e
                );
            }
        }
    } else {
        checks.push(DoctorCheck {
            scope: ws_scope.into(),
            check: "legacy-ref-field".into(),
            status: CheckStatus::Warn,
            message: format!("{} repos have stale ref values", stale_refs.len()),
            fixable,
            details: Some(serde_json::json!({ "identities": stale_refs })),
        });
        eprintln!("  ⚠ {} repos have stale ref values", stale_refs.len());
    }
}

/// W4. Stale dirs map — orphaned entries in dirs collision map.
fn check_stale_dirs_map(
    ws_dir: &std::path::Path,
    meta: &workspace::Metadata,
    ws_scope: &str,
    fix: bool,
    checks: &mut Vec<DoctorCheck>,
    fixed: &mut usize,
) {
    let stale_entries: Vec<String> = meta
        .dirs
        .keys()
        .filter(|identity| !meta.repos.contains_key(*identity))
        .cloned()
        .collect();

    if stale_entries.is_empty() {
        return;
    }

    let fixable = true;
    if fix {
        match wsp_core::filelock::with_metadata(ws_dir, |m| {
            m.dirs.retain(|identity, _| m.repos.contains_key(identity));
            Ok(())
        }) {
            Ok(_) => {
                checks.push(DoctorCheck {
                    scope: ws_scope.into(),
                    check: "stale-dirs-map".into(),
                    status: CheckStatus::Ok,
                    message: format!("removed {} stale dirs entries", stale_entries.len()),
                    fixable,
                    details: None,
                });
                eprintln!("  ✓ removed {} stale dirs entries", stale_entries.len());
                *fixed += 1;
            }
            Err(e) => {
                checks.push(DoctorCheck {
                    scope: ws_scope.into(),
                    check: "stale-dirs-map".into(),
                    status: CheckStatus::Warn,
                    message: format!(
                        "{} stale dirs entries, fix failed: {}",
                        stale_entries.len(),
                        e
                    ),
                    fixable,
                    details: Some(serde_json::json!({ "identities": stale_entries })),
                });
                eprintln!(
                    "  ⚠ {} stale dirs entries, fix failed: {}",
                    stale_entries.len(),
                    e
                );
            }
        }
    } else {
        checks.push(DoctorCheck {
            scope: ws_scope.into(),
            check: "stale-dirs-map".into(),
            status: CheckStatus::Warn,
            message: format!(
                "{} dirs entries for repos no longer in workspace",
                stale_entries.len()
            ),
            fixable,
            details: Some(serde_json::json!({ "identities": stale_entries })),
        });
        eprintln!(
            "  ⚠ {} dirs entries for repos no longer in workspace",
            stale_entries.len()
        );
    }
}

/// W12. Unregistered repos — workspace repos not in global registry.
#[allow(clippy::too_many_arguments)]
fn check_unregistered_repos(
    ws_dir: &std::path::Path,
    meta: &workspace::Metadata,
    cfg: &config::Config,
    paths: &Paths,
    ws_scope: &str,
    fix: bool,
    checks: &mut Vec<DoctorCheck>,
    fixed: &mut usize,
) {
    let unregistered: Vec<&str> = meta
        .repos
        .keys()
        .filter(|identity| !cfg.repos.contains_key(identity.as_str()))
        .map(|s| s.as_str())
        .collect();

    if unregistered.is_empty() {
        checks.push(DoctorCheck {
            scope: ws_scope.into(),
            check: "unregistered-repos".into(),
            status: CheckStatus::Ok,
            message: "all workspace repos are in global registry".into(),
            fixable: false,
            details: None,
        });
        eprintln!("  ✓ all workspace repos are in global registry");
    } else {
        let fixable = true;
        if fix {
            // Collect identity → URL from clone origins, cloning mirrors as needed
            let mut to_register: Vec<(String, String)> = Vec::new();
            let mut clone_failures: Vec<String> = Vec::new();
            for identity in &unregistered {
                let dir_name = match meta.dir_name(identity) {
                    Ok(d) => d,
                    Err(_) => continue,
                };
                let clone_dir = ws_dir.join(&dir_name);
                if let Ok(url) = git::remote_get_url(&clone_dir, "origin") {
                    let url = url.trim().to_string();
                    if !url.is_empty() {
                        // Ensure mirror exists before registering
                        if let Ok(parsed) = giturl::parse(&url)
                            && !mirror::exists(&paths.mirrors_dir, &parsed)
                        {
                            eprintln!("  cloning mirror for {}...", identity);
                            if let Err(e) = mirror::clone(&paths.mirrors_dir, &parsed, &url) {
                                clone_failures.push(format!("{}: {}", identity, e));
                                continue;
                            }
                        }
                        to_register.push((identity.to_string(), url));
                    }
                }
            }

            if !clone_failures.is_empty() {
                checks.push(DoctorCheck {
                    scope: ws_scope.into(),
                    check: "unregistered-repos".into(),
                    status: CheckStatus::Warn,
                    message: format!(
                        "{} workspace repo(s) failed to clone mirrors",
                        clone_failures.len()
                    ),
                    fixable,
                    details: Some(serde_json::json!({ "failures": clone_failures })),
                });
                eprintln!(
                    "  ⚠ {} workspace repo(s) failed to clone mirrors",
                    clone_failures.len()
                );
                if to_register.is_empty() {
                    return;
                }
            }

            if !to_register.is_empty() {
                match filelock::with_config(&paths.config_path, |locked_cfg| {
                    for (identity, url) in &to_register {
                        if !locked_cfg.repos.contains_key(identity) {
                            locked_cfg.repos.insert(
                                identity.clone(),
                                config::RepoEntry {
                                    url: url.clone(),
                                    added: chrono::Utc::now(),
                                    setup_commands: None,
                                },
                            );
                        }
                    }
                    Ok(())
                }) {
                    Ok(_) => {
                        checks.push(DoctorCheck {
                            scope: ws_scope.into(),
                            check: "unregistered-repos".into(),
                            status: CheckStatus::Ok,
                            message: format!("registered {} workspace repo(s)", to_register.len()),
                            fixable,
                            details: None,
                        });
                        eprintln!("  ✓ registered {} workspace repo(s)", to_register.len());
                        *fixed += 1;
                        return;
                    }
                    Err(e) => {
                        checks.push(DoctorCheck {
                            scope: ws_scope.into(),
                            check: "unregistered-repos".into(),
                            status: CheckStatus::Warn,
                            message: format!("failed to register workspace repos: {}", e),
                            fixable,
                            details: None,
                        });
                        eprintln!("  ⚠ failed to register workspace repos: {}", e);
                        return;
                    }
                }
            }
        }
        checks.push(DoctorCheck {
            scope: ws_scope.into(),
            check: "unregistered-repos".into(),
            status: CheckStatus::Warn,
            message: format!(
                "{} workspace repos not in global registry",
                unregistered.len()
            ),
            fixable,
            details: Some(serde_json::json!({ "identities": unregistered })),
        });
        eprintln!(
            "  ⚠ {} workspace repos not in global registry: {}",
            unregistered.len(),
            unregistered.join(", ")
        );
    }
}

/// W9. AGENTS.md / CLAUDE.md validity.
fn check_agents_md_valid(
    ws_dir: &std::path::Path,
    meta: &workspace::Metadata,
    ws_scope: &str,
    fix: bool,
    checks: &mut Vec<DoctorCheck>,
    fixed: &mut usize,
) {
    let agents_path = ws_dir.join("AGENTS.md");
    let claude_path = ws_dir.join("CLAUDE.md");

    let mut problems = Vec::new();

    // Check AGENTS.md markers
    if agents_path.exists() {
        match fs::read_to_string(&agents_path) {
            Ok(content) => {
                if !content.contains(agentmd::MARKER_BEGIN)
                    || !content.contains(agentmd::MARKER_END)
                {
                    problems.push("AGENTS.md missing wsp markers");
                }
            }
            Err(_) => {
                problems.push("AGENTS.md unreadable");
            }
        }
    } else {
        problems.push("AGENTS.md missing");
    }

    // Check CLAUDE.md symlink
    match fs::symlink_metadata(&claude_path) {
        Ok(m) => {
            if m.file_type().is_symlink() {
                match fs::read_link(&claude_path) {
                    Ok(target) if target != std::path::Path::new("AGENTS.md") => {
                        problems.push("CLAUDE.md symlinks to wrong target");
                    }
                    Err(_) => {
                        problems.push("CLAUDE.md symlink unreadable");
                    }
                    _ => {} // correct symlink
                }
            } else {
                problems.push("CLAUDE.md is not a symlink to AGENTS.md");
            }
        }
        Err(_) => {
            // CLAUDE.md doesn't exist — only a problem if AGENTS.md exists
            if agents_path.exists() {
                problems.push("CLAUDE.md missing (should be symlink to AGENTS.md)");
            }
        }
    }

    if problems.is_empty() {
        checks.push(DoctorCheck {
            scope: ws_scope.into(),
            check: "agents-md-valid".into(),
            status: CheckStatus::Ok,
            message: "AGENTS.md and CLAUDE.md are valid".into(),
            fixable: false,
            details: None,
        });
        eprintln!("  ✓ AGENTS.md and CLAUDE.md are valid");
    } else {
        let fixable = true;
        if fix {
            match agentmd::update(ws_dir, meta) {
                Ok(()) => {
                    // agentmd::update already ran ensure_symlink, which repairs a
                    // missing or stale symlink but leaves a regular-file CLAUDE.md
                    // alone — repairing that is doctor's job. Only rewrite when the
                    // link isn't already correct, so we never destroy a valid
                    // symlink (which on Windows without Developer Mode couldn't be
                    // recreated). The shared helper skips gracefully on os 1314.
                    let already_linked = fs::read_link(&claude_path)
                        .map(|t| t == std::path::Path::new("AGENTS.md"))
                        .unwrap_or(false);
                    let link_result = if already_linked {
                        Ok(true)
                    } else {
                        let _ = fs::remove_file(&claude_path);
                        symlink::symlink_file_or_skip("AGENTS.md", &claude_path)
                    };
                    match link_result {
                        Ok(true) => {
                            checks.push(DoctorCheck {
                                scope: ws_scope.into(),
                                check: "agents-md-valid".into(),
                                status: CheckStatus::Ok,
                                message: "regenerated AGENTS.md and CLAUDE.md".into(),
                                fixable,
                                details: None,
                            });
                            eprintln!("  ✓ regenerated AGENTS.md and CLAUDE.md");
                            *fixed += 1;
                        }
                        // Symlink unavailable (e.g. Windows without Developer Mode):
                        // AGENTS.md was regenerated, but CLAUDE.md can't be linked.
                        // Report a Warn (not a counted fix) so this stays consistent
                        // with a plain re-run and tells the user how to resolve it.
                        Ok(false) => {
                            let msg = "regenerated AGENTS.md; CLAUDE.md symlink needs Developer Mode (Windows)";
                            checks.push(DoctorCheck {
                                scope: ws_scope.into(),
                                check: "agents-md-valid".into(),
                                status: CheckStatus::Warn,
                                message: msg.into(),
                                fixable,
                                details: Some(serde_json::json!({ "problems": problems })),
                            });
                            eprintln!("  ⚠ {msg}");
                        }
                        Err(e) => {
                            checks.push(DoctorCheck {
                                scope: ws_scope.into(),
                                check: "agents-md-valid".into(),
                                status: CheckStatus::Warn,
                                message: format!(
                                    "AGENTS.md regenerated but CLAUDE.md symlink failed: {}",
                                    e
                                ),
                                fixable,
                                details: Some(serde_json::json!({ "problems": problems })),
                            });
                            eprintln!(
                                "  ⚠ AGENTS.md regenerated but CLAUDE.md symlink failed: {}",
                                e
                            );
                        }
                    }
                }
                Err(e) => {
                    checks.push(DoctorCheck {
                        scope: ws_scope.into(),
                        check: "agents-md-valid".into(),
                        status: CheckStatus::Warn,
                        message: format!("AGENTS.md/CLAUDE.md issues, fix failed: {}", e),
                        fixable,
                        details: Some(serde_json::json!({ "problems": problems })),
                    });
                    eprintln!("  ⚠ AGENTS.md/CLAUDE.md issues, fix failed: {}", e);
                }
            }
        } else {
            checks.push(DoctorCheck {
                scope: ws_scope.into(),
                check: "agents-md-valid".into(),
                status: CheckStatus::Warn,
                message: problems.join("; "),
                fixable,
                details: Some(serde_json::json!({ "problems": problems })),
            });
            for p in &problems {
                eprintln!("  ⚠ {}", p);
            }
        }
    }
}

/// G3. Workspaces dir exists.
fn check_workspaces_dir_exists(
    paths: &Paths,
    fix: bool,
    checks: &mut Vec<DoctorCheck>,
    fixed: &mut usize,
) {
    if paths.workspaces_dir.exists() {
        checks.push(DoctorCheck {
            scope: "global".into(),
            check: "workspaces-dir-exists".into(),
            status: CheckStatus::Ok,
            message: format!("workspaces dir exists: {}", paths.workspaces_dir.display()),
            fixable: false,
            details: None,
        });
        eprintln!(
            "  ✓ workspaces dir exists: {}",
            paths.workspaces_dir.display()
        );
    } else {
        let fixable = true;
        if fix {
            match fs::create_dir_all(&paths.workspaces_dir) {
                Ok(()) => {
                    checks.push(DoctorCheck {
                        scope: "global".into(),
                        check: "workspaces-dir-exists".into(),
                        status: CheckStatus::Ok,
                        message: format!(
                            "created workspaces dir: {}",
                            paths.workspaces_dir.display()
                        ),
                        fixable,
                        details: None,
                    });
                    eprintln!(
                        "  ✓ created workspaces dir: {}",
                        paths.workspaces_dir.display()
                    );
                    *fixed += 1;
                }
                Err(e) => {
                    checks.push(DoctorCheck {
                        scope: "global".into(),
                        check: "workspaces-dir-exists".into(),
                        status: CheckStatus::Error,
                        message: format!("failed to create workspaces dir: {}", e),
                        fixable,
                        details: None,
                    });
                    eprintln!("  ✗ failed to create workspaces dir: {}", e);
                }
            }
        } else {
            checks.push(DoctorCheck {
                scope: "global".into(),
                check: "workspaces-dir-exists".into(),
                status: CheckStatus::Error,
                message: format!("workspaces dir missing: {}", paths.workspaces_dir.display()),
                fixable,
                details: None,
            });
            eprintln!(
                "  ✗ workspaces dir missing: {}",
                paths.workspaces_dir.display()
            );
        }
    }
}

/// G5. GC orphaned entries — dirs in gc/ without valid metadata.
fn check_gc_orphaned_entries(paths: &Paths, checks: &mut Vec<DoctorCheck>) {
    if !paths.gc_dir.exists() {
        return;
    }

    let mut orphaned = Vec::new();
    if let Ok(entries) = fs::read_dir(&paths.gc_dir) {
        for item in entries.flatten() {
            let path = item.path();
            if !path.is_dir() {
                continue;
            }
            let meta_path = path.join(".wsp-gc.yaml");
            let is_orphaned = if meta_path.exists() {
                // Metadata exists but might be corrupt
                match fs::read_to_string(&meta_path) {
                    Ok(data) => serde_yaml_ng::from_str::<gc::GcEntry>(&data).is_err(),
                    Err(_) => true,
                }
            } else {
                true
            };
            if is_orphaned && let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                orphaned.push(name.to_string());
            }
        }
    }

    if orphaned.is_empty() {
        // No check emitted when clean — only report problems
    } else {
        checks.push(DoctorCheck {
            scope: "global".into(),
            check: "gc-orphaned-entries".into(),
            status: CheckStatus::Warn,
            message: format!(
                "{} gc {} without valid metadata",
                orphaned.len(),
                if orphaned.len() == 1 {
                    "entry"
                } else {
                    "entries"
                }
            ),
            fixable: false,
            details: Some(serde_json::json!({ "orphaned": orphaned })),
        });
        eprintln!(
            "  ⚠ {} gc {} without valid metadata",
            orphaned.len(),
            if orphaned.len() == 1 {
                "entry"
            } else {
                "entries"
            }
        );
    }
}

/// G6. GC disk usage — informational.
fn check_gc_disk_usage(paths: &Paths, checks: &mut Vec<DoctorCheck>) {
    if !paths.gc_dir.exists() {
        return;
    }

    let total_bytes = dir_size(&paths.gc_dir);
    let human = format_bytes(total_bytes);

    checks.push(DoctorCheck {
        scope: "global".into(),
        check: "gc-disk-usage".into(),
        status: CheckStatus::Ok,
        message: format!("gc disk usage: {}", human),
        fixable: false,
        details: Some(serde_json::json!({ "bytes": total_bytes })),
    });
    eprintln!("  ✓ gc disk usage: {}", human);
}

/// G7. Template repos parseable — all repo URLs in templates parse via giturl.
fn check_template_repos_parseable(paths: &Paths, checks: &mut Vec<DoctorCheck>) {
    let names = match template::list(&paths.templates_dir) {
        Ok(n) => n,
        Err(_) => return, // No templates dir
    };
    if names.is_empty() {
        return;
    }

    let mut bad = Vec::new();
    for name in &names {
        if let Ok(tmpl) = template::load(&paths.templates_dir, name) {
            for repo in &tmpl.repos {
                if giturl::parse(&repo.url).is_err() {
                    bad.push(format!("{}:{}", name, repo.url));
                }
            }
        }
    }

    if bad.is_empty() {
        checks.push(DoctorCheck {
            scope: "global".into(),
            check: "template-repos-parseable".into(),
            status: CheckStatus::Ok,
            message: format!("{} template(s) have valid repo URLs", names.len()),
            fixable: false,
            details: None,
        });
        eprintln!("  ✓ {} template(s) have valid repo URLs", names.len());
    } else {
        checks.push(DoctorCheck {
            scope: "global".into(),
            check: "template-repos-parseable".into(),
            status: CheckStatus::Warn,
            message: format!(
                "{} template repo URL(s) failed to parse — \
                 edit with `wsp template repo <name> add/rm`",
                bad.len()
            ),
            fixable: false,
            details: Some(serde_json::json!({ "invalid_urls": bad })),
        });
        eprintln!("  ⚠ {} template repo URL(s) failed to parse", bad.len());
        for b in &bad {
            eprintln!("      {}", b);
        }
        eprintln!("      fix: `wsp template repo <name> add/rm`");
    }
}

/// G8. Template repos registered — template repos have corresponding mirrors.
fn check_template_repos_registered(
    paths: &Paths,
    cfg: &config::Config,
    fix: bool,
    checks: &mut Vec<DoctorCheck>,
    fixed: &mut usize,
) {
    let names = match template::list(&paths.templates_dir) {
        Ok(n) => n,
        Err(_) => return,
    };
    if names.is_empty() {
        return;
    }

    // Collect unregistered repos with their parsed URL info for fixing
    let mut unregistered: Vec<(String, giturl::Parsed, String)> = Vec::new();
    let mut unregistered_labels = Vec::new();
    for name in &names {
        if let Ok(tmpl) = template::load(&paths.templates_dir, name) {
            for repo in &tmpl.repos {
                if let Ok(parsed) = giturl::parse(&repo.url) {
                    let identity = parsed.identity();
                    if !cfg.repos.contains_key(&identity) {
                        unregistered_labels.push(format!("{}:{}", name, identity));
                        unregistered.push((identity, parsed, repo.url.clone()));
                    }
                }
            }
        }
    }

    if unregistered.is_empty() {
        checks.push(DoctorCheck {
            scope: "global".into(),
            check: "template-repos-registered".into(),
            status: CheckStatus::Ok,
            message: "all template repos have mirrors".into(),
            fixable: false,
            details: None,
        });
        eprintln!("  ✓ all template repos have mirrors");
    } else {
        let fixable = true;
        if fix {
            // Clone missing mirrors
            let mut clone_failures = Vec::new();
            for (identity, parsed, url) in &unregistered {
                if !mirror::exists(&paths.mirrors_dir, parsed) {
                    eprintln!("  ⚠ {}: not in registry, cloning {}...", identity, url);
                    if let Err(e) = mirror::clone(&paths.mirrors_dir, parsed, url) {
                        clone_failures.push(format!("{}: {}", identity, e));
                    }
                }
            }

            if !clone_failures.is_empty() {
                checks.push(DoctorCheck {
                    scope: "global".into(),
                    check: "template-repos-registered".into(),
                    status: CheckStatus::Warn,
                    message: format!(
                        "{} template repo(s) failed to clone mirrors",
                        clone_failures.len()
                    ),
                    fixable,
                    details: Some(serde_json::json!({ "failures": clone_failures })),
                });
                eprintln!(
                    "  ⚠ {} template repo(s) failed to clone mirrors",
                    clone_failures.len()
                );
                return;
            }

            // Register under lock
            let to_register: Vec<(String, String)> = unregistered
                .iter()
                .map(|(id, _, url)| (id.clone(), url.clone()))
                .collect();
            match filelock::with_config(&paths.config_path, |locked_cfg| {
                for (identity, url) in &to_register {
                    if !locked_cfg.repos.contains_key(identity) {
                        locked_cfg.repos.insert(
                            identity.clone(),
                            config::RepoEntry {
                                url: url.clone(),
                                added: chrono::Utc::now(),
                                setup_commands: None,
                            },
                        );
                    }
                }
                Ok(())
            }) {
                Ok(_) => {
                    checks.push(DoctorCheck {
                        scope: "global".into(),
                        check: "template-repos-registered".into(),
                        status: CheckStatus::Ok,
                        message: format!(
                            "registered {} template repo(s): {}",
                            unregistered.len(),
                            unregistered
                                .iter()
                                .map(|(id, _, _)| id.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                        fixable,
                        details: None,
                    });
                    eprintln!(
                        "  ✓ registered {} template repo(s): {}",
                        unregistered.len(),
                        unregistered
                            .iter()
                            .map(|(id, _, _)| id.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                    *fixed += 1;
                }
                Err(e) => {
                    checks.push(DoctorCheck {
                        scope: "global".into(),
                        check: "template-repos-registered".into(),
                        status: CheckStatus::Warn,
                        message: format!("failed to register template repos: {}", e),
                        fixable,
                        details: None,
                    });
                    eprintln!("  ⚠ failed to register template repos: {}", e);
                }
            }
            return;
        }
        checks.push(DoctorCheck {
            scope: "global".into(),
            check: "template-repos-registered".into(),
            status: CheckStatus::Warn,
            message: format!("{} template repo(s) not in registry", unregistered.len()),
            fixable,
            details: Some(serde_json::json!({ "unregistered": unregistered_labels })),
        });
        eprintln!(
            "  ⚠ {} template repo(s) not in registry: {}",
            unregistered.len(),
            unregistered
                .iter()
                .map(|(id, _, _)| id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
}

/// G11. Deprecated config keys — old-format keys that should be migrated.
///
/// Detects: `experimental` section present, or raw YAML has `git_config`/`language_integrations`.
/// Fix: load config via `Config::load_from()` (applies migration) → save via `save_to()`.
/// The serde `rename` + `skip_serializing` attributes do the rewriting automatically.
fn check_deprecated_config_keys(
    paths: &Paths,
    cfg: &config::Config,
    fix: bool,
    checks: &mut Vec<DoctorCheck>,
    fixed: &mut usize,
) {
    // Check for deprecated keys by reading raw YAML
    let mut deprecated: Vec<String> = Vec::new();

    if cfg.experimental.is_some() {
        deprecated.push("experimental".into());
    }

    // Check raw YAML for old field names (git_config, language_integrations)
    if let Ok(raw) = std::fs::read_to_string(&paths.config_path) {
        if raw.contains("git_config:") {
            deprecated.push("git_config (use git)".into());
        }
        if raw.contains("language_integrations:") {
            deprecated.push("language_integrations (use lang)".into());
        }
    }

    if deprecated.is_empty() {
        checks.push(DoctorCheck {
            scope: "global".into(),
            check: "deprecated-config-keys".into(),
            status: CheckStatus::Ok,
            message: "no deprecated config keys".into(),
            fixable: false,
            details: None,
        });
        eprintln!("  ✓ no deprecated config keys");
    } else {
        let fixable = true;
        if fix {
            // Load under lock (applies migration), save writes new format.
            // Using with_config avoids TOCTOU with concurrent `wsp config set`.
            match filelock::with_config(&paths.config_path, |_cfg| Ok(())) {
                Ok(_) => {
                    checks.push(DoctorCheck {
                        scope: "global".into(),
                        check: "deprecated-config-keys".into(),
                        status: CheckStatus::Ok,
                        message: format!(
                            "migrated deprecated config keys: {}",
                            deprecated.join(", ")
                        ),
                        fixable,
                        details: Some(serde_json::json!({ "migrated": deprecated })),
                    });
                    eprintln!(
                        "  ✓ migrated deprecated config keys: {}",
                        deprecated.join(", ")
                    );
                    *fixed += 1;
                    return;
                }
                Err(e) => {
                    checks.push(DoctorCheck {
                        scope: "global".into(),
                        check: "deprecated-config-keys".into(),
                        status: CheckStatus::Warn,
                        message: format!("deprecated config keys, migration failed: {}", e),
                        fixable,
                        details: Some(serde_json::json!({ "deprecated": deprecated })),
                    });
                    eprintln!("  ⚠ deprecated config keys, migration failed: {}", e);
                    return;
                }
            }
        }
        checks.push(DoctorCheck {
            scope: "global".into(),
            check: "deprecated-config-keys".into(),
            status: CheckStatus::Warn,
            message: format!(
                "deprecated config keys found: {} (run wsp doctor --fix to migrate)",
                deprecated.join(", ")
            ),
            fixable,
            details: Some(serde_json::json!({ "deprecated": deprecated })),
        });
        eprintln!("  ⚠ deprecated config keys: {}", deprecated.join(", "));
    }
}

/// W5. Missing dirs map — collision disambiguation needed but absent.
fn check_missing_dirs_map(
    ws_dir: &std::path::Path,
    meta: &workspace::Metadata,
    ws_scope: &str,
    fix: bool,
    checks: &mut Vec<DoctorCheck>,
    fixed: &mut usize,
) {
    let identities: Vec<&str> = meta.repos.keys().map(|s| s.as_str()).collect();
    let expected = match workspace::compute_dir_names(&identities) {
        Ok(d) => d,
        Err(_) => return,
    };

    // Check if metadata dirs map matches expected dirs map
    if meta.dirs == expected {
        return; // No mismatch
    }

    // Check if there are collisions that need entries but don't have them
    let mut missing: Vec<String> = Vec::new();
    for identity in expected.keys() {
        if !meta.dirs.contains_key(identity) {
            missing.push(identity.clone());
        }
    }

    // Also check for entries in meta.dirs that shouldn't be there (expected is empty but dirs has entries)
    let mut extra: Vec<String> = Vec::new();
    for identity in meta.dirs.keys() {
        if !expected.contains_key(identity) && meta.repos.contains_key(identity) {
            extra.push(identity.clone());
        }
    }

    // Check for value mismatches (same keys, different dir names)
    let value_mismatch = missing.is_empty()
        && extra.is_empty()
        && expected.iter().any(|(k, v)| meta.dirs.get(k) != Some(v));

    if missing.is_empty() && extra.is_empty() && !value_mismatch {
        return;
    }

    let fixable = true;
    if fix {
        match wsp_core::filelock::with_metadata(ws_dir, |m| {
            m.dirs = expected.clone();
            Ok(())
        }) {
            Ok(_) => {
                checks.push(DoctorCheck {
                    scope: ws_scope.into(),
                    check: "missing-dirs-map".into(),
                    status: CheckStatus::Ok,
                    message: "recomputed dirs collision map".into(),
                    fixable,
                    details: None,
                });
                eprintln!("  ✓ recomputed dirs collision map");
                *fixed += 1;
            }
            Err(e) => {
                checks.push(DoctorCheck {
                    scope: ws_scope.into(),
                    check: "missing-dirs-map".into(),
                    status: CheckStatus::Warn,
                    message: format!("dirs map mismatch, fix failed: {}", e),
                    fixable,
                    details: None,
                });
                eprintln!("  ⚠ dirs map mismatch, fix failed: {}", e);
            }
        }
    } else {
        let detail = if !missing.is_empty() {
            format!("missing collision entries for: {}", missing.join(", "))
        } else if !extra.is_empty() {
            format!("extra dirs entries for: {}", extra.join(", "))
        } else {
            "dirs map has incorrect directory name mappings".into()
        };
        checks.push(DoctorCheck {
            scope: ws_scope.into(),
            check: "missing-dirs-map".into(),
            status: CheckStatus::Warn,
            message: detail,
            fixable,
            details: Some(serde_json::json!({
                "expected": expected,
                "actual": meta.dirs,
            })),
        });
        eprintln!("  ⚠ dirs collision map out of sync");
    }
}

/// G10. Global wspignore defaults — check for expected default patterns.
fn check_wspignore_defaults(
    paths: &Paths,
    fix: bool,
    checks: &mut Vec<DoctorCheck>,
    fixed: &mut usize,
) {
    let wspignore_path = paths.data_dir().join("wspignore");
    if !wspignore_path.exists() {
        // ensure_global_wspignore will create it on next command; not an issue
        return;
    }

    let content = match fs::read_to_string(&wspignore_path) {
        Ok(c) => c,
        Err(_) => return,
    };

    // Check that each non-comment, non-empty line from DEFAULT_WSPIGNORE is present
    // Uses line-based matching to avoid substring false positives
    let content_lines: Vec<&str> = content.lines().map(|l| l.trim()).collect();
    let expected: Vec<&str> = workspace::DEFAULT_WSPIGNORE
        .lines()
        .filter(|l| {
            let t = l.trim();
            !t.is_empty() && !t.starts_with('#')
        })
        .collect();

    let missing: Vec<&&str> = expected
        .iter()
        .filter(|p| !content_lines.contains(&p.trim()))
        .collect();

    if missing.is_empty() {
        checks.push(DoctorCheck {
            scope: "global".into(),
            check: "wspignore-defaults".into(),
            status: CheckStatus::Ok,
            message: "global wspignore has all default patterns".into(),
            fixable: false,
            details: None,
        });
        eprintln!("  ✓ global wspignore has all default patterns");
    } else {
        let fixable = true;
        if fix {
            // Append missing patterns
            let mut append = String::new();
            append.push_str("\n# Added by wsp doctor --fix\n");
            for pattern in &missing {
                append.push_str(pattern);
                append.push('\n');
            }
            match std::fs::OpenOptions::new()
                .append(true)
                .open(&wspignore_path)
            {
                Ok(mut f) => {
                    use std::io::Write;
                    if f.write_all(append.as_bytes()).is_ok() {
                        checks.push(DoctorCheck {
                            scope: "global".into(),
                            check: "wspignore-defaults".into(),
                            status: CheckStatus::Ok,
                            message: format!(
                                "appended {} missing default pattern(s) to wspignore",
                                missing.len()
                            ),
                            fixable,
                            details: None,
                        });
                        eprintln!(
                            "  ✓ appended {} missing default pattern(s) to wspignore",
                            missing.len()
                        );
                        *fixed += 1;
                    } else {
                        checks.push(DoctorCheck {
                            scope: "global".into(),
                            check: "wspignore-defaults".into(),
                            status: CheckStatus::Warn,
                            message: "wspignore missing defaults, write failed".into(),
                            fixable,
                            details: None,
                        });
                        eprintln!("  ⚠ wspignore missing defaults, write failed");
                    }
                }
                Err(_) => {
                    checks.push(DoctorCheck {
                        scope: "global".into(),
                        check: "wspignore-defaults".into(),
                        status: CheckStatus::Warn,
                        message: "wspignore missing defaults, could not open file".into(),
                        fixable,
                        details: None,
                    });
                    eprintln!("  ⚠ wspignore missing defaults, could not open file");
                }
            }
        } else {
            let missing_strs: Vec<&str> = missing.iter().map(|s| **s).collect();
            checks.push(DoctorCheck {
                scope: "global".into(),
                check: "wspignore-defaults".into(),
                status: CheckStatus::Warn,
                message: format!(
                    "global wspignore missing {} default pattern(s)",
                    missing.len()
                ),
                fixable,
                details: Some(serde_json::json!({ "missing_patterns": missing_strs })),
            });
            eprintln!(
                "  ⚠ global wspignore missing {} default pattern(s): {}",
                missing.len(),
                missing_strs.join(", ")
            );
        }
    }
}

/// W11. go.work validity — check wsp-managed go.work header and regenerate if needed.
fn check_go_work_valid(
    ws_dir: &std::path::Path,
    meta: &workspace::Metadata,
    ws_scope: &str,
    fix: bool,
    checks: &mut Vec<DoctorCheck>,
    fixed: &mut usize,
) {
    let go_work_path = ws_dir.join("go.work");
    if !go_work_path.exists() {
        // No go.work — check if Go integration would create one
        let go = lang::go::GoIntegration;
        if lang::LanguageIntegration::detect(&go, ws_dir, meta) {
            checks.push(DoctorCheck {
                scope: ws_scope.into(),
                check: "go-work-valid".into(),
                status: CheckStatus::Warn,
                message: "Go repos detected but go.work is missing".into(),
                fixable: true,
                details: None,
            });
            eprintln!("  ⚠ Go repos detected but go.work is missing");
            if fix && let Ok(()) = lang::LanguageIntegration::apply(&go, ws_dir, meta) {
                // Re-emit as fixed
                let last = checks.last_mut().unwrap();
                last.status = CheckStatus::Ok;
                last.message = "generated go.work".into();
                eprintln!("  ✓ generated go.work");
                *fixed += 1;
            }
        }
        return;
    }

    // go.work exists — check if it has the wsp header
    if let Some(problem) = workspace::check_go_work(ws_dir) {
        let fixable = true;
        if fix {
            let go = lang::go::GoIntegration;
            match lang::LanguageIntegration::apply(&go, ws_dir, meta) {
                Ok(()) => {
                    checks.push(DoctorCheck {
                        scope: ws_scope.into(),
                        check: "go-work-valid".into(),
                        status: CheckStatus::Ok,
                        message: "regenerated go.work".into(),
                        fixable,
                        details: None,
                    });
                    eprintln!("  ✓ regenerated go.work");
                    *fixed += 1;
                }
                Err(e) => {
                    checks.push(DoctorCheck {
                        scope: ws_scope.into(),
                        check: "go-work-valid".into(),
                        status: CheckStatus::Warn,
                        message: format!("go.work: {}, fix failed: {}", problem, e),
                        fixable,
                        details: None,
                    });
                    eprintln!("  ⚠ go.work: {}, fix failed: {}", problem, e);
                }
            }
        } else {
            checks.push(DoctorCheck {
                scope: ws_scope.into(),
                check: "go-work-valid".into(),
                status: CheckStatus::Warn,
                message: format!("go.work: {}", problem),
                fixable,
                details: None,
            });
            eprintln!("  ⚠ go.work: {}", problem);
        }
    } else {
        checks.push(DoctorCheck {
            scope: ws_scope.into(),
            check: "go-work-valid".into(),
            status: CheckStatus::Ok,
            message: "go.work is valid".into(),
            fixable: false,
            details: None,
        });
        eprintln!("  ✓ go.work is valid");
    }
}

/// W13. Mirror refspec — check clone mirrors have correct fetch refspecs.
fn check_mirror_refspec(
    clone_dir: &std::path::Path,
    dir_name: &str,
    scope: &str,
    fix: bool,
    checks: &mut Vec<DoctorCheck>,
    fixed: &mut usize,
) {
    let expected_refspec = "+refs/heads/*:refs/remotes/origin/*";
    let output = match git::remote_get_url(clone_dir, "origin") {
        Ok(_) => {
            // Check fetch refspec
            match std::process::Command::new("git")
                .args(["config", "--get-all", "remote.origin.fetch"])
                .current_dir(clone_dir)
                .output()
            {
                Ok(o) => o,
                Err(_) => return,
            }
        }
        Err(_) => return,
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let refspecs: Vec<&str> = stdout.lines().collect();

    if refspecs.contains(&expected_refspec) {
        return; // Correct refspec present, no check emitted
    }

    let fixable = true;
    if fix {
        let result = std::process::Command::new("git")
            .args(["config", "--add", "remote.origin.fetch", expected_refspec])
            .current_dir(clone_dir)
            .output();
        match result {
            Ok(o) if o.status.success() => {
                checks.push(DoctorCheck {
                    scope: scope.into(),
                    check: "mirror-refspec".into(),
                    status: CheckStatus::Ok,
                    message: format!("{}: added missing fetch refspec", dir_name),
                    fixable,
                    details: None,
                });
                eprintln!("  ✓ {}: added missing fetch refspec", dir_name);
                *fixed += 1;
            }
            _ => {
                checks.push(DoctorCheck {
                    scope: scope.into(),
                    check: "mirror-refspec".into(),
                    status: CheckStatus::Warn,
                    message: format!("{}: missing fetch refspec, fix failed", dir_name),
                    fixable,
                    details: None,
                });
                eprintln!("  ⚠ {}: missing fetch refspec, fix failed", dir_name);
            }
        }
    } else {
        checks.push(DoctorCheck {
            scope: scope.into(),
            check: "mirror-refspec".into(),
            status: CheckStatus::Warn,
            message: format!("{}: missing expected fetch refspec", dir_name),
            fixable,
            details: Some(serde_json::json!({
                "current_refspecs": refspecs,
                "expected": expected_refspec,
            })),
        });
        eprintln!("  ⚠ {}: missing expected fetch refspec", dir_name);
    }
}

/// W14. Git config drift — clone's local git config differs from effective config.
fn check_git_config_drift(
    ws_dir: &std::path::Path,
    meta: &workspace::Metadata,
    effective_gc: &std::collections::BTreeMap<String, String>,
    ws_scope: &str,
    fix: bool,
    checks: &mut Vec<DoctorCheck>,
    fixed: &mut usize,
) {
    if effective_gc.is_empty() {
        return;
    }

    let repo_infos = meta.repo_infos(ws_dir);
    let mut all_drifted: Vec<serde_json::Value> = Vec::new();

    for info in &repo_infos {
        if info.error.is_some() || !info.clone_dir.join(".git").exists() {
            continue;
        }

        let mut drifted_keys: Vec<serde_json::Value> = Vec::new();
        for (key, expected) in effective_gc {
            let actual = git::get_config(&info.clone_dir, key).ok();
            if actual.as_deref() != Some(expected.as_str()) {
                drifted_keys.push(serde_json::json!({
                    "key": key,
                    "expected": expected,
                    "actual": actual,
                }));
            }
        }

        if drifted_keys.is_empty() {
            continue;
        }

        all_drifted.push(serde_json::json!({
            "repo": info.identity,
            "dir": info.dir_name,
            "keys": drifted_keys,
        }));
    }

    if all_drifted.is_empty() {
        return;
    }

    let total_keys: usize = all_drifted
        .iter()
        .map(|r| r["keys"].as_array().map_or(0, |a| a.len()))
        .sum();
    let repo_count = all_drifted.len();

    if fix {
        workspace::apply_git_config(ws_dir, meta, effective_gc, None);
        checks.push(DoctorCheck {
            scope: ws_scope.into(),
            check: "git-config-drift".into(),
            status: CheckStatus::Ok,
            message: format!(
                "applied {} git config value{} across {} repo{}",
                total_keys,
                if total_keys == 1 { "" } else { "s" },
                repo_count,
                if repo_count == 1 { "" } else { "s" },
            ),
            fixable: true,
            details: None,
        });
        eprintln!(
            "  ✓ applied {} git config value{} across {} repo{}",
            total_keys,
            if total_keys == 1 { "" } else { "s" },
            repo_count,
            if repo_count == 1 { "" } else { "s" },
        );
        *fixed += 1;
    } else {
        checks.push(DoctorCheck {
            scope: ws_scope.into(),
            check: "git-config-drift".into(),
            status: CheckStatus::Warn,
            message: format!(
                "{} git config value{} drifted across {} repo{}",
                total_keys,
                if total_keys == 1 { "" } else { "s" },
                repo_count,
                if repo_count == 1 { "" } else { "s" },
            ),
            fixable: true,
            details: Some(serde_json::json!({ "drifted": all_drifted })),
        });
        eprintln!(
            "  ⚠ {} git config value{} drifted across {} repo{}",
            total_keys,
            if total_keys == 1 { "" } else { "s" },
            repo_count,
            if repo_count == 1 { "" } else { "s" },
        );
    }
}

/// W15. Unapproved setup commands — repo has setup_commands that haven't been approved.
///
/// Resolves commands from all layers (registry, template, repo, workspace) and
/// checks whether the merged list is approved.
///
/// With `--fix`: invokes the interactive approval flow. In non-interactive
/// environments (no tty) the check remains fixable but the fix is skipped
/// with a hint to run `wsp repo setup` manually.
#[allow(clippy::too_many_arguments)]
fn check_unapproved_setup_commands(
    clone_dir: &std::path::Path,
    identity: &str,
    dir_name: &str,
    scope: &str,
    paths: &wsp_core::config::Paths,
    cfg: &config::Config,
    meta: &workspace::Metadata,
    fix: bool,
    checks: &mut Vec<DoctorCheck>,
    fixed: &mut usize,
) {
    let resolved = wsp_core::setup_commands::resolve_for_repo(
        cfg,
        None, // no template context in doctor
        Some(meta),
        identity,
        Some(clone_dir),
    )
    .dedup();
    if resolved.is_empty() {
        return; // no setup_commands from any layer
    }

    let hash = wsp_core::approvals::commands_hash(&resolved.commands);
    let store = match wsp_core::approvals::load(paths.data_dir()) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("  warning: loading approvals store: {}", e);
            return;
        }
    };

    if wsp_core::approvals::is_approved(&store, identity, &hash) {
        return; // already approved — not a problem
    }

    if fix {
        match wsp_core::setup_runner::prompt_and_run_resolved_setup(
            paths.data_dir(),
            clone_dir,
            identity,
            &resolved,
        ) {
            Ok(true) => {
                *fixed += 1;
                checks.push(DoctorCheck {
                    scope: scope.into(),
                    check: "setup-commands-approved".into(),
                    status: CheckStatus::Ok,
                    message: format!("{}: setup commands approved and run", dir_name),
                    fixable: false,
                    details: None,
                });
                eprintln!("  ✓ {}: setup commands approved and run", dir_name);
                return;
            }
            Ok(false) => {
                // User declined or non-interactive — fall through to warn
            }
            Err(e) => {
                eprintln!("  warning: {}: setup failed: {}", dir_name, e);
                // Fall through to warn
            }
        }
    }

    checks.push(DoctorCheck {
        scope: scope.into(),
        check: "setup-commands-approved".into(),
        status: CheckStatus::Warn,
        message: format!("{}: has unapproved setup commands", dir_name),
        fixable: true,
        details: None,
    });
    eprintln!(
        "  ⚠ {}: has unapproved setup commands (run `wsp repo setup` or `wsp doctor --fix`)",
        dir_name
    );
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn dir_size(path: &std::path::Path) -> u64 {
    let mut total = 0u64;
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            let ft = match entry.file_type() {
                Ok(ft) => ft,
                Err(_) => continue,
            };
            if ft.is_dir() {
                total += dir_size(&entry.path());
            } else {
                total += entry.metadata().map(|m| m.len()).unwrap_or(0);
            }
        }
    }
    total
}

fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} bytes", bytes)
    }
}

fn build_output(checks: Vec<DoctorCheck>, fixed: usize) -> DoctorOutput {
    let total = checks.len();
    let ok_count = checks
        .iter()
        .filter(|c| c.status == CheckStatus::Ok)
        .count();
    let warn_count = checks
        .iter()
        .filter(|c| c.status == CheckStatus::Warn)
        .count();
    let error_count = checks
        .iter()
        .filter(|c| c.status == CheckStatus::Error)
        .count();
    let ok = warn_count == 0 && error_count == 0;

    DoctorOutput {
        ok,
        checks,
        summary: DoctorSummary {
            total,
            ok: ok_count,
            warn: warn_count,
            error: error_count,
            fixed,
        },
    }
}

/// Compare two git URLs for equivalence. Handles SSH vs HTTPS for the same repo.
/// Falls back to string comparison if parsing fails.
fn urls_equivalent(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    // Both parse to same identity → equivalent
    let pa = giturl::parse(a);
    let pb = giturl::parse(b);
    match (pa, pb) {
        (Ok(a), Ok(b)) => a.identity() == b.identity(),
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Exit code
// ---------------------------------------------------------------------------

/// Returns the appropriate exit code: 0=ok, 1=any problems found.
pub fn exit_code(output: &DoctorOutput) -> i32 {
    if output.summary.error > 0 || output.summary.warn > 0 {
        1
    } else {
        0
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command as StdCommand;

    /// Create a minimal git repo at `dir` with one commit on `main`.
    fn init_git_repo(dir: &std::path::Path) {
        for args in &[
            vec!["git", "init", "--initial-branch=main"],
            vec!["git", "config", "user.email", "test@test.com"],
            vec!["git", "config", "user.name", "Test"],
            vec!["git", "config", "commit.gpgsign", "false"],
            vec!["git", "commit", "--allow-empty", "-m", "initial"],
        ] {
            let out = StdCommand::new(args[0])
                .args(&args[1..])
                .current_dir(dir)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{:?}: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }

    /// Create a workspace dir with .wsp.yaml metadata written to disk.
    fn create_workspace_on_disk(ws_dir: &std::path::Path, meta: &workspace::Metadata) {
        fs::create_dir_all(ws_dir).unwrap();
        workspace::save_metadata(ws_dir, meta).unwrap();
    }

    /// Build a Metadata with sensible defaults. Repos/dirs can be customized.
    fn test_metadata(
        name: &str,
        branch: &str,
        repos: std::collections::BTreeMap<String, Option<workspace::WorkspaceRepoRef>>,
    ) -> workspace::Metadata {
        workspace::Metadata {
            version: 0,
            name: name.into(),
            branch: branch.into(),
            repos,
            created: chrono::Utc::now(),
            description: None,
            last_used: None,
            created_from: None,
            dirs: std::collections::BTreeMap::new(),
            config: None,
            setup_commands: std::collections::BTreeMap::new(),
        }
    }

    /// Build Paths rooted under `tmp`. Does NOT create any directories — callers
    /// must `fs::create_dir_all` for whichever dirs their test needs.
    fn test_paths(tmp: &std::path::Path) -> Paths {
        Paths {
            config_path: tmp.join("config.yaml"),
            mirrors_dir: tmp.join("mirrors"),
            gc_dir: tmp.join("gc"),
            templates_dir: tmp.join("templates"),
            workspaces_dir: tmp.join("workspaces"),
        }
    }

    // -----------------------------------------------------------------------
    // URL equivalence
    // -----------------------------------------------------------------------

    #[test]
    fn urls_equivalent_same_string() {
        assert!(urls_equivalent(
            "git@github.com:acme/repo.git",
            "git@github.com:acme/repo.git"
        ));
    }

    #[test]
    fn urls_equivalent_ssh_vs_https() {
        assert!(urls_equivalent(
            "git@github.com:acme/repo.git",
            "https://github.com/acme/repo"
        ));
    }

    #[test]
    fn urls_equivalent_different_repos() {
        assert!(!urls_equivalent(
            "git@github.com:acme/repo-a.git",
            "git@github.com:acme/repo-b.git"
        ));
    }

    #[test]
    fn build_output_counts() {
        let checks = vec![
            DoctorCheck {
                scope: "global".into(),
                check: "config-parseable".into(),
                status: CheckStatus::Ok,
                message: "ok".into(),
                fixable: false,
                details: None,
            },
            DoctorCheck {
                scope: "ws/foo".into(),
                check: "origin-url-match".into(),
                status: CheckStatus::Warn,
                message: "mismatch".into(),
                fixable: true,
                details: None,
            },
            DoctorCheck {
                scope: "ws/foo".into(),
                check: "repo-dir-exists".into(),
                status: CheckStatus::Error,
                message: "missing".into(),
                fixable: false,
                details: None,
            },
        ];
        let output = build_output(checks, 0);
        assert!(!output.ok);
        assert_eq!(output.summary.total, 3);
        assert_eq!(output.summary.ok, 1);
        assert_eq!(output.summary.warn, 1);
        assert_eq!(output.summary.error, 1);
        assert_eq!(output.summary.fixed, 0);
    }

    #[test]
    fn all_ok_output() {
        let checks = vec![DoctorCheck {
            scope: "global".into(),
            check: "config-parseable".into(),
            status: CheckStatus::Ok,
            message: "ok".into(),
            fixable: false,
            details: None,
        }];
        let output = build_output(checks, 0);
        assert!(output.ok);
    }

    #[test]
    fn exit_code_all_ok() {
        let output = build_output(
            vec![DoctorCheck {
                scope: "global".into(),
                check: "test".into(),
                status: CheckStatus::Ok,
                message: "ok".into(),
                fixable: false,
                details: None,
            }],
            0,
        );
        assert_eq!(exit_code(&output), 0);
    }

    #[test]
    fn exit_code_warnings() {
        let output = build_output(
            vec![DoctorCheck {
                scope: "global".into(),
                check: "test".into(),
                status: CheckStatus::Warn,
                message: "warn".into(),
                fixable: true,
                details: None,
            }],
            0,
        );
        assert_eq!(exit_code(&output), 1);
    }

    #[test]
    fn exit_code_errors() {
        let output = build_output(
            vec![DoctorCheck {
                scope: "global".into(),
                check: "test".into(),
                status: CheckStatus::Error,
                message: "err".into(),
                fixable: false,
                details: None,
            }],
            0,
        );
        assert_eq!(exit_code(&output), 1);
    }

    #[test]
    fn json_serialization() {
        let output = build_output(
            vec![DoctorCheck {
                scope: "global".into(),
                check: "config-parseable".into(),
                status: CheckStatus::Ok,
                message: "config is valid".into(),
                fixable: false,
                details: None,
            }],
            0,
        );
        let json = serde_json::to_string_pretty(&output).unwrap();
        assert!(json.contains("\"ok\": true"));
        assert!(json.contains("\"status\": \"ok\""));
        assert!(!json.contains("\"fixable\"")); // skip_serializing_if = false
        assert!(!json.contains("\"details\"")); // skip_serializing_if = None
    }

    #[test]
    fn json_with_details() {
        let output = build_output(
            vec![DoctorCheck {
                scope: "workspace/foo/bar".into(),
                check: "origin-url-match".into(),
                status: CheckStatus::Warn,
                message: "mismatch".into(),
                fixable: true,
                details: Some(serde_json::json!({
                    "clone_url": "git@github.com:acme/bar.git",
                    "registered_url": "https://github.com/acme/bar",
                })),
            }],
            0,
        );
        let json = serde_json::to_string_pretty(&output).unwrap();
        assert!(json.contains("\"fixable\": true"));
        assert!(json.contains("\"clone_url\""));
        assert!(json.contains("\"registered_url\""));
    }

    #[test]
    fn orphaned_mirrors_detection() {
        let tmp = tempfile::tempdir().unwrap();
        let mirrors_dir = tmp.path().join("mirrors");
        let cfg = config::Config {
            repos: std::collections::BTreeMap::from([(
                "github.com/acme/kept".to_string(),
                config::RepoEntry {
                    url: "git@github.com:acme/kept.git".into(),
                    added: chrono::Utc::now(),
                    setup_commands: None,
                },
            )]),
            ..Default::default()
        };

        // Create a mirror that's in config
        let kept_dir = mirrors_dir.join("github.com/acme/kept.git");
        fs::create_dir_all(&kept_dir).unwrap();

        // Create a mirror that's orphaned
        let orphan_dir = mirrors_dir.join("github.com/acme/orphan.git");
        fs::create_dir_all(&orphan_dir).unwrap();

        let paths = Paths {
            config_path: tmp.path().join("config.yaml"),
            mirrors_dir,
            gc_dir: tmp.path().join("gc"),
            templates_dir: tmp.path().join("templates"),
            workspaces_dir: tmp.path().join("workspaces"),
        };

        let mut checks = Vec::new();
        let mut fixed = 0;
        check_orphaned_mirrors(&paths, &cfg, false, &mut checks, &mut fixed);

        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].check, "orphaned-mirrors");
        assert_eq!(checks[0].status, CheckStatus::Warn);
        assert!(checks[0].message.contains("orphan"));
    }

    #[test]
    fn orphaned_mirrors_none() {
        let tmp = tempfile::tempdir().unwrap();
        let mirrors_dir = tmp.path().join("mirrors");
        let cfg = config::Config {
            repos: std::collections::BTreeMap::from([(
                "github.com/acme/repo".to_string(),
                config::RepoEntry {
                    url: "git@github.com:acme/repo.git".into(),
                    added: chrono::Utc::now(),
                    setup_commands: None,
                },
            )]),
            ..Default::default()
        };

        // Only create a mirror that's in config
        let kept_dir = mirrors_dir.join("github.com/acme/repo.git");
        fs::create_dir_all(&kept_dir).unwrap();

        let paths = Paths {
            config_path: tmp.path().join("config.yaml"),
            mirrors_dir,
            gc_dir: tmp.path().join("gc"),
            templates_dir: tmp.path().join("templates"),
            workspaces_dir: tmp.path().join("workspaces"),
        };

        let mut checks = Vec::new();
        let mut fixed = 0;
        check_orphaned_mirrors(&paths, &cfg, false, &mut checks, &mut fixed);

        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].status, CheckStatus::Ok);
    }

    #[test]
    fn orphaned_mirrors_fix() {
        let tmp = tempfile::tempdir().unwrap();
        let mirrors_dir = tmp.path().join("mirrors");
        let cfg = config::Config::default();

        // Create an orphaned mirror
        let orphan_dir = mirrors_dir.join("github.com/acme/orphan.git");
        fs::create_dir_all(&orphan_dir).unwrap();

        let paths = Paths {
            config_path: tmp.path().join("config.yaml"),
            mirrors_dir: mirrors_dir.clone(),
            gc_dir: tmp.path().join("gc"),
            templates_dir: tmp.path().join("templates"),
            workspaces_dir: tmp.path().join("workspaces"),
        };

        let mut checks = Vec::new();
        let mut fixed = 0;
        check_orphaned_mirrors(&paths, &cfg, true, &mut checks, &mut fixed);

        assert_eq!(fixed, 1);
        assert!(!orphan_dir.exists());
    }

    #[test]
    fn gc_stale_entries_none() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths {
            config_path: tmp.path().join("config.yaml"),
            mirrors_dir: tmp.path().join("mirrors"),
            gc_dir: tmp.path().join("gc"),
            templates_dir: tmp.path().join("templates"),
            workspaces_dir: tmp.path().join("workspaces"),
        };
        let cfg = config::Config::default();

        let mut checks = Vec::new();
        let mut fixed = 0;
        check_gc_stale_entries(&paths, &cfg, false, &mut checks, &mut fixed);

        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].status, CheckStatus::Ok);
    }

    /// Set up a workspace whose `unknown` repo is absent from the registry but
    /// whose clone has a usable `origin`. Pre-creates the mirror directory so
    /// the fix path takes the "mirror already exists" branch and never reaches
    /// the network.
    fn setup_unregistered_repo_with_origin(
        tmp: &std::path::Path,
        origin_url: &str,
    ) -> (
        std::path::PathBuf,
        workspace::Metadata,
        config::Config,
        Paths,
    ) {
        let ws_dir = tmp.join("ws");
        let meta = test_metadata(
            "test",
            "test/branch",
            std::collections::BTreeMap::from([("github.com/acme/unknown".into(), None)]),
        );
        create_workspace_on_disk(&ws_dir, &meta);

        let clone_dir = ws_dir.join(meta.dir_name("github.com/acme/unknown").unwrap());
        fs::create_dir_all(&clone_dir).unwrap();
        init_git_repo(&clone_dir);
        git::run(Some(&clone_dir), &["remote", "add", "origin", origin_url]).unwrap();

        let paths = test_paths(tmp);
        if let Ok(parsed) = giturl::parse(origin_url) {
            fs::create_dir_all(mirror::dir(&paths.mirrors_dir, &parsed)).unwrap();
        }

        // Registry is empty, so the repo is unregistered.
        (ws_dir, meta, config::Config::default(), paths)
    }

    /// Issue #65: `doctor --fix` should register a workspace repo that is
    /// missing from the registry, using its clone's origin URL.
    #[test]
    fn unregistered_repos_fix_registers_from_origin_url() {
        let tmp = tempfile::tempdir().unwrap();
        let origin_url = "git@test.local:acme/unknown.git";
        let (ws_dir, meta, cfg, paths) =
            setup_unregistered_repo_with_origin(tmp.path(), origin_url);

        let mut checks = Vec::new();
        let mut fixed = 0;
        check_unregistered_repos(
            &ws_dir,
            &meta,
            &cfg,
            &paths,
            "workspace/test",
            true,
            &mut checks,
            &mut fixed,
        );

        assert_eq!(fixed, 1, "the fix should be counted");
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].status, CheckStatus::Ok);
        assert!(
            checks[0].message.contains("registered 1"),
            "unexpected message: {}",
            checks[0].message
        );

        // The real proof is the persisted registry, not the in-memory report:
        // the entry must exist and carry the clone's origin URL verbatim.
        let saved = config::Config::load_from(&paths.config_path).unwrap();
        let entry = saved
            .repos
            .get("github.com/acme/unknown")
            .expect("repo should have been registered");
        assert_eq!(entry.url, origin_url);
    }

    /// Guards the other half of the behavior: with no origin to read, there is
    /// nothing to register, so the check must stay a Warn rather than inventing
    /// an entry or reporting a fix it did not make.
    #[test]
    fn unregistered_repos_fix_is_noop_without_origin() {
        let tmp = tempfile::tempdir().unwrap();
        let ws_dir = tmp.path().join("ws");
        let meta = test_metadata(
            "test",
            "test/branch",
            std::collections::BTreeMap::from([("github.com/acme/unknown".into(), None)]),
        );
        create_workspace_on_disk(&ws_dir, &meta);
        let clone_dir = ws_dir.join(meta.dir_name("github.com/acme/unknown").unwrap());
        fs::create_dir_all(&clone_dir).unwrap();
        init_git_repo(&clone_dir); // deliberately no origin remote
        let paths = test_paths(tmp.path());

        let mut checks = Vec::new();
        let mut fixed = 0;
        check_unregistered_repos(
            &ws_dir,
            &meta,
            &config::Config::default(),
            &paths,
            "workspace/test",
            true,
            &mut checks,
            &mut fixed,
        );

        assert_eq!(fixed, 0, "nothing was registered, so nothing was fixed");
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].status, CheckStatus::Warn);
        assert!(
            config::Config::load_from(&paths.config_path)
                .map(|c| c.repos.is_empty())
                .unwrap_or(true),
            "registry must stay empty when there is no origin to read"
        );
    }

    #[test]
    fn unregistered_repos_detected() {
        let tmp = tempfile::tempdir().unwrap();
        let ws_dir = tmp.path().join("ws");
        fs::create_dir_all(&ws_dir).unwrap();
        let paths = test_paths(tmp.path());

        let meta = workspace::Metadata {
            version: 0,
            name: "test".into(),
            branch: "test/branch".into(),
            repos: std::collections::BTreeMap::from([
                ("github.com/acme/known".into(), None),
                ("github.com/acme/unknown".into(), None),
            ]),
            created: chrono::Utc::now(),
            description: None,
            last_used: None,
            created_from: None,
            dirs: std::collections::BTreeMap::new(),
            config: None,
            setup_commands: std::collections::BTreeMap::new(),
        };
        let cfg = config::Config {
            repos: std::collections::BTreeMap::from([(
                "github.com/acme/known".to_string(),
                config::RepoEntry {
                    url: "git@github.com:acme/known.git".into(),
                    added: chrono::Utc::now(),
                    setup_commands: None,
                },
            )]),
            ..Default::default()
        };

        let mut checks = Vec::new();
        let mut fixed = 0;
        check_unregistered_repos(
            &ws_dir,
            &meta,
            &cfg,
            &paths,
            "workspace/test",
            false,
            &mut checks,
            &mut fixed,
        );

        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].check, "unregistered-repos");
        assert_eq!(checks[0].status, CheckStatus::Warn);
        assert!(checks[0].fixable);
    }

    #[test]
    fn legacy_ref_field_detected() {
        let meta = workspace::Metadata {
            version: 0,
            name: "test".into(),
            branch: "test/branch".into(),
            repos: std::collections::BTreeMap::from([(
                "github.com/acme/repo".into(),
                Some(workspace::WorkspaceRepoRef {
                    r#ref: "v1.0".into(),
                    url: None,
                }),
            )]),
            created: chrono::Utc::now(),
            description: None,
            last_used: None,
            created_from: None,
            dirs: std::collections::BTreeMap::new(),
            config: None,
            setup_commands: std::collections::BTreeMap::new(),
        };

        let mut checks = Vec::new();
        let mut fixed = 0;
        // Can't easily test fix without a real workspace dir, so test detection only
        check_legacy_ref_field(
            std::path::Path::new("/nonexistent"),
            &meta,
            "workspace/test",
            false,
            &mut checks,
            &mut fixed,
        );

        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].check, "legacy-ref-field");
        assert_eq!(checks[0].status, CheckStatus::Warn);
    }

    #[test]
    fn legacy_ref_field_clean() {
        let meta = workspace::Metadata {
            version: 0,
            name: "test".into(),
            branch: "test/branch".into(),
            repos: std::collections::BTreeMap::from([
                ("github.com/acme/repo".into(), None),
                (
                    "github.com/acme/repo2".into(),
                    Some(workspace::WorkspaceRepoRef {
                        r#ref: String::new(),
                        url: None,
                    }),
                ),
            ]),
            created: chrono::Utc::now(),
            description: None,
            last_used: None,
            created_from: None,
            dirs: std::collections::BTreeMap::new(),
            config: None,
            setup_commands: std::collections::BTreeMap::new(),
        };

        let mut checks = Vec::new();
        let mut fixed = 0;
        check_legacy_ref_field(
            std::path::Path::new("/nonexistent"),
            &meta,
            "workspace/test",
            false,
            &mut checks,
            &mut fixed,
        );

        // No stale refs → no check emitted
        assert!(checks.is_empty());
    }

    #[test]
    fn stale_dirs_map_detected() {
        let meta = workspace::Metadata {
            version: 0,
            name: "test".into(),
            branch: "test/branch".into(),
            repos: std::collections::BTreeMap::from([("github.com/acme/repo".into(), None)]),
            created: chrono::Utc::now(),
            description: None,
            last_used: None,
            created_from: None,
            dirs: std::collections::BTreeMap::from([
                ("github.com/acme/repo".into(), "repo".into()),
                ("github.com/acme/removed".into(), "removed".into()),
            ]),
            config: None,
            setup_commands: std::collections::BTreeMap::new(),
        };

        let mut checks = Vec::new();
        let mut fixed = 0;
        check_stale_dirs_map(
            std::path::Path::new("/nonexistent"),
            &meta,
            "workspace/test",
            false,
            &mut checks,
            &mut fixed,
        );

        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].check, "stale-dirs-map");
        assert_eq!(checks[0].status, CheckStatus::Warn);
    }

    #[test]
    fn stale_dirs_map_clean() {
        let meta = workspace::Metadata {
            version: 0,
            name: "test".into(),
            branch: "test/branch".into(),
            repos: std::collections::BTreeMap::from([("github.com/acme/repo".into(), None)]),
            created: chrono::Utc::now(),
            description: None,
            last_used: None,
            created_from: None,
            dirs: std::collections::BTreeMap::from([(
                "github.com/acme/repo".into(),
                "repo".into(),
            )]),
            config: None,
            setup_commands: std::collections::BTreeMap::new(),
        };

        let mut checks = Vec::new();
        let mut fixed = 0;
        check_stale_dirs_map(
            std::path::Path::new("/nonexistent"),
            &meta,
            "workspace/test",
            false,
            &mut checks,
            &mut fixed,
        );

        assert!(checks.is_empty());
    }

    // -----------------------------------------------------------------------
    // G2. config-version
    // -----------------------------------------------------------------------

    #[test]
    fn config_version_ok() {
        let cfg = config::Config {
            version: config::CURRENT_CONFIG_VERSION,
            ..Default::default()
        };
        let mut checks = Vec::new();
        check_config_version(&cfg, &mut checks);
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].check, "config-version");
        assert_eq!(checks[0].status, CheckStatus::Ok);
    }

    #[test]
    fn config_version_skew() {
        let cfg = config::Config {
            version: config::CURRENT_CONFIG_VERSION + 1,
            ..Default::default()
        };
        let mut checks = Vec::new();
        check_config_version(&cfg, &mut checks);
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].check, "config-version");
        assert_eq!(checks[0].status, CheckStatus::Warn);
        assert!(checks[0].message.contains("newer than supported"));
    }

    // -----------------------------------------------------------------------
    // W1. metadata-version
    // -----------------------------------------------------------------------

    #[test]
    fn metadata_version_ok() {
        let meta = test_metadata("test", "test/branch", std::collections::BTreeMap::new());
        let mut checks = Vec::new();
        check_metadata_version(&meta, "workspace/test", &mut checks);
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].check, "metadata-version");
        assert_eq!(checks[0].status, CheckStatus::Ok);
    }

    #[test]
    fn metadata_version_skew() {
        let mut meta = test_metadata("test", "test/branch", std::collections::BTreeMap::new());
        meta.version = workspace::CURRENT_METADATA_VERSION + 1;
        let mut checks = Vec::new();
        check_metadata_version(&meta, "workspace/test", &mut checks);
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].check, "metadata-version");
        assert_eq!(checks[0].status, CheckStatus::Warn);
        assert!(checks[0].message.contains("newer than supported"));
    }

    // -----------------------------------------------------------------------
    // G4. gc-stale-entries (with stale data + fix)
    // -----------------------------------------------------------------------

    /// Create a workspace, GC it, and backdate the entry to 10 days ago.
    fn create_stale_gc_entry(paths: &Paths) {
        let ws_dir = paths.workspaces_dir.join("old-ws");
        fs::create_dir_all(&ws_dir).unwrap();
        let meta = test_metadata("old-ws", "test/old-ws", std::collections::BTreeMap::new());
        workspace::save_metadata(&ws_dir, &meta).unwrap();
        gc::move_to_gc(paths, "old-ws", "test/old-ws").unwrap();

        for item in fs::read_dir(&paths.gc_dir).unwrap() {
            let path = item.unwrap().path();
            if !path.is_dir() {
                continue;
            }
            let meta_path = path.join(".wsp-gc.yaml");
            if let Ok(data) = fs::read_to_string(&meta_path) {
                let mut entry: gc::GcEntry = serde_yaml_ng::from_str(&data).unwrap();
                entry.trashed_at = chrono::Utc::now() - chrono::Duration::days(10);
                fs::write(&meta_path, serde_yaml_ng::to_string(&entry).unwrap()).unwrap();
            }
        }
    }

    #[test]
    fn gc_stale_entries_detected() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = test_paths(tmp.path());
        let cfg = config::Config::default();
        create_stale_gc_entry(&paths);

        let mut checks = Vec::new();
        let mut fixed = 0;
        check_gc_stale_entries(&paths, &cfg, false, &mut checks, &mut fixed);

        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].check, "gc-stale-entries");
        assert_eq!(checks[0].status, CheckStatus::Warn);
        assert!(checks[0].fixable);
    }

    #[test]
    fn gc_stale_entries_fix() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = test_paths(tmp.path());
        let cfg = config::Config::default();
        create_stale_gc_entry(&paths);

        let mut checks = Vec::new();
        let mut fixed = 0;
        check_gc_stale_entries(&paths, &cfg, true, &mut checks, &mut fixed);

        assert_eq!(fixed, 1);
        assert_eq!(checks[0].status, CheckStatus::Ok);
        assert!(checks[0].message.contains("purged"));
    }

    // -----------------------------------------------------------------------
    // W2. legacy-wsp-mirror-remote (detect + fix)
    // -----------------------------------------------------------------------

    #[test]
    fn legacy_wsp_mirror_detected() {
        let tmp = tempfile::tempdir().unwrap();
        let clone_dir = tmp.path().join("repo");
        fs::create_dir_all(&clone_dir).unwrap();
        init_git_repo(&clone_dir);

        // Add a wsp-mirror remote
        git::run(
            Some(&clone_dir),
            &[
                "remote",
                "add",
                "wsp-mirror",
                "https://example.com/mirror.git",
            ],
        )
        .unwrap();

        let mut checks = Vec::new();
        let mut fixed = 0;
        check_legacy_wsp_mirror(
            &clone_dir,
            "repo",
            "workspace/test/repo",
            false,
            &mut checks,
            &mut fixed,
        );

        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].check, "legacy-wsp-mirror-remote");
        assert_eq!(checks[0].status, CheckStatus::Warn);
        assert!(checks[0].fixable);
    }

    #[test]
    fn legacy_wsp_mirror_fix() {
        let tmp = tempfile::tempdir().unwrap();
        let clone_dir = tmp.path().join("repo");
        fs::create_dir_all(&clone_dir).unwrap();
        init_git_repo(&clone_dir);

        git::run(
            Some(&clone_dir),
            &[
                "remote",
                "add",
                "wsp-mirror",
                "https://example.com/mirror.git",
            ],
        )
        .unwrap();
        assert!(git::has_remote(&clone_dir, "wsp-mirror"));

        let mut checks = Vec::new();
        let mut fixed = 0;
        check_legacy_wsp_mirror(
            &clone_dir,
            "repo",
            "workspace/test/repo",
            true,
            &mut checks,
            &mut fixed,
        );

        assert_eq!(fixed, 1);
        assert_eq!(checks[0].status, CheckStatus::Ok);
        assert!(!git::has_remote(&clone_dir, "wsp-mirror"));
    }

    #[test]
    fn legacy_wsp_mirror_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let clone_dir = tmp.path().join("repo");
        fs::create_dir_all(&clone_dir).unwrap();
        init_git_repo(&clone_dir);

        let mut checks = Vec::new();
        let mut fixed = 0;
        check_legacy_wsp_mirror(
            &clone_dir,
            "repo",
            "workspace/test/repo",
            false,
            &mut checks,
            &mut fixed,
        );

        // No wsp-mirror → no check emitted
        assert!(checks.is_empty());
    }

    // -----------------------------------------------------------------------
    // W7. in-progress-git-op
    // -----------------------------------------------------------------------

    #[test]
    fn in_progress_op_rebase_detected() {
        let (clone_dir, source, _ct, _st) = wsp_core::testutil::setup_clone_repo();

        // Create a conflict to leave rebase in progress
        wsp_core::testutil::local_commit(&clone_dir, "conflict.txt", "local");
        // Push a conflicting change to origin
        let out = StdCommand::new("git")
            .args(["checkout", "main"])
            .current_dir(&source)
            .output()
            .unwrap();
        assert!(out.status.success());
        std::fs::write(source.join("conflict.txt"), "upstream").unwrap();
        for args in &[
            vec!["git", "add", "conflict.txt"],
            vec!["git", "commit", "-m", "upstream conflict"],
        ] {
            let out = StdCommand::new(args[0])
                .args(&args[1..])
                .current_dir(&source)
                .output()
                .unwrap();
            assert!(out.status.success());
        }
        git::fetch_from_path(
            &clone_dir,
            &source,
            "+refs/heads/*:refs/remotes/origin/*",
            false,
        )
        .unwrap();

        // Start rebase that will conflict (don't use rebase_onto which auto-aborts)
        let out = StdCommand::new("git")
            .args(["rebase", "origin/main"])
            .current_dir(&clone_dir)
            .output()
            .unwrap();
        assert!(!out.status.success());

        let mut checks = Vec::new();
        check_in_progress_op(&clone_dir, "repo", "workspace/test/repo", &mut checks);

        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].check, "in-progress-git-op");
        assert_eq!(checks[0].status, CheckStatus::Warn);
        assert!(checks[0].message.contains("rebase"));

        // Clean up
        let _ = git::run(Some(&clone_dir), &["rebase", "--abort"]);
    }

    #[test]
    fn in_progress_op_merge_detected() {
        let (clone_dir, source, _ct, _st) = wsp_core::testutil::setup_clone_repo();

        wsp_core::testutil::local_commit(&clone_dir, "conflict.txt", "local");
        let out = StdCommand::new("git")
            .args(["checkout", "main"])
            .current_dir(&source)
            .output()
            .unwrap();
        assert!(out.status.success());
        std::fs::write(source.join("conflict.txt"), "upstream").unwrap();
        for args in &[
            vec!["git", "add", "conflict.txt"],
            vec!["git", "commit", "-m", "upstream conflict"],
        ] {
            let out = StdCommand::new(args[0])
                .args(&args[1..])
                .current_dir(&source)
                .output()
                .unwrap();
            assert!(out.status.success());
        }
        git::fetch_from_path(
            &clone_dir,
            &source,
            "+refs/heads/*:refs/remotes/origin/*",
            false,
        )
        .unwrap();

        let out = StdCommand::new("git")
            .args(["merge", "origin/main"])
            .current_dir(&clone_dir)
            .output()
            .unwrap();
        assert!(!out.status.success());

        let mut checks = Vec::new();
        check_in_progress_op(&clone_dir, "repo", "workspace/test/repo", &mut checks);

        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].check, "in-progress-git-op");
        assert!(checks[0].message.contains("merge"));

        let _ = git::run(Some(&clone_dir), &["merge", "--abort"]);
    }

    #[test]
    fn in_progress_op_clean() {
        let (clone_dir, _source, _ct, _st) = wsp_core::testutil::setup_clone_repo();

        let mut checks = Vec::new();
        check_in_progress_op(&clone_dir, "repo", "workspace/test/repo", &mut checks);

        assert!(checks.is_empty());
    }

    // -----------------------------------------------------------------------
    // W3. legacy-ref-field (fix path)
    // -----------------------------------------------------------------------

    #[test]
    fn legacy_ref_field_fix() {
        let tmp = tempfile::tempdir().unwrap();
        let ws_dir = tmp.path().join("ws");
        let meta = workspace::Metadata {
            version: 0,
            name: "test".into(),
            branch: "test/branch".into(),
            repos: std::collections::BTreeMap::from([
                (
                    "github.com/acme/repo1".into(),
                    Some(workspace::WorkspaceRepoRef {
                        r#ref: "v1.0".into(),
                        url: None,
                    }),
                ),
                (
                    "github.com/acme/repo2".into(),
                    Some(workspace::WorkspaceRepoRef {
                        r#ref: "main".into(),
                        url: None,
                    }),
                ),
            ]),
            created: chrono::Utc::now(),
            description: None,
            last_used: None,
            created_from: None,
            dirs: std::collections::BTreeMap::new(),
            config: None,
            setup_commands: std::collections::BTreeMap::new(),
        };
        create_workspace_on_disk(&ws_dir, &meta);

        let mut checks = Vec::new();
        let mut fixed = 0;
        check_legacy_ref_field(
            &ws_dir,
            &meta,
            "workspace/test",
            true,
            &mut checks,
            &mut fixed,
        );

        assert_eq!(fixed, 1);
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].status, CheckStatus::Ok);
        assert!(checks[0].message.contains("cleared 2 stale ref values"));

        // Verify the fix persisted to disk
        let reloaded = workspace::load_metadata(&ws_dir).unwrap();
        for (_, ref_opt) in &reloaded.repos {
            if let Some(repo_ref) = ref_opt {
                assert!(repo_ref.r#ref.is_empty(), "ref should be cleared");
            }
        }
    }

    // -----------------------------------------------------------------------
    // W4. stale-dirs-map (fix path)
    // -----------------------------------------------------------------------

    #[test]
    fn stale_dirs_map_fix() {
        let tmp = tempfile::tempdir().unwrap();
        let ws_dir = tmp.path().join("ws");
        let meta = workspace::Metadata {
            version: 0,
            name: "test".into(),
            branch: "test/branch".into(),
            repos: std::collections::BTreeMap::from([("github.com/acme/repo".into(), None)]),
            created: chrono::Utc::now(),
            description: None,
            last_used: None,
            created_from: None,
            dirs: std::collections::BTreeMap::from([
                ("github.com/acme/repo".into(), "repo".into()),
                ("github.com/acme/removed".into(), "removed".into()),
            ]),
            config: None,
            setup_commands: std::collections::BTreeMap::new(),
        };
        create_workspace_on_disk(&ws_dir, &meta);

        let mut checks = Vec::new();
        let mut fixed = 0;
        check_stale_dirs_map(
            &ws_dir,
            &meta,
            "workspace/test",
            true,
            &mut checks,
            &mut fixed,
        );

        assert_eq!(fixed, 1);
        assert_eq!(checks[0].status, CheckStatus::Ok);
        assert!(checks[0].message.contains("removed 1 stale dirs entries"));

        // Verify the fix persisted
        let reloaded = workspace::load_metadata(&ws_dir).unwrap();
        assert_eq!(reloaded.dirs.len(), 1);
        assert!(reloaded.dirs.contains_key("github.com/acme/repo"));
        assert!(!reloaded.dirs.contains_key("github.com/acme/removed"));
    }

    // -----------------------------------------------------------------------
    // W9. agents-md-valid (detect + fix)
    // -----------------------------------------------------------------------

    #[test]
    fn agents_md_valid_ok() {
        let tmp = tempfile::tempdir().unwrap();
        let ws_dir = tmp.path().join("ws");
        let meta = test_metadata("test", "test/branch", std::collections::BTreeMap::new());
        create_workspace_on_disk(&ws_dir, &meta);

        // Create a valid AGENTS.md with markers
        agentmd::update(&ws_dir, &meta).unwrap();

        // On Windows without Developer Mode, symlink creation is silently skipped
        // (os error 1314). The doctor check requires the symlink, so skip if absent.
        if !ws_dir.join("CLAUDE.md").exists() {
            return;
        }

        let mut checks = Vec::new();
        let mut fixed = 0;
        check_agents_md_valid(
            &ws_dir,
            &meta,
            "workspace/test",
            false,
            &mut checks,
            &mut fixed,
        );

        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].check, "agents-md-valid");
        assert_eq!(checks[0].status, CheckStatus::Ok);
    }

    #[test]
    fn agents_md_missing_markers() {
        let tmp = tempfile::tempdir().unwrap();
        let ws_dir = tmp.path().join("ws");
        let meta = test_metadata("test", "test/branch", std::collections::BTreeMap::new());
        create_workspace_on_disk(&ws_dir, &meta);

        // Write an AGENTS.md without markers
        fs::write(ws_dir.join("AGENTS.md"), "# My Project\nSome notes.\n").unwrap();

        let mut checks = Vec::new();
        let mut fixed = 0;
        check_agents_md_valid(
            &ws_dir,
            &meta,
            "workspace/test",
            false,
            &mut checks,
            &mut fixed,
        );

        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].check, "agents-md-valid");
        assert_eq!(checks[0].status, CheckStatus::Warn);
        assert!(checks[0].fixable);
    }

    #[test]
    fn agents_md_missing_entirely() {
        let tmp = tempfile::tempdir().unwrap();
        let ws_dir = tmp.path().join("ws");
        let meta = test_metadata("test", "test/branch", std::collections::BTreeMap::new());
        create_workspace_on_disk(&ws_dir, &meta);

        let mut checks = Vec::new();
        let mut fixed = 0;
        check_agents_md_valid(
            &ws_dir,
            &meta,
            "workspace/test",
            false,
            &mut checks,
            &mut fixed,
        );

        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].status, CheckStatus::Warn);
    }

    #[test]
    #[cfg(unix)]
    fn agents_md_claude_md_not_symlink() {
        let tmp = tempfile::tempdir().unwrap();
        let ws_dir = tmp.path().join("ws");
        let meta = test_metadata("test", "test/branch", std::collections::BTreeMap::new());
        create_workspace_on_disk(&ws_dir, &meta);

        // Create valid AGENTS.md
        agentmd::update(&ws_dir, &meta).unwrap();
        // Replace CLAUDE.md symlink with a regular file
        let claude_path = ws_dir.join("CLAUDE.md");
        let _ = fs::remove_file(&claude_path);
        fs::write(&claude_path, "not a symlink").unwrap();

        let mut checks = Vec::new();
        let mut fixed = 0;
        check_agents_md_valid(
            &ws_dir,
            &meta,
            "workspace/test",
            false,
            &mut checks,
            &mut fixed,
        );

        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].status, CheckStatus::Warn);
    }

    #[test]
    #[cfg(unix)]
    fn agents_md_fix_regenerates() {
        let tmp = tempfile::tempdir().unwrap();
        let ws_dir = tmp.path().join("ws");
        let meta = test_metadata("test", "test/branch", std::collections::BTreeMap::new());
        create_workspace_on_disk(&ws_dir, &meta);

        // Start with no AGENTS.md or CLAUDE.md
        assert!(!ws_dir.join("AGENTS.md").exists());

        let mut checks = Vec::new();
        let mut fixed = 0;
        check_agents_md_valid(
            &ws_dir,
            &meta,
            "workspace/test",
            true,
            &mut checks,
            &mut fixed,
        );

        assert_eq!(fixed, 1);
        assert_eq!(checks[0].status, CheckStatus::Ok);

        // Verify files were created
        assert!(ws_dir.join("AGENTS.md").exists());
        let content = fs::read_to_string(ws_dir.join("AGENTS.md")).unwrap();
        assert!(content.contains(agentmd::MARKER_BEGIN));
        assert!(content.contains(agentmd::MARKER_END));

        // CLAUDE.md should be a symlink to AGENTS.md
        let claude_meta = fs::symlink_metadata(ws_dir.join("CLAUDE.md")).unwrap();
        assert!(claude_meta.file_type().is_symlink());
        assert_eq!(
            fs::read_link(ws_dir.join("CLAUDE.md")).unwrap(),
            std::path::Path::new("AGENTS.md")
        );
    }

    /// True if this platform/process can create symlinks. On Windows that needs
    /// Developer Mode or elevation; probing at runtime lets the test below run on
    /// Windows CI (elevated) instead of being compiled out like its `cfg(unix)`
    /// siblings.
    fn symlinks_supported(dir: &std::path::Path) -> bool {
        let probe = dir.join(".symlink_probe");
        let ok = wsp_core::symlink::symlink_file_or_skip("AGENTS.md", &probe).unwrap_or(false);
        let _ = fs::remove_file(&probe);
        ok
    }

    /// The fix path must not delete and recreate a CLAUDE.md symlink that is
    /// already correct: on Windows without Developer Mode the delete would
    /// succeed and the recreate would not, destroying a valid link to repair a
    /// problem that lived in AGENTS.md.
    #[test]
    fn agents_md_fix_preserves_valid_claude_md_symlink() {
        let tmp = tempfile::tempdir().unwrap();
        let ws_dir = tmp.path().join("ws");
        let meta = test_metadata("test", "test/branch", std::collections::BTreeMap::new());
        create_workspace_on_disk(&ws_dir, &meta);
        if !symlinks_supported(&ws_dir) {
            return;
        }

        // Valid CLAUDE.md symlink, but AGENTS.md has lost its wsp markers — so
        // there is a real problem to fix that is not the symlink.
        agentmd::update(&ws_dir, &meta).unwrap();
        fs::write(ws_dir.join("AGENTS.md"), "no markers here").unwrap();
        let claude_path = ws_dir.join("CLAUDE.md");
        assert!(
            fs::symlink_metadata(&claude_path)
                .unwrap()
                .file_type()
                .is_symlink()
        );

        let mut checks = Vec::new();
        let mut fixed = 0;
        check_agents_md_valid(
            &ws_dir,
            &meta,
            "workspace/test",
            true,
            &mut checks,
            &mut fixed,
        );

        assert_eq!(fixed, 1);
        assert_eq!(checks[0].status, CheckStatus::Ok);
        // Still a symlink pointing at AGENTS.md, and AGENTS.md was regenerated.
        let claude_meta = fs::symlink_metadata(&claude_path).unwrap();
        assert!(
            claude_meta.file_type().is_symlink(),
            "valid CLAUDE.md symlink must survive the fix"
        );
        assert_eq!(
            fs::read_link(&claude_path).unwrap(),
            std::path::Path::new("AGENTS.md")
        );
        assert!(
            fs::read_to_string(ws_dir.join("AGENTS.md"))
                .unwrap()
                .contains(agentmd::MARKER_BEGIN)
        );
    }

    // -----------------------------------------------------------------------
    // W12. unregistered-repos (all registered → ok)
    // -----------------------------------------------------------------------

    #[test]
    fn unregistered_repos_all_registered() {
        let tmp = tempfile::tempdir().unwrap();
        let ws_dir = tmp.path().join("ws");
        fs::create_dir_all(&ws_dir).unwrap();
        let paths = test_paths(tmp.path());

        let meta = test_metadata(
            "test",
            "test/branch",
            std::collections::BTreeMap::from([("github.com/acme/repo".into(), None)]),
        );
        let cfg = config::Config {
            repos: std::collections::BTreeMap::from([(
                "github.com/acme/repo".to_string(),
                config::RepoEntry {
                    url: "git@github.com:acme/repo.git".into(),
                    added: chrono::Utc::now(),
                    setup_commands: None,
                },
            )]),
            ..Default::default()
        };

        let mut checks = Vec::new();
        let mut fixed = 0;
        check_unregistered_repos(
            &ws_dir,
            &meta,
            &cfg,
            &paths,
            "workspace/test",
            false,
            &mut checks,
            &mut fixed,
        );

        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].status, CheckStatus::Ok);
    }

    // -----------------------------------------------------------------------
    // Orphaned mirrors: symlink guard
    // -----------------------------------------------------------------------

    #[test]
    #[cfg(unix)]
    fn orphaned_mirrors_symlink_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let mirrors_dir = tmp.path().join("mirrors");
        let cfg = config::Config::default();

        // Create a symlink pretending to be a mirror
        let host_dir = mirrors_dir.join("github.com/acme");
        fs::create_dir_all(&host_dir).unwrap();
        std::os::unix::fs::symlink("/tmp", host_dir.join("evil.git")).unwrap();

        let paths = test_paths(tmp.path());
        let paths = Paths {
            mirrors_dir,
            ..paths
        };

        let mut checks = Vec::new();
        let mut fixed = 0;
        check_orphaned_mirrors(&paths, &cfg, true, &mut checks, &mut fixed);

        // Should warn but NOT fix (symlink guard)
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].status, CheckStatus::Warn);
        assert!(checks[0].message.contains("symlink"));
        assert_eq!(fixed, 0);
    }

    // -----------------------------------------------------------------------
    // G3. workspaces-dir-exists
    // -----------------------------------------------------------------------

    #[test]
    fn workspaces_dir_exists_ok() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = test_paths(tmp.path());
        fs::create_dir_all(&paths.workspaces_dir).unwrap();

        let mut checks = Vec::new();
        let mut fixed = 0;
        check_workspaces_dir_exists(&paths, false, &mut checks, &mut fixed);

        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].check, "workspaces-dir-exists");
        assert_eq!(checks[0].status, CheckStatus::Ok);
    }

    #[test]
    fn workspaces_dir_missing_detected() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths {
            workspaces_dir: tmp.path().join("nonexistent"),
            ..test_paths(tmp.path())
        };

        let mut checks = Vec::new();
        let mut fixed = 0;
        check_workspaces_dir_exists(&paths, false, &mut checks, &mut fixed);

        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].status, CheckStatus::Error);
        assert!(checks[0].fixable);
    }

    #[test]
    fn workspaces_dir_missing_fix() {
        let tmp = tempfile::tempdir().unwrap();
        let new_ws_dir = tmp.path().join("new_workspaces");
        let paths = Paths {
            workspaces_dir: new_ws_dir.clone(),
            ..test_paths(tmp.path())
        };

        let mut checks = Vec::new();
        let mut fixed = 0;
        check_workspaces_dir_exists(&paths, true, &mut checks, &mut fixed);

        assert_eq!(fixed, 1);
        assert_eq!(checks[0].status, CheckStatus::Ok);
        assert!(new_ws_dir.exists());
    }

    // -----------------------------------------------------------------------
    // G5. gc-orphaned-entries
    // -----------------------------------------------------------------------

    #[test]
    fn gc_orphaned_entries_none() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = test_paths(tmp.path());
        // Empty gc dir

        let mut checks = Vec::new();
        check_gc_orphaned_entries(&paths, &mut checks);

        // No orphaned → no check emitted
        assert!(checks.is_empty());
    }

    #[test]
    fn gc_orphaned_entries_detected() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = test_paths(tmp.path());

        // Create a dir in gc/ without .wsp-gc.yaml
        let orphan = paths.gc_dir.join("orphan__12345");
        fs::create_dir_all(&orphan).unwrap();

        let mut checks = Vec::new();
        check_gc_orphaned_entries(&paths, &mut checks);

        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].check, "gc-orphaned-entries");
        assert_eq!(checks[0].status, CheckStatus::Warn);
        assert!(checks[0].message.contains("1"));
    }

    #[test]
    fn gc_orphaned_entries_corrupt_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = test_paths(tmp.path());

        // Create a dir with corrupt .wsp-gc.yaml
        let orphan = paths.gc_dir.join("corrupt__12345");
        fs::create_dir_all(&orphan).unwrap();
        fs::write(orphan.join(".wsp-gc.yaml"), "not: valid: gc: entry:").unwrap();

        let mut checks = Vec::new();
        check_gc_orphaned_entries(&paths, &mut checks);

        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].status, CheckStatus::Warn);
    }

    // -----------------------------------------------------------------------
    // G6. gc-disk-usage
    // -----------------------------------------------------------------------

    #[test]
    fn gc_disk_usage_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = test_paths(tmp.path());
        fs::create_dir_all(&paths.gc_dir).unwrap();

        let mut checks = Vec::new();
        check_gc_disk_usage(&paths, &mut checks);

        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].check, "gc-disk-usage");
        assert_eq!(checks[0].status, CheckStatus::Ok);
        assert!(checks[0].message.contains("gc disk usage"));
    }

    #[test]
    fn gc_disk_usage_with_data() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = test_paths(tmp.path());

        // Put some data in gc
        let entry_dir = paths.gc_dir.join("test__12345");
        fs::create_dir_all(&entry_dir).unwrap();
        fs::write(entry_dir.join("data.bin"), vec![0u8; 2048]).unwrap();

        let mut checks = Vec::new();
        check_gc_disk_usage(&paths, &mut checks);

        assert_eq!(checks[0].status, CheckStatus::Ok);
        // Should report bytes in details
        let bytes = checks[0].details.as_ref().unwrap()["bytes"]
            .as_u64()
            .unwrap();
        assert!(bytes >= 2048);
    }

    // -----------------------------------------------------------------------
    // G7. template-repos-parseable
    // -----------------------------------------------------------------------

    #[test]
    fn template_repos_parseable_ok() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = test_paths(tmp.path());
        fs::create_dir_all(&paths.templates_dir).unwrap();

        // Create a template with valid URL
        let tmpl = template::Template {
            name: Some("test".into()),
            description: None,
            wsp_version: None,
            repos: vec![template::TemplateRepo {
                url: "git@github.com:acme/repo.git".into(),
                setup_commands: None,
            }],
            config: None,
            agent_md: None,
            setup_commands: None,
        };
        template::save(&paths.templates_dir, "test", &tmpl).unwrap();

        let mut checks = Vec::new();
        check_template_repos_parseable(&paths, &mut checks);

        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].check, "template-repos-parseable");
        assert_eq!(checks[0].status, CheckStatus::Ok);
    }

    #[test]
    fn template_repos_parseable_bad_url() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = test_paths(tmp.path());
        fs::create_dir_all(&paths.templates_dir).unwrap();

        let tmpl = template::Template {
            name: Some("bad".into()),
            description: None,
            wsp_version: None,
            repos: vec![template::TemplateRepo {
                url: "not-a-valid-url".into(),
                setup_commands: None,
            }],
            config: None,
            agent_md: None,
            setup_commands: None,
        };
        template::save(&paths.templates_dir, "bad", &tmpl).unwrap();

        let mut checks = Vec::new();
        check_template_repos_parseable(&paths, &mut checks);

        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].status, CheckStatus::Warn);
        assert!(checks[0].message.contains("failed to parse"));
    }

    // -----------------------------------------------------------------------
    // G8. template-repos-registered
    // -----------------------------------------------------------------------

    #[test]
    fn template_repos_registered_ok() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = test_paths(tmp.path());
        fs::create_dir_all(&paths.templates_dir).unwrap();

        let tmpl = template::Template {
            name: Some("test".into()),
            description: None,
            wsp_version: None,
            repos: vec![template::TemplateRepo {
                url: "git@github.com:acme/repo.git".into(),
                setup_commands: None,
            }],
            config: None,
            agent_md: None,
            setup_commands: None,
        };
        template::save(&paths.templates_dir, "test", &tmpl).unwrap();

        let cfg = config::Config {
            repos: std::collections::BTreeMap::from([(
                "github.com/acme/repo".to_string(),
                config::RepoEntry {
                    url: "git@github.com:acme/repo.git".into(),
                    added: chrono::Utc::now(),
                    setup_commands: None,
                },
            )]),
            ..Default::default()
        };

        let mut checks = Vec::new();
        let mut fixed = 0;
        check_template_repos_registered(&paths, &cfg, false, &mut checks, &mut fixed);

        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].status, CheckStatus::Ok);
    }

    #[test]
    fn template_repos_unregistered() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = test_paths(tmp.path());
        fs::create_dir_all(&paths.templates_dir).unwrap();

        let tmpl = template::Template {
            name: Some("test".into()),
            description: None,
            wsp_version: None,
            repos: vec![template::TemplateRepo {
                url: "git@github.com:acme/repo.git".into(),
                setup_commands: None,
            }],
            config: None,
            agent_md: None,
            setup_commands: None,
        };
        template::save(&paths.templates_dir, "test", &tmpl).unwrap();

        let cfg = config::Config::default(); // No repos registered

        let mut checks = Vec::new();
        let mut fixed = 0;
        check_template_repos_registered(&paths, &cfg, false, &mut checks, &mut fixed);

        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].status, CheckStatus::Warn);
        assert!(checks[0].fixable);
        assert!(checks[0].message.contains("not in registry"));
    }

    // -----------------------------------------------------------------------
    // W5. missing-dirs-map
    // -----------------------------------------------------------------------

    #[test]
    fn missing_dirs_map_no_collision() {
        let tmp = tempfile::tempdir().unwrap();
        let ws_dir = tmp.path().join("ws");
        // Two repos, no collision — dirs map should be empty
        let meta = workspace::Metadata {
            version: 0,
            name: "test".into(),
            branch: "test/branch".into(),
            repos: std::collections::BTreeMap::from([
                ("github.com/acme/repo1".into(), None),
                ("github.com/acme/repo2".into(), None),
            ]),
            created: chrono::Utc::now(),
            description: None,
            last_used: None,
            created_from: None,
            dirs: std::collections::BTreeMap::new(),
            config: None,
            setup_commands: std::collections::BTreeMap::new(),
        };
        create_workspace_on_disk(&ws_dir, &meta);

        let mut checks = Vec::new();
        let mut fixed = 0;
        check_missing_dirs_map(
            &ws_dir,
            &meta,
            "workspace/test",
            false,
            &mut checks,
            &mut fixed,
        );

        // No collision → no check emitted
        assert!(checks.is_empty());
    }

    #[test]
    fn missing_dirs_map_collision_detected() {
        let tmp = tempfile::tempdir().unwrap();
        let ws_dir = tmp.path().join("ws");
        // Two repos with same short name but from different orgs → collision
        let meta = workspace::Metadata {
            version: 0,
            name: "test".into(),
            branch: "test/branch".into(),
            repos: std::collections::BTreeMap::from([
                ("github.com/org1/shared".into(), None),
                ("github.com/org2/shared".into(), None),
            ]),
            created: chrono::Utc::now(),
            description: None,
            last_used: None,
            created_from: None,
            dirs: std::collections::BTreeMap::new(), // Missing collision entries!
            config: None,
            setup_commands: std::collections::BTreeMap::new(),
        };
        create_workspace_on_disk(&ws_dir, &meta);

        let mut checks = Vec::new();
        let mut fixed = 0;
        check_missing_dirs_map(
            &ws_dir,
            &meta,
            "workspace/test",
            false,
            &mut checks,
            &mut fixed,
        );

        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].check, "missing-dirs-map");
        assert_eq!(checks[0].status, CheckStatus::Warn);
        assert!(checks[0].fixable);
    }

    #[test]
    fn missing_dirs_map_fix() {
        let tmp = tempfile::tempdir().unwrap();
        let ws_dir = tmp.path().join("ws");
        let meta = workspace::Metadata {
            version: 0,
            name: "test".into(),
            branch: "test/branch".into(),
            repos: std::collections::BTreeMap::from([
                ("github.com/org1/shared".into(), None),
                ("github.com/org2/shared".into(), None),
            ]),
            created: chrono::Utc::now(),
            description: None,
            last_used: None,
            created_from: None,
            dirs: std::collections::BTreeMap::new(),
            config: None,
            setup_commands: std::collections::BTreeMap::new(),
        };
        create_workspace_on_disk(&ws_dir, &meta);

        let mut checks = Vec::new();
        let mut fixed = 0;
        check_missing_dirs_map(
            &ws_dir,
            &meta,
            "workspace/test",
            true,
            &mut checks,
            &mut fixed,
        );

        assert_eq!(fixed, 1);
        assert_eq!(checks[0].status, CheckStatus::Ok);

        // Verify fix persisted
        let reloaded = workspace::load_metadata(&ws_dir).unwrap();
        assert!(reloaded.dirs.contains_key("github.com/org1/shared"));
        assert!(reloaded.dirs.contains_key("github.com/org2/shared"));
    }

    #[test]
    fn missing_dirs_map_value_mismatch() {
        let tmp = tempfile::tempdir().unwrap();
        let ws_dir = tmp.path().join("ws");
        // Two repos with same short name → collision. dirs has right keys but wrong values.
        let meta = workspace::Metadata {
            version: 0,
            name: "test".into(),
            branch: "test/branch".into(),
            repos: std::collections::BTreeMap::from([
                ("github.com/org1/shared".into(), None),
                ("github.com/org2/shared".into(), None),
            ]),
            created: chrono::Utc::now(),
            description: None,
            last_used: None,
            created_from: None,
            dirs: std::collections::BTreeMap::from([
                ("github.com/org1/shared".into(), "wrong-name-1".into()),
                ("github.com/org2/shared".into(), "wrong-name-2".into()),
            ]),
            config: None,
            setup_commands: std::collections::BTreeMap::new(),
        };
        create_workspace_on_disk(&ws_dir, &meta);

        let mut checks = Vec::new();
        let mut fixed = 0;
        check_missing_dirs_map(
            &ws_dir,
            &meta,
            "workspace/test",
            false,
            &mut checks,
            &mut fixed,
        );

        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].check, "missing-dirs-map");
        assert_eq!(checks[0].status, CheckStatus::Warn);
        assert!(
            checks[0]
                .message
                .contains("incorrect directory name mappings"),
            "expected value mismatch message, got: {}",
            checks[0].message
        );
    }

    // -----------------------------------------------------------------------
    // W10. wspignore-defaults
    // -----------------------------------------------------------------------

    #[test]
    fn wspignore_defaults_ok() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = test_paths(tmp.path());
        // Write the default wspignore content
        fs::write(
            paths.data_dir().join("wspignore"),
            workspace::DEFAULT_WSPIGNORE,
        )
        .unwrap();

        let mut checks = Vec::new();
        let mut fixed = 0;
        check_wspignore_defaults(&paths, false, &mut checks, &mut fixed);

        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].check, "wspignore-defaults");
        assert_eq!(checks[0].status, CheckStatus::Ok);
    }

    #[test]
    fn wspignore_defaults_missing_patterns() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = test_paths(tmp.path());
        // Write a partial wspignore
        fs::write(paths.data_dir().join("wspignore"), "# Partial\n.DS_Store\n").unwrap();

        let mut checks = Vec::new();
        let mut fixed = 0;
        check_wspignore_defaults(&paths, false, &mut checks, &mut fixed);

        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].status, CheckStatus::Warn);
        assert!(checks[0].fixable);
    }

    #[test]
    fn wspignore_defaults_fix() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = test_paths(tmp.path());
        // Write a partial wspignore missing some defaults
        fs::write(paths.data_dir().join("wspignore"), "# Partial\n.DS_Store\n").unwrap();

        let mut checks = Vec::new();
        let mut fixed = 0;
        check_wspignore_defaults(&paths, true, &mut checks, &mut fixed);

        assert_eq!(fixed, 1);
        assert_eq!(checks[0].status, CheckStatus::Ok);

        // Verify missing defaults were appended
        let content = fs::read_to_string(paths.data_dir().join("wspignore")).unwrap();
        assert!(content.contains("Thumbs.db"));
        assert!(content.contains("desktop.ini"));
    }

    // -----------------------------------------------------------------------
    // W11. go-work-valid
    // -----------------------------------------------------------------------

    #[test]
    fn go_work_valid_no_go() {
        let tmp = tempfile::tempdir().unwrap();
        let ws_dir = tmp.path().join("ws");
        let meta = test_metadata(
            "test",
            "test/branch",
            std::collections::BTreeMap::from([("github.com/acme/frontend".into(), None)]),
        );
        create_workspace_on_disk(&ws_dir, &meta);
        // Create a non-Go repo
        let repo_dir = ws_dir.join("frontend");
        fs::create_dir_all(&repo_dir).unwrap();
        fs::write(repo_dir.join("package.json"), "{}").unwrap();

        let mut checks = Vec::new();
        let mut fixed = 0;
        check_go_work_valid(
            &ws_dir,
            &meta,
            "workspace/test",
            false,
            &mut checks,
            &mut fixed,
        );

        // No go.work, no Go repos → no check emitted
        assert!(checks.is_empty());
    }

    #[test]
    fn go_work_valid_ok() {
        let tmp = tempfile::tempdir().unwrap();
        let ws_dir = tmp.path().join("ws");
        let meta = test_metadata(
            "test",
            "test/branch",
            std::collections::BTreeMap::from([("github.com/acme/api".into(), None)]),
        );
        create_workspace_on_disk(&ws_dir, &meta);

        // Create Go repo and valid go.work
        let repo_dir = ws_dir.join("api");
        fs::create_dir_all(&repo_dir).unwrap();
        fs::write(
            repo_dir.join("go.mod"),
            "module example.com/api\n\ngo 1.22\n",
        )
        .unwrap();
        fs::write(
            ws_dir.join("go.work"),
            format!(
                "{}\ngo 1.22\n\nuse (\n\t./api\n)\n",
                wsp_core::lang::GO_WORK_HEADER
            ),
        )
        .unwrap();

        let mut checks = Vec::new();
        let mut fixed = 0;
        check_go_work_valid(
            &ws_dir,
            &meta,
            "workspace/test",
            false,
            &mut checks,
            &mut fixed,
        );

        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].check, "go-work-valid");
        assert_eq!(checks[0].status, CheckStatus::Ok);
    }

    #[test]
    fn go_work_not_wsp_managed() {
        let tmp = tempfile::tempdir().unwrap();
        let ws_dir = tmp.path().join("ws");
        let meta = test_metadata(
            "test",
            "test/branch",
            std::collections::BTreeMap::from([("github.com/acme/api".into(), None)]),
        );
        create_workspace_on_disk(&ws_dir, &meta);

        // go.work without wsp header
        fs::write(ws_dir.join("go.work"), "go 1.22\n\nuse (\n\t./api\n)\n").unwrap();

        let mut checks = Vec::new();
        let mut fixed = 0;
        check_go_work_valid(
            &ws_dir,
            &meta,
            "workspace/test",
            false,
            &mut checks,
            &mut fixed,
        );

        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].status, CheckStatus::Warn);
        assert!(checks[0].fixable);
    }

    // -----------------------------------------------------------------------
    // W13. mirror-refspec
    // -----------------------------------------------------------------------

    #[test]
    fn mirror_refspec_ok() {
        let (clone_dir, _source, _ct, _st) = wsp_core::testutil::setup_clone_repo();

        let mut checks = Vec::new();
        let mut fixed = 0;
        check_mirror_refspec(
            &clone_dir,
            "repo",
            "workspace/test/repo",
            false,
            &mut checks,
            &mut fixed,
        );

        // setup_clone_repo creates proper refspecs → no check emitted
        assert!(checks.is_empty());
    }

    #[test]
    fn mirror_refspec_missing_detected() {
        let tmp = tempfile::tempdir().unwrap();
        let clone_dir = tmp.path().join("repo");
        fs::create_dir_all(&clone_dir).unwrap();
        init_git_repo(&clone_dir);

        // Add origin remote with no fetch refspec (bare remote)
        git::run(
            Some(&clone_dir),
            &["remote", "add", "origin", "https://example.com/repo.git"],
        )
        .unwrap();
        // Remove the default fetch refspec
        let _ = std::process::Command::new("git")
            .args(["config", "--unset-all", "remote.origin.fetch"])
            .current_dir(&clone_dir)
            .output();

        let mut checks = Vec::new();
        let mut fixed = 0;
        check_mirror_refspec(
            &clone_dir,
            "repo",
            "workspace/test/repo",
            false,
            &mut checks,
            &mut fixed,
        );

        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].check, "mirror-refspec");
        assert_eq!(checks[0].status, CheckStatus::Warn);
        assert!(checks[0].fixable);
    }

    #[test]
    fn mirror_refspec_fix() {
        let tmp = tempfile::tempdir().unwrap();
        let clone_dir = tmp.path().join("repo");
        fs::create_dir_all(&clone_dir).unwrap();
        init_git_repo(&clone_dir);

        git::run(
            Some(&clone_dir),
            &["remote", "add", "origin", "https://example.com/repo.git"],
        )
        .unwrap();
        let _ = std::process::Command::new("git")
            .args(["config", "--unset-all", "remote.origin.fetch"])
            .current_dir(&clone_dir)
            .output();

        let mut checks = Vec::new();
        let mut fixed = 0;
        check_mirror_refspec(
            &clone_dir,
            "repo",
            "workspace/test/repo",
            true,
            &mut checks,
            &mut fixed,
        );

        assert_eq!(fixed, 1);
        assert_eq!(checks[0].status, CheckStatus::Ok);

        // Verify refspec was added
        let output = std::process::Command::new("git")
            .args(["config", "--get-all", "remote.origin.fetch"])
            .current_dir(&clone_dir)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("+refs/heads/*:refs/remotes/origin/*"));
    }

    // -----------------------------------------------------------------------
    // G11: Deprecated config keys
    // -----------------------------------------------------------------------

    #[test]
    fn deprecated_config_keys_none() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = test_paths(tmp.path());
        let cfg = config::Config::default();
        cfg.save_to(&paths.config_path).unwrap();

        let mut checks = Vec::new();
        let mut fixed = 0;
        check_deprecated_config_keys(&paths, &cfg, false, &mut checks, &mut fixed);

        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].status, CheckStatus::Ok);
    }

    #[test]
    fn deprecated_config_keys_experimental_detected() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = test_paths(tmp.path());
        // Write old-format config with experimental section
        std::fs::write(
            &paths.config_path,
            "experimental:\n  enabled: true\n  shell-prompt: true\n",
        )
        .unwrap();

        let cfg = config::Config::load_from(&paths.config_path).unwrap();
        let mut checks = Vec::new();
        let mut fixed = 0;
        check_deprecated_config_keys(&paths, &cfg, false, &mut checks, &mut fixed);

        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].status, CheckStatus::Warn);
        assert!(checks[0].fixable);
        assert!(checks[0].message.contains("experimental"));
    }

    #[test]
    fn deprecated_config_keys_fix_migrates() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = test_paths(tmp.path());
        // Write old-format config
        std::fs::write(
            &paths.config_path,
            "experimental:\n  enabled: true\n  shell-prompt: true\n  shell-tmux: window-title\n",
        )
        .unwrap();

        let cfg = config::Config::load_from(&paths.config_path).unwrap();
        let mut checks = Vec::new();
        let mut fixed = 0;
        check_deprecated_config_keys(&paths, &cfg, true, &mut checks, &mut fixed);

        assert_eq!(fixed, 1);
        assert_eq!(checks[0].status, CheckStatus::Ok);
        assert!(checks[0].message.contains("migrated"));

        // Verify the new format was written
        let reloaded = config::Config::load_from(&paths.config_path).unwrap();
        assert!(reloaded.shell_prompt_enabled());
        assert_eq!(reloaded.shell_tmux_mode(), Some("window-title"));
        // experimental section should not be written back
        let raw = std::fs::read_to_string(&paths.config_path).unwrap();
        assert!(
            !raw.contains("experimental"),
            "experimental should not appear in migrated config: {}",
            raw
        );
    }

    // -----------------------------------------------------------------------
    // Helper: format_bytes
    // -----------------------------------------------------------------------

    #[test]
    fn format_bytes_cases() {
        assert_eq!(format_bytes(0), "0 bytes");
        assert_eq!(format_bytes(512), "512 bytes");
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(1048576), "1.0 MB");
        assert_eq!(format_bytes(1073741824), "1.0 GB");
    }
}
