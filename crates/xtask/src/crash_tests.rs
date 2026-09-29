//! Dedicated build entry point for test-only crash barrier instrumentation.

use std::env;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result, bail};

const CRASH_CFG: &str = "wsp_crash_test";

pub fn run() -> Result<()> {
    run_cargo(&[
        "clippy",
        "-p",
        "wsp",
        "--all-targets",
        "--features",
        "test-crash-barriers",
        "--",
        "-D",
        "warnings",
    ])?;
    run_cargo(&[
        "test",
        "-p",
        "wsp",
        "--test",
        "workspace_crash",
        "--features",
        "test-crash-barriers",
    ])?;
    verify_release_exclusion()
}

fn run_cargo(args: &[&str]) -> Result<()> {
    let mut command = Command::new("cargo");
    command.args(args);
    command.env("CARGO_TARGET_DIR", crash_target_dir()?);
    command.env(
        "WSP_VERSION_STRING",
        env::var("WSP_VERSION_STRING").unwrap_or_else(|_| env!("CARGO_PKG_VERSION").into()),
    );
    configure_rustflags(&mut command, &["--cfg", CRASH_CFG])?;
    run_status(command, &format!("crash-barrier cargo task {args:?}"))
}

/// Proves that ordinary and release artifacts cannot accidentally enable the
/// test protocol, and that a normal release binary ignores its private controls.
fn verify_release_exclusion() -> Result<()> {
    expect_cargo_guard_failure(
        &["check", "-p", "wsp", "--features", "test-crash-barriers"],
        &[],
        "feature without explicit crash cfg",
        "test-crash-barriers requires --cfg wsp_crash_test",
    )?;
    expect_cargo_guard_failure(
        &["check", "-p", "wsp"],
        &["--cfg", CRASH_CFG],
        "crash cfg without feature",
        "--cfg wsp_crash_test requires the test-crash-barriers feature",
    )?;
    expect_cargo_guard_failure(
        &[
            "check",
            "-p",
            "wsp",
            "--release",
            "--features",
            "test-crash-barriers",
        ],
        &["--cfg", CRASH_CFG, "-C", "debug-assertions=yes"],
        "release check with crash instrumentation",
        "test-crash-barriers may only be built with Cargo's debug profile",
    )?;
    expect_cargo_guard_failure(
        &[
            "build",
            "-p",
            "wsp",
            "--release",
            "--features",
            "test-crash-barriers",
        ],
        &["--cfg", CRASH_CFG, "-C", "debug-assertions=yes"],
        "release artifact build with crash instrumentation",
        "test-crash-barriers may only be built with Cargo's debug profile",
    )?;

    let mut command = Command::new("cargo");
    command.args(["run", "-p", "wsp", "--release", "--", "--help"]);
    command.env("CARGO_TARGET_DIR", exclusion_target_dir()?);
    command.env("WSP_TEST_CRASH_ADDR", "127.0.0.1:1");
    command.env("WSP_TEST_CRASH_TOKEN", "release-test-token");
    command.env("WSP_TEST_CRASH_SESSION", "release-test-session");
    configure_rustflags(&mut command, &[])?;
    run_status(command, "ordinary release binary with crash controls set")
}

fn expect_cargo_guard_failure(
    args: &[&str],
    extra_flags: &[&str],
    description: &str,
    expected_diagnostic: &str,
) -> Result<()> {
    let mut command = Command::new("cargo");
    command.args(args);
    command.env("CARGO_TARGET_DIR", exclusion_target_dir()?);
    configure_rustflags(&mut command, extra_flags)?;
    let output = command
        .output()
        .with_context(|| format!("running exclusion check: {description}"))?;
    if output.status.success() {
        bail!("exclusion check unexpectedly succeeded: {description}");
    }
    let diagnostics = String::from_utf8_lossy(&output.stderr);
    if !diagnostics_contain_guard(&diagnostics, expected_diagnostic) {
        bail!(
            "exclusion check failed without its expected guard diagnostic: {description}\n\
             expected: {expected_diagnostic}\n\
             status: {}\n\
             stderr:\n{diagnostics}",
            output.status
        );
    }
    Ok(())
}

fn diagnostics_contain_guard(diagnostics: &str, expected_diagnostic: &str) -> bool {
    diagnostics.contains(expected_diagnostic)
}

fn run_status(mut command: Command, description: &str) -> Result<()> {
    let status = command
        .status()
        .with_context(|| format!("running {description}"))?;
    if !status.success() {
        bail!("{description} failed with {status}");
    }
    Ok(())
}

fn crash_target_dir() -> Result<PathBuf> {
    workspace_root().map(|root| root.join("target/crash-tests"))
}

fn exclusion_target_dir() -> Result<PathBuf> {
    workspace_root().map(|root| root.join("target/crash-exclusion"))
}

fn workspace_root() -> Result<PathBuf> {
    PathBuf::from(env::var("CARGO_MANIFEST_DIR")?)
        .parent()
        .and_then(|path| path.parent())
        .map(PathBuf::from)
        .context("locating workspace root")
}

fn configure_rustflags(command: &mut Command, extra_flags: &[&str]) -> Result<()> {
    let plain = env::var("RUSTFLAGS").unwrap_or_default();
    let encoded = env::var("CARGO_ENCODED_RUSTFLAGS").unwrap_or_default();
    if !plain.is_empty() && !encoded.is_empty() {
        bail!("refusing both RUSTFLAGS and CARGO_ENCODED_RUSTFLAGS for crash tests");
    }
    if plain.contains(CRASH_CFG) || encoded.split('\x1f').any(|flag| flag.contains(CRASH_CFG)) {
        bail!("crash-test cfg is already present in inherited Rust flags");
    }
    if extra_flags.is_empty() {
        return Ok(());
    }
    if encoded.is_empty() {
        let separator = if plain.is_empty() { "" } else { " " };
        command.env(
            "RUSTFLAGS",
            format!("{plain}{separator}{}", extra_flags.join(" ")),
        );
    } else {
        let suffix = extra_flags.join("\x1f");
        command.env("CARGO_ENCODED_RUSTFLAGS", format!("{encoded}\x1f{suffix}"));
        command.env_remove("RUSTFLAGS");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_are_separate_from_normal_builds() {
        assert!(crash_target_dir().unwrap().ends_with("target/crash-tests"));
        assert!(
            exclusion_target_dir()
                .unwrap()
                .ends_with("target/crash-exclusion")
        );
    }

    #[test]
    fn exclusion_failure_requires_its_specific_guard_diagnostic() {
        assert!(diagnostics_contain_guard(
            "error: test-crash-barriers requires --cfg wsp_crash_test; run `just crash-test`",
            "test-crash-barriers requires --cfg wsp_crash_test",
        ));
        assert!(!diagnostics_contain_guard(
            "error: could not compile dependency due to an unrelated error",
            "test-crash-barriers requires --cfg wsp_crash_test",
        ));
    }
}
