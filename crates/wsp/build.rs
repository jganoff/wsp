use std::process::Command;

fn main() {
    println!("cargo::rustc-check-cfg=cfg(wsp_crash_test)");
    if std::env::var_os("CARGO_FEATURE_TEST_CRASH_BARRIERS").is_some()
        && std::env::var("PROFILE").as_deref() != Ok("debug")
    {
        panic!("test-crash-barriers may only be built with Cargo's debug profile");
    }

    // Re-run if git HEAD or tags change (relative to workspace root from crates/wsp/)
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/refs/tags");

    let pkg = env!("CARGO_PKG_VERSION");
    let tag = format!("v{pkg}");
    let describe = Command::new("git")
        .args(["describe", "--tags", "--dirty", "--always"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|value| value.trim().to_string())
        .unwrap_or_default();
    let version = if describe.is_empty() || describe == tag {
        pkg.to_string()
    } else {
        format!("{pkg} ({describe})")
    };
    println!("cargo:rustc-env=WSP_VERSION_STRING={version}");
}
