//! Quint orchestration for the workspace-local ownership specification.

use std::{
    fs,
    process::{Command, Output},
};

use anyhow::{Context, Result, bail};

const QUINT: &str = "@informalsystems/quint@0.32.0";
const MODEL: &str = "formal/quint/workspace_local.qnt";

pub(crate) fn check() -> Result<()> {
    typecheck(MODEL)?;
    for fault in [
        "formal/quint/faults/register_before_member_check.qnt",
        "formal/quint/faults/unstable_mapping.qnt",
        "formal/quint/faults/overwrite_occupied_publication.qnt",
        "formal/quint/faults/stale_metadata_snapshot.qnt",
        "formal/quint/faults/clear_after_failed_delete.qnt",
        "formal/quint/faults/clear_missing_without_force.qnt",
        "formal/quint/faults/force_removes_foreign.qnt",
        "formal/quint/faults/route_by_host_name.qnt",
        "formal/quint/faults/transport_fallback.qnt",
        "formal/quint/faults/replay_setup_on_adoption.qnt",
        "formal/quint/faults/stale_guidance_snapshot.qnt",
    ] {
        typecheck(fault)?;
    }

    run_ok(&["test", MODEL, "--main=workspace_local", "--seed=0x168"])?;
    run_ok(&[
        "run",
        MODEL,
        "--main=workspace_local",
        "--seed=0x168",
        "--max-samples=200",
        "--max-steps=60",
        "--verbosity=0",
        "--invariants",
        "validMembers",
        "uniqueMappings",
        "localConfinement",
        "mappingIsAllocated",
        "registryDoesNotOwnMembership",
        "mountedInvocationLeavesSiblingUntouched",
        "noTransportRetry",
        "removalAccounting",
    ])?;

    // These mutation tests keep the oracle honest: every listed violation
    // must remain reachable at its deterministic seed.
    for (model, main, invariant, seed, steps) in [
        (
            "formal/quint/faults/register_before_member_check.qnt",
            "register_before_member_check",
            "existingRetryHasNoGlobalEffect",
            "0x168",
            "2",
        ),
        (
            "formal/quint/faults/unstable_mapping.qnt",
            "unstable_mapping",
            "stableMapping",
            "0x168",
            "2",
        ),
        (
            "formal/quint/faults/overwrite_occupied_publication.qnt",
            "overwrite_occupied_publication",
            "foreignDestinationPreserved",
            "0x168",
            "2",
        ),
        (
            "formal/quint/faults/stale_metadata_snapshot.qnt",
            "stale_metadata_snapshot",
            "peerMemberPreserved",
            "0x168",
            "2",
        ),
        (
            "formal/quint/faults/clear_after_failed_delete.qnt",
            "clear_after_failed_delete",
            "failedDeleteKeepsMembership",
            "0x168",
            "2",
        ),
        (
            "formal/quint/faults/clear_missing_without_force.qnt",
            "clear_missing_without_force",
            "missingCloneNeedsForcedConfirmation",
            "0x168",
            "2",
        ),
        (
            "formal/quint/faults/force_removes_foreign.qnt",
            "force_removes_foreign",
            "forceNeverClearsForeignOwnership",
            "0x168",
            "2",
        ),
        (
            "formal/quint/faults/route_by_host_name.qnt",
            "route_by_host_name",
            "mountedInvocationLeavesSiblingUntouched",
            "0x168",
            "2",
        ),
        (
            "formal/quint/faults/transport_fallback.qnt",
            "transport_fallback",
            "noTransportRetry",
            "1",
            "6",
        ),
        (
            "formal/quint/faults/replay_setup_on_adoption.qnt",
            "replay_setup_on_adoption",
            "adoptionDoesNotReplaySetup",
            "0x168",
            "2",
        ),
        (
            "formal/quint/faults/stale_guidance_snapshot.qnt",
            "stale_guidance_snapshot",
            "guidanceMatchesLatestMetadata",
            "0x168",
            "2",
        ),
    ] {
        run_violation(
            &[
                "run",
                model,
                &format!("--main={main}"),
                &format!("--seed={seed}"),
                "--max-samples=1",
                &format!("--max-steps={steps}"),
                "--invariants",
                invariant,
            ],
            invariant,
        )?;
    }
    Ok(())
}

pub(crate) fn traces() -> Result<()> {
    fs::create_dir_all("target/quint-traces").context("create Quint trace directory")?;
    run_ok(&[
        "run",
        MODEL,
        "--main=workspace_local",
        "--seed=0x168",
        "--max-samples=1",
        "--max-steps=60",
        "--verbosity=0",
        "--out-itf",
        "target/quint-traces/workspace_local_{seq}.itf.json",
    ])
}

fn typecheck(input: &str) -> Result<()> {
    run_ok(&["typecheck", input])
}

fn run_ok(args: &[&str]) -> Result<()> {
    let status = quint(args)?.status;
    if status.success() {
        Ok(())
    } else {
        bail!("Quint command unexpectedly failed: {}", args.join(" "))
    }
}

/// A mutation passes only when Quint reports the invariant failure we asked
/// for. A non-zero status also covers type errors, malformed arguments, and
/// runner failures, none of which exercise the mutation oracle.
fn run_violation(args: &[&str], expected_invariant: &str) -> Result<()> {
    let output = quint(args)?;
    if output.status.success() {
        bail!(
            "faulty model satisfied invariant {expected_invariant}: {}",
            args.join(" ")
        );
    }
    classify_violation(&output, args, expected_invariant).with_context(|| {
        format!(
            "fault model did not violate expected invariant {expected_invariant}: {}",
            args.join(" ")
        )
    })
}

fn classify_violation(output: &Output, args: &[&str], expected_invariant: &str) -> Result<()> {
    if expected_invariant.is_empty() {
        bail!("mutation test must name an expected invariant");
    }
    let requested_invariant = args
        .windows(2)
        .find_map(|pair| (pair[0] == "--invariants").then_some(pair[1]));
    if requested_invariant != Some(expected_invariant) {
        bail!(
            "mutation command must select expected invariant {expected_invariant}, got {requested_invariant:?}"
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}{stderr}");
    if combined.contains("error: Invariant violated") {
        Ok(())
    } else {
        bail!("expected Quint invariant violation for {expected_invariant}, got: {combined}")
    }
}

fn quint(args: &[&str]) -> Result<Output> {
    Command::new("npx")
        .arg("--yes")
        .arg(QUINT)
        .args(args)
        .output()
        .with_context(|| format!("run npx --yes {QUINT} {}", args.join(" ")))
        .inspect(|output| {
            print!("{}", String::from_utf8_lossy(&output.stdout));
            eprint!("{}", String::from_utf8_lossy(&output.stderr));
        })
}

#[cfg(test)]
mod tests {
    use std::process::{ExitStatus, Output};

    use super::classify_violation;

    #[cfg(unix)]
    fn status(code: i32) -> ExitStatus {
        use std::os::unix::process::ExitStatusExt;
        ExitStatus::from_raw(code << 8)
    }

    #[cfg(windows)]
    fn status(code: i32) -> ExitStatus {
        use std::os::windows::process::ExitStatusExt;
        ExitStatus::from_raw(code as u32)
    }

    fn output(stdout: &str, stderr: &str) -> Output {
        Output {
            status: status(1),
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    #[test]
    fn accepts_the_expected_quint_invariant_marker() {
        let output = output(
            "[violation] Found an issue\n",
            "error: Invariant violated\n",
        );
        assert!(
            classify_violation(&output, &["--invariants", "stableMapping"], "stableMapping")
                .is_ok()
        );
    }

    #[test]
    fn rejects_nonzero_argument_errors() {
        let output = output(
            "",
            "error: [QNT404] Name 'missing' not found\nerror: Argument error\n",
        );
        assert!(
            classify_violation(&output, &["--invariants", "stableMapping"], "stableMapping")
                .is_err()
        );
    }

    #[test]
    fn rejects_a_different_invariant_than_the_one_declared_by_the_mutation() {
        let output = output("", "error: Invariant violated\n");
        assert!(
            classify_violation(
                &output,
                &["--invariants", "otherInvariant"],
                "stableMapping",
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_an_unnamed_mutation_expectation() {
        let output = output("", "error: Invariant violated\n");
        assert!(classify_violation(&output, &["--invariants", ""], "").is_err());
    }
}
