//! PR data fetching via the `gh` CLI.
//!
//! All functions degrade gracefully: if `gh` is not installed, not authenticated,
//! or the repo is not on GitHub, they return `None` without printing errors.

use wsp_core::output::PrInfo;

/// Parse a repo identity into the `owner/repo` slug expected by `gh`.
/// Only handles GitHub identities (`github.com/owner/repo`).
/// Returns `None` for any other host.
pub fn github_slug(identity: &str) -> Option<&str> {
    identity.strip_prefix("github.com/")
}

/// Fetch the most recent PR for `slug` (`owner/repo`) whose head branch
/// matches `branch`. Returns `None` if `gh` is unavailable, unauthenticated,
/// no PR is found, or any error occurs.
pub fn fetch(slug: &str, branch: &str) -> Option<PrInfo> {
    #[derive(serde::Deserialize)]
    struct GhPr {
        number: u64,
        url: String,
        state: String,
        title: String,
        #[serde(rename = "isDraft")]
        is_draft: bool,
    }

    let out = std::process::Command::new("gh")
        .args([
            "pr",
            "list",
            "--repo",
            slug,
            "--head",
            branch,
            "--json",
            "number,url,state,title,isDraft",
            "--limit",
            "1",
            "--state",
            "all",
        ])
        .output()
        .ok()?;

    if !out.status.success() {
        return None;
    }

    let prs: Vec<GhPr> = serde_json::from_slice(&out.stdout).ok()?;
    let pr = prs.into_iter().next()?;

    Some(PrInfo {
        number: pr.number,
        url: pr.url,
        state: pr.state,
        title: pr.title,
        is_draft: pr.is_draft,
    })
}

/// Fetch PR data for multiple repos in parallel.
/// `repos` is a slice of `(identity, branch)` pairs.
/// Returns a vec of `((identity, branch), Option<PrInfo>)` so callers don't
/// depend on positional correspondence between input and output.
/// The identity/branch in each result always echoes the input — even on worker
/// thread panic — so callers can safely use the identity as a map key.
pub fn fetch_parallel(repos: &[(String, String)]) -> Vec<((String, String), Option<PrInfo>)> {
    std::thread::scope(|s| {
        let handles: Vec<_> = repos
            .iter()
            .map(|(identity, branch)| {
                s.spawn(move || github_slug(identity).and_then(|slug| fetch(slug, branch)))
            })
            .collect();

        repos
            .iter()
            .zip(handles)
            .map(|((identity, branch), h)| {
                let pr = match h.join() {
                    Ok(result) => result,
                    Err(_) => {
                        eprintln!(
                            "warning: PR fetch thread panicked for {}/{}",
                            identity, branch
                        );
                        None
                    }
                };
                ((identity.clone(), branch.clone()), pr)
            })
            .collect()
    })
}
