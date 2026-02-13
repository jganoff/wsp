use std::process::Command;

fn main() {
    // Re-run if git HEAD or tags change
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/tags");

    let pkg = env!("CARGO_PKG_VERSION");
    let tag = format!("v{}", pkg);

    let describe = Command::new("git")
        .args(["describe", "--tags", "--dirty", "--always"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    let version = if describe.is_empty() || describe == tag {
        pkg.to_string()
    } else {
        format!("{} ({})", pkg, describe)
    };

    println!("cargo:rustc-env=WS_VERSION_STRING={}", version);
}
