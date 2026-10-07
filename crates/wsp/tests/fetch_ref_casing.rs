//! Repo add and subsequent mirror propagation must tolerate case-mixed branch directories.
use std::fs;
use std::path::Path;
use std::process::Command;

use wsp_core::config::Config;
use wsp_core::testutil::local_commit;

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().into()
}

#[test]
fn repo_add_then_fetch_prune_handles_case_mixed_branch_directories() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let data_home = temp.path().join("data");
    let workspaces = temp.path().join("workspaces");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir_all(data_home.join("wsp")).unwrap();
    fs::create_dir_all(&workspaces).unwrap();
    git(&source, &["init", "--initial-branch=main"]);
    local_commit(&source, "README.md", "fixture");
    let initial = git(&source, &["rev-parse", "HEAD"]);
    // Writing packed refs retains canonical names independently of the host's
    // filesystem. Git serves these exact names to both initial clone and fetch.
    fs::write(
        source.join(".git/packed-refs"),
        format!(
            "# pack-refs with: peeled fully-peeled sorted\n\
             {initial} refs/heads/Kern/entrypoint-prd\n\
             {initial} refs/heads/kern/exec-race\n\
             {initial} refs/heads/kern/stale\n"
        ),
    )
    .unwrap();
    let global = temp.path().join("gitconfig");
    fs::write(
        &global,
        format!(
            "[url \"file://{}\"]\n\tinsteadOf = git@test.local:acme/widgets.git\n",
            source.display()
        ),
    )
    .unwrap();
    Config {
        workspaces_dir: Some(workspaces.display().to_string()),
        hints: Some(false),
        ..Default::default()
    }
    .save_to(&data_home.join("wsp/config.yaml"))
    .unwrap();
    let wsp = |cwd: &Path, args: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_wsp"))
            .args(args)
            .arg("--json")
            .current_dir(cwd)
            .env("XDG_DATA_HOME", &data_home)
            .env("GIT_CONFIG_GLOBAL", &global)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "wsp {args:?}: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        println!("wsp {}: success", args.join(" "));
        result
    };
    wsp(temp.path(), &["new", "ref-casing", "--empty"]);
    let workspace = workspaces.join("ref-casing");
    let added = wsp(
        &workspace,
        &[
            "repo",
            "add",
            "git@test.local:acme/widgets.git",
            "--no-discover",
        ],
    );
    assert_eq!(added["repos"][0]["clone"], "created", "{added}");
    assert_eq!(added["repos"][0]["membership"], "updated", "{added}");
    let clone = workspace.join("widgets");
    let head = git(&clone, &["rev-parse", "HEAD"]);
    local_commit(&source, "upstream.txt", "updated");
    let advanced = git(&source, &["rev-parse", "HEAD"]);
    fs::write(
        source.join(".git/packed-refs"),
        format!(
            "# pack-refs with: peeled fully-peeled sorted\n\
             {advanced} refs/heads/Kern/entrypoint-prd\n\
             {advanced} refs/heads/kern/exec-race\n"
        ),
    )
    .unwrap();
    for _ in 0..2 {
        let fetched = wsp(&workspace, &["repo", "fetch", "--prune"]);
        assert!(fetched["repos"][0]["error"].is_null(), "{fetched}");
        for branch in ["Kern/entrypoint-prd", "kern/exec-race"] {
            assert_eq!(
                git(
                    &clone,
                    &["rev-parse", &format!("refs/remotes/origin/{branch}")]
                ),
                advanced
            );
        }
        let stale = Command::new("git")
            .args([
                "show-ref",
                "--verify",
                "--quiet",
                "refs/remotes/origin/kern/stale",
            ])
            .current_dir(&clone)
            .status()
            .unwrap();
        assert_eq!(stale.code(), Some(1), "stale tracking ref was not pruned");
        assert_eq!(git(&clone, &["rev-parse", "HEAD"]), head);
        assert_eq!(git(&clone, &["status", "--porcelain"]), "");
    }
    println!(
        "Verified advanced tracking refs, pruned stale refs, and unchanged local HEAD/worktree."
    );
}
