//! Opt-in access observations using the same isolated policy as parallel Git.
//!
//! These checks report one bounded attempt. They cannot certify a credential
//! helper, predict future access, or diagnose authentication from error text.
use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use clap::{Arg, ArgAction};
use wsp_core::config::{Config, Paths};
use wsp_core::git::{self, AccessResult};
use wsp_core::output::{CheckStatus, DoctorCheck};
use wsp_core::{git_policy, giturl, mirror, progress, workspace};

const TIMEOUT: Duration = Duration::from_secs(15);

pub(crate) fn check_access_arg() -> Arg {
    Arg::new("check-access")
        .long("check-access")
        .action(ArgAction::SetTrue)
        .help("Test current remote access without terminal prompts (15s limit per remote)")
}

pub(crate) fn checks(
    cfg: &Config,
    paths: Option<&Paths>,
    workspace_dir: Option<&Path>,
) -> Result<Vec<DoctorCheck>> {
    let mut checks = Vec::new();
    if let Some(paths) = paths {
        for (identity, entry) in &cfg.repos {
            let dir = giturl::parse(&entry.url)
                .ok()
                .map(|parsed| mirror::dir(&paths.mirrors_dir, &parsed))
                .filter(|dir| dir.is_dir());
            checks.push(probe(
                &format!("registry/{identity}"),
                identity,
                dir.as_deref(),
                &entry.url,
                "repeat the intended wsp Git operation with --git-progress native",
            ));
        }
    }
    if let Some(dir) = workspace_dir {
        let meta = workspace::load_metadata(dir)?;
        for info in meta.repo_infos(dir) {
            let scope = format!("workspace/{}/{}", meta.name, info.dir_name);
            let retry = "wsp repo fetch --git-progress native";
            if info.error.is_some() || !info.clone_dir.is_dir() {
                checks.push(result_check(
                    &scope,
                    AccessResult::Failed("clone directory is unavailable; run wsp doctor".into()),
                    "origin",
                    retry,
                ));
                continue;
            }
            checks.push(probe(
                &scope,
                &info.identity,
                Some(&info.clone_dir),
                "origin",
                retry,
            ));
        }
    }
    if checks.is_empty() {
        let check = DoctorCheck {
            scope: "global".into(),
            check: "git-access".into(),
            status: CheckStatus::Ok,
            message:
                "no repository remotes to check; register a repository or run inside a workspace"
                    .into(),
            fixable: false,
            details: Some(serde_json::json!({"result": "skipped", "mode": "parallel"})),
        };
        progress::eprintln!("  {}", check.message);
        checks.push(check);
    }
    Ok(checks)
}

fn probe(
    scope: &str,
    identity: &str,
    dir: Option<&Path>,
    remote: &str,
    retry: &str,
) -> DoctorCheck {
    let _identity = git_policy::repository(identity);
    let _progress = progress::Progress::start(format!("Checking remote access for {identity}"));
    let result = git::probe_access(dir, remote, TIMEOUT)
        .unwrap_or_else(|error| AccessResult::Failed(format!("{error:#}")));
    result_check(scope, result, remote, retry)
}

fn result_check(scope: &str, result: AccessResult, _remote: &str, retry: &str) -> DoctorCheck {
    let (status, message, details) = match result {
        AccessResult::Succeeded => (
            CheckStatus::Ok,
            "remote access succeeded without terminal prompts on this attempt".to_owned(),
            serde_json::json!({"result": "succeeded", "mode": "parallel", "timeout_seconds": 15}),
        ),
        AccessResult::Failed(_) => (
            CheckStatus::Warn,
            format!("remote access failed without terminal prompts; cause unknown. Try: {retry}"),
            serde_json::json!({
                "result": "failed", "mode": "parallel", "cause": "unknown",
                "retry": retry,
                "timeout_seconds": 15,
            }),
        ),
        AccessResult::TimedOut => (
            CheckStatus::Warn,
            format!("remote access did not finish within 15 seconds; cause unknown. Try: {retry}"),
            serde_json::json!({
                "result": "timed_out", "mode": "parallel", "cause": "unknown",
                "retry": retry, "timeout_seconds": 15,
            }),
        ),
    };
    progress::eprintln!("  {scope}: {message}");
    DoctorCheck {
        scope: scope.into(),
        check: "git-access".into(),
        status,
        message,
        fixable: false,
        details: Some(details),
    }
}

// Custom helpers can print unlabelled secrets. Access checks intentionally
// withhold their output rather than treating redaction as a confidentiality guarantee.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_observations_are_not_authentication_classifications() {
        for (result, status, label) in [
            (AccessResult::Succeeded, CheckStatus::Ok, "succeeded"),
            (
                AccessResult::Failed("permission denied".into()),
                CheckStatus::Warn,
                "failed",
            ),
            (AccessResult::TimedOut, CheckStatus::Warn, "timed_out"),
        ] {
            let check = result_check(
                "test",
                result,
                "origin",
                "wsp repo fetch --git-progress native",
            );
            assert_eq!(check.status, status);
            assert!(!check.fixable);
            let details = check.details.unwrap();
            assert_eq!(details["result"], label);
            assert_eq!(details["mode"], "parallel");
            if status == CheckStatus::Warn {
                assert_eq!(details["cause"], "unknown");
                assert!(check.message.contains("--git-progress native"));
            }
        }
    }

    #[test]
    fn access_failures_withhold_arbitrary_helper_output() {
        for diagnostic in [
            "unlabelled-private-value",
            "https://user:private@example.test/repo",
        ] {
            let check = result_check(
                "test",
                AccessResult::Failed(diagnostic.into()),
                "origin",
                "wsp repo fetch --git-progress native",
            );
            let json = serde_json::to_string(&check).unwrap();
            assert!(!json.contains(diagnostic));
            assert!(!json.contains("private"));
            assert!(
                !check
                    .details
                    .unwrap()
                    .as_object()
                    .unwrap()
                    .contains_key("diagnostic")
            );
        }
    }
}
