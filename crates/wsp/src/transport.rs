//! Choose the shared mirror only when this invocation is allowed to refresh it.

use std::path::Path;

use anyhow::{Context, Result};
use wsp_core::config::Paths;
use wsp_core::{git, giturl, mirror, workspace_add};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RefreshTransport {
    Mirror,
    DirectOrigin,
}

pub(crate) struct RefreshResult {
    pub transport: RefreshTransport,
    pub fallback_reason: Option<&'static str>,
}

/// Refresh one workspace clone without changing its configured remote.
///
/// A workspace-local invocation never writes a global mirror. A host invocation
/// uses an existing matching mirror when it has one; an unregistered
/// workspace-local member continues through its ordinary `origin` remote.
pub(crate) fn refresh_clone(
    paths: Option<&Paths>,
    allow_mirror_write: bool,
    workspace_root: &Path,
    clone_dir: &Path,
    identity: &str,
    prune: bool,
    direct_reason: Option<&'static str>,
) -> Result<RefreshResult> {
    workspace_add::validate_clone(workspace_root, clone_dir, identity, None)?;
    if allow_mirror_write
        && let Some(paths) = paths
        && let Ok(parsed) = giturl::Parsed::from_identity(identity)
    {
        let mirror_dir = mirror::dir(&paths.mirrors_dir, &parsed);
        if mirror_dir.exists() && !mirror_dir.is_dir() {
            anyhow::bail!("mirror {} is not a directory", mirror_dir.display());
        }
        if mirror_dir.is_dir() {
            let origin = git::run(Some(&mirror_dir), &["config", "--get", "remote.origin.url"])
                .with_context(|| format!("reading mirror origin for {}", identity))?;
            if giturl::parse(origin.trim())?.identity() != identity {
                anyhow::bail!(
                    "mirror {} does not match workspace repository {}",
                    mirror_dir.display(),
                    identity
                );
            }
            wsp_core::crash_barrier!(
                wsp_core::crash_barrier::Operation::Refresh,
                identity,
                wsp_core::crash_barrier::Point::RefreshSelected,
                false,
            )?;
            git::fetch(&mirror_dir, prune)?;
            git::fetch_from_path(
                clone_dir,
                &mirror_dir,
                "+refs/remotes/origin/*:refs/remotes/origin/*",
                prune,
            )?;
            wsp_core::crash_barrier!(
                wsp_core::crash_barrier::Operation::Refresh,
                identity,
                wsp_core::crash_barrier::Point::RefreshComplete,
                false,
            )?;
            return Ok(RefreshResult {
                transport: RefreshTransport::Mirror,
                fallback_reason: None,
            });
        }
    }

    // Capture and verify the actual origin immediately before spawning Git.
    // The captured URL is passed as a command-line config override so a later
    // `.git/config` replacement cannot redirect this fetch.
    let refspecs = git::remote_fetch_refspecs(clone_dir, "origin")?;
    let origin = git::remote_get_configured_url(clone_dir, "origin")?;
    if giturl::parse(origin.trim())?.identity() != identity {
        anyhow::bail!(
            "clone {} origin no longer matches workspace repository {}",
            clone_dir.display(),
            identity
        );
    }
    wsp_core::crash_barrier!(
        wsp_core::crash_barrier::Operation::Refresh,
        identity,
        wsp_core::crash_barrier::Point::RefreshSelected,
        false,
    )?;
    git::fetch_remote_at_url_with_refspecs(clone_dir, "origin", &origin, &refspecs, prune)?;
    wsp_core::crash_barrier!(
        wsp_core::crash_barrier::Operation::Refresh,
        identity,
        wsp_core::crash_barrier::Point::RefreshComplete,
        false,
    )?;
    Ok(RefreshResult {
        transport: RefreshTransport::DirectOrigin,
        fallback_reason: direct_reason.or(Some("matching_mirror_absent")),
    })
}
