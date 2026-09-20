//! Workspace-local operations must not require the host's global wsp state.

use std::collections::BTreeMap;
use std::fs;
use std::process::Command;

use wsp_core::workspace::{self, Metadata};

const WSP: &str = env!("CARGO_BIN_EXE_wsp");
const WSP_CONFINEMENT_TEST_REQUIRE_BWRAP: &str = "WSP_CONFINEMENT_TEST_REQUIRE_BWRAP";

#[derive(Debug, Eq, PartialEq)]
enum TreeEntry {
    Directory,
    File(Vec<u8>),
    Symlink(std::path::PathBuf),
}

/// Capture all persistent state below a fixture root.  Tests use this for
/// authorities that a workspace-local invocation must leave alone; checking a
/// single known file would miss newly-created state beside it.
fn snapshot_tree(root: &std::path::Path) -> BTreeMap<std::path::PathBuf, TreeEntry> {
    fn visit(
        root: &std::path::Path,
        path: &std::path::Path,
        entries: &mut BTreeMap<std::path::PathBuf, TreeEntry>,
    ) {
        let metadata = fs::symlink_metadata(path).unwrap();
        let relative = path.strip_prefix(root).unwrap().to_path_buf();
        let kind = metadata.file_type();
        if kind.is_dir() {
            entries.insert(relative, TreeEntry::Directory);
            for child in fs::read_dir(path).unwrap() {
                visit(root, &child.unwrap().path(), entries);
            }
        } else if kind.is_symlink() {
            entries.insert(relative, TreeEntry::Symlink(fs::read_link(path).unwrap()));
        } else {
            entries.insert(relative, TreeEntry::File(fs::read(path).unwrap()));
        }
    }

    let mut entries = BTreeMap::new();
    if root.exists() {
        visit(root, root, &mut entries);
    }
    entries
}

/// `wsp --json` emits one formatted JSON value per command.  Sandbox journeys
/// run several commands in one shell, so deserialize the resulting JSON stream
/// rather than assuming each value occupies a single line.
fn json_stream(bytes: &[u8]) -> Vec<serde_json::Value> {
    serde_json::Deserializer::from_slice(bytes)
        .into_iter()
        .map(|value| value.unwrap())
        .collect()
}

#[test]
fn describe_updates_a_mounted_workspace_without_global_state() {
    let tmp = tempfile::tempdir().unwrap();
    let global_state = tmp.path().join("no-global-state");
    let workspace_dir = tmp.path().join("mounted-workspace");
    fs::create_dir(&workspace_dir).unwrap();

    workspace::save_metadata(
        &workspace_dir,
        &Metadata {
            version: 0,
            name: "mounted-workspace".into(),
            branch: "feature/portable".into(),
            repos: BTreeMap::new(),
            created: chrono::Utc::now(),
            description: None,
            last_used: None,
            created_from: None,
            dirs: BTreeMap::new(),
            config: None,
            setup_commands: BTreeMap::new(),
        },
    )
    .unwrap();

    let output = Command::new(WSP)
        .args(["--json", "describe", "portable workspace"])
        .current_dir(&workspace_dir)
        .env("XDG_DATA_HOME", &global_state)
        .env("HOME", tmp.path())
        .env("USERPROFILE", tmp.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "wsp describe failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["context"]["mode"], "workspace_local");
    assert_eq!(
        json["context"]["workspace"],
        workspace_dir.display().to_string()
    );
    assert!(
        !global_state.exists(),
        "workspace-local describe must not create global state"
    );
    assert_eq!(
        workspace::load_metadata(&workspace_dir)
            .unwrap()
            .description
            .as_deref(),
        Some("portable workspace")
    );
}

fn empty_workspace(root: &std::path::Path) -> std::path::PathBuf {
    let directory = root.join("mounted location");
    fs::create_dir(&directory).unwrap();
    workspace::save_metadata(
        &directory,
        &Metadata {
            version: 0,
            name: "original-name".into(),
            branch: "feature/local".into(),
            repos: BTreeMap::new(),
            created: chrono::Utc::now(),
            description: None,
            last_used: None,
            created_from: None,
            dirs: BTreeMap::new(),
            config: None,
            setup_commands: BTreeMap::new(),
        },
    )
    .unwrap();
    directory
}

fn isolated_command(workspace: &std::path::Path, root: &std::path::Path) -> Command {
    let mut command = Command::new(WSP);
    command
        .current_dir(workspace)
        .env("XDG_DATA_HOME", root.join("absent-global"))
        .env("HOME", root.join("absent-home"))
        .env("USERPROFILE", root.join("absent-home"))
        .env_remove("WSP_PWD")
        .env_remove("WSP_SHELL");
    command
}

#[test]
fn portable_reads_leave_workspace_and_global_state_unchanged() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = empty_workspace(temp.path());
    let manifest = fs::read(workspace.join(workspace::METADATA_FILE)).unwrap();
    let commands: &[&[&str]] = &[
        &[],
        &["st"],
        &["st", "original-name"],
        &["diff"],
        &["diff", "original-name"],
        &["log"],
        &["repo", "ls"],
        &["exec", "--", "git", "status"],
    ];
    for args in commands {
        for json in [false, true] {
            let mut command = isolated_command(&workspace, temp.path());
            command.args(*args);
            // exec's `--` belongs to its child; global flags precede the command.
            if json {
                command = isolated_command(&workspace, temp.path());
                command.arg("--json").args(*args);
            }
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{args:?}, json={json}: {} {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            if json {
                serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap();
            }
            assert!(
                !temp.path().join("absent-global").exists(),
                "{args:?} created global state"
            );
            assert!(
                !temp.path().join("absent-home").exists(),
                "{args:?} created home state"
            );
            assert_eq!(
                fs::read(workspace.join(workspace::METADATA_FILE)).unwrap(),
                manifest
            );
            assert_eq!(
                fs::read_dir(&workspace).unwrap().count(),
                1,
                "{args:?} created workspace files"
            );
        }
    }
}

#[test]
fn explicit_matching_name_updates_mounted_root_but_different_name_fails() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = empty_workspace(temp.path());
    let output = isolated_command(&workspace, temp.path())
        .args([
            "--json",
            "describe",
            "original-name",
            "--",
            "mounted description",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(
        workspace::load_metadata(&workspace)
            .unwrap()
            .description
            .as_deref(),
        Some("mounted description")
    );
    let output = isolated_command(&workspace, temp.path())
        .args([
            "--json",
            "describe",
            "another-workspace",
            "--",
            "wrong description",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(
        workspace::load_metadata(&workspace)
            .unwrap()
            .description
            .as_deref(),
        Some("mounted description")
    );
}

#[test]
fn workspace_local_mutation_does_not_touch_a_sibling_with_the_same_name() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = empty_workspace(temp.path());
    let sibling_root = temp.path().join("sibling-mount");
    fs::create_dir(&sibling_root).unwrap();
    let sibling = empty_workspace(&sibling_root);
    fs::write(sibling.join("sibling-sentinel"), "must remain untouched\n").unwrap();
    let sibling_before = snapshot_tree(&sibling);

    let output = isolated_command(&workspace, temp.path())
        .args([
            "--json",
            "describe",
            "original-name",
            "--",
            "only the mounted workspace changes",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "workspace-local describe failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        workspace::load_metadata(&workspace)
            .unwrap()
            .description
            .as_deref(),
        Some("only the mounted workspace changes")
    );
    assert_eq!(
        snapshot_tree(&sibling),
        sibling_before,
        "a same-name sibling mount must not be selected or modified"
    );
}

#[test]
fn corrupt_global_config_fails_before_local_mutation_and_completion_still_works() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = empty_workspace(temp.path());
    let global = temp.path().join("absent-global/wsp");
    fs::create_dir_all(&global).unwrap();
    fs::write(global.join("config.yaml"), "repos: [unterminated").unwrap();
    let output = isolated_command(&workspace, temp.path())
        .args(["--json", "describe", "must not be saved"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let error: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        error["error"]
            .as_str()
            .unwrap()
            .contains("global configuration")
    );
    assert_eq!(
        workspace::load_metadata(&workspace).unwrap().description,
        None
    );
    let output = isolated_command(&workspace, temp.path())
        .args(["completion", "bash"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("wsp"));
}

#[cfg(unix)]
#[test]
fn workspace_local_mutation_tolerates_a_permission_denied_global_store() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let workspace = empty_workspace(temp.path());
    let denied_xdg = temp.path().join("denied-xdg");
    let global = denied_xdg.join("wsp");
    fs::create_dir_all(&global).unwrap();
    let sentinel = global.join("config.yaml");
    fs::write(&sentinel, "sentinel global configuration\n").unwrap();
    fs::create_dir_all(global.join("mirrors/host/owner/repo.git")).unwrap();
    fs::write(
        global.join("mirrors/host/owner/repo.git/HEAD"),
        "sentinel mirror state\n",
    )
    .unwrap();
    let global_before = snapshot_tree(&denied_xdg);
    fs::set_permissions(&denied_xdg, fs::Permissions::from_mode(0o000)).unwrap();

    // This is intentionally a same-identity child rather than a parent-side
    // metadata check. A privileged test runner would bypass chmod and make
    // this assertion fail, requiring CI to run this fixture unprivileged.
    let unreadable = Command::new("cat").arg(&sentinel).output().unwrap();
    assert!(
        !unreadable.status.success(),
        "the child identity can still read the supposedly denied global store"
    );
    let unwritable = Command::new("touch")
        .arg(global.join("must-not-create"))
        .output()
        .unwrap();
    assert!(
        !unwritable.status.success(),
        "the child identity can still write the supposedly denied global store"
    );

    let output = Command::new(WSP)
        .current_dir(&workspace)
        .env("XDG_DATA_HOME", &denied_xdg)
        .env("HOME", temp.path().join("unavailable-home"))
        .env("USERPROFILE", temp.path().join("unavailable-home"))
        .args(["--json", "describe", "works with denied global state"])
        .output()
        .unwrap();

    fs::set_permissions(&denied_xdg, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(
        output.status.success(),
        "workspace-local describe failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        snapshot_tree(&denied_xdg),
        global_before,
        "workspace-local operation must not alter any denied global state"
    );
    assert_eq!(
        workspace::load_metadata(&workspace)
            .unwrap()
            .description
            .as_deref(),
        Some("works with denied global state")
    );
}

/// Some local development environments deliberately disable unprivileged user
/// namespaces. A one-shot Bubblewrap probe distinguishes that unavailable
/// test facility from a failure of the product operation below; CI Linux has
/// user namespaces enabled and therefore gates the full fixture.
#[cfg(target_os = "linux")]
fn add_optional_lib64_bind(command: &mut Command) {
    if std::path::Path::new("/lib64").exists() {
        command.args(["--ro-bind", "/lib64", "/lib64"]);
    }
}

#[cfg(target_os = "linux")]
fn bubblewrap_is_available() -> bool {
    // A missing user-namespace facility is convenient for local development,
    // but CI must never turn the confinement proof into a silent skip.
    let require_bwrap = std::env::var_os(WSP_CONFINEMENT_TEST_REQUIRE_BWRAP).is_some();
    let mut probe_command = Command::new("bwrap");
    probe_command.args([
        "--die-with-parent",
        "--unshare-user",
        "--uid",
        "0",
        "--gid",
        "0",
        "--tmpfs",
        "/",
        "--dir",
        "/runtime",
        "--ro-bind",
        "/bin/true",
        "/runtime/true",
        "--ro-bind",
        "/usr/lib",
        "/usr/lib",
        "--ro-bind",
        "/lib",
        "/lib",
    ]);
    add_optional_lib64_bind(&mut probe_command);
    let probe = match probe_command.args(["--", "/runtime/true"]).output() {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            assert!(
                !require_bwrap,
                "{WSP_CONFINEMENT_TEST_REQUIRE_BWRAP} is set but bwrap is unavailable; \
                 install bubblewrap or unset the variable."
            );
            eprintln!("skipping bubblewrap confinement fixture: bwrap is unavailable");
            return false;
        }
        Err(error) => panic!("could not start bubblewrap confinement probe: {error}"),
    };
    if !probe.status.success() {
        let diagnostic = String::from_utf8_lossy(&probe.stderr);
        let unavailable = [
            "Operation not permitted",
            "No permissions to create new namespace",
            "user namespaces are disabled",
        ];
        if unavailable
            .iter()
            .any(|message| diagnostic.contains(message))
        {
            assert!(
                !require_bwrap,
                "{WSP_CONFINEMENT_TEST_REQUIRE_BWRAP} is set but bubblewrap cannot create \
                 a user namespace: {diagnostic}"
            );
            eprintln!(
                "skipping bubblewrap confinement fixture: user namespaces are unavailable: {diagnostic}"
            );
            return false;
        }
        panic!("bubblewrap confinement probe failed: {diagnostic}");
    }

    true
}

/// Start a fresh filesystem root containing the workspace and a deliberately
/// narrow, read-only runtime. Host home directories, `/var`, arbitrary `/tmp`
/// content, and global data stores are never mounted into this process.
#[cfg(target_os = "linux")]
fn confined_workspace_command(workspace: &std::path::Path, empty_tmp: &std::path::Path) -> Command {
    let mut command = Command::new("bwrap");
    command
        .args([
            "--die-with-parent",
            "--unshare-user",
            "--uid",
            "0",
            "--gid",
            "0",
            "--tmpfs",
            "/",
            "--dir",
            "/runtime",
            "--dir",
            "/runtime/bin",
            "--ro-bind",
        ])
        .arg(WSP)
        .args([
            "/runtime/wsp",
            "--ro-bind",
            "/bin/sh",
            "/runtime/bin/sh",
            "--ro-bind",
            "/usr/bin/git",
            "/runtime/bin/git",
            "--ro-bind",
            "/usr/lib/git-core",
            "/runtime/git-core",
            "--ro-bind",
            "/usr/lib",
            "/usr/lib",
            "--ro-bind",
            "/lib",
            "/lib",
        ]);
    add_optional_lib64_bind(&mut command);
    command
        .args(["--dev", "/dev", "--ro-bind"])
        .arg(empty_tmp)
        .args(["/tmp", "--bind"])
        .arg(workspace)
        .args([
            "/mnt",
            "--chdir",
            "/mnt",
            "--setenv",
            "PATH",
            "/runtime/bin",
            "--setenv",
            "GIT_EXEC_PATH",
            "/runtime/git-core",
            "--setenv",
            "TMPDIR",
            "/mnt/.wsp-tmp",
            "--setenv",
            "XDG_DATA_HOME",
            "/tmp/global-state",
            "--setenv",
            "HOME",
            "/tmp/home",
            "--setenv",
            "USERPROFILE",
            "/tmp/home",
        ]);
    command
}

/// Verify the isolation boundary that agents use in Linux sandboxes. The
/// journey includes normal workspace reads, JSON doctor diagnostics, and a
/// direct-origin fetch/sync. The workspace is the only writable host-backed
/// mount: the global store, a same-name sibling, and the host root remain
/// unavailable to the child.
#[cfg(target_os = "linux")]
#[test]
fn workspace_local_journey_is_confined_to_its_bind_mount() {
    if !bubblewrap_is_available() {
        return;
    }

    let temp = tempfile::tempdir().unwrap();
    let remotes = temp.path().join("transport-remotes");
    create_remote(&remotes, "acme", "widgets");
    let daemon = git_daemon(&remotes);
    let workspace = add_remote_locally(temp.path(), &remote_url(&daemon, "acme", "widgets"));
    upstream_commit(
        &remotes.join("acme/widgets.git"),
        temp.path(),
        "available-only-through-direct-origin.txt",
    );

    let global = temp.path().join("host-global-state");
    let sibling_root = temp.path().join("host-sibling");
    let empty_tmp = temp.path().join("empty-sandbox-tmp");
    fs::create_dir_all(&global).unwrap();
    fs::create_dir_all(&sibling_root).unwrap();
    fs::create_dir(&empty_tmp).unwrap();
    let global_sentinel = global.join("must-not-read-or-write");
    let sibling_sentinel = sibling_root.join("must-not-read-or-write");
    fs::write(&global_sentinel, "global sentinel\n").unwrap();
    fs::write(&sibling_sentinel, "sibling sentinel\n").unwrap();
    let global_before = snapshot_tree(&global);
    let sibling_before = snapshot_tree(&sibling_root);

    let output = confined_workspace_command(&workspace, &empty_tmp)
        .args([
            "--",
            "/runtime/bin/sh",
            "-ceu",
            "test ! -e /etc; test ! -e /home; test ! -e /proc; test ! -e /var; test ! -e \"$1\"; test ! -e \"$2\"; \"$3\" --json st; \"$3\" --json repo ls; set +e; \"$3\" --json doctor; doctor_status=$?; set -e; test \"$doctor_status\" -eq 0 -o \"$doctor_status\" -eq 1; \"$3\" --json repo fetch --prune; exec \"$3\" --json sync",
            "confinement-check",
        ])
        .arg(&global_sentinel)
        .arg(&sibling_sentinel)
        .arg("/runtime/wsp")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "bubblewrap workspace-local journey failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let json = json_stream(&output.stdout);
    assert_eq!(json.len(), 5, "one JSON result per supported command");
    for result in &json {
        assert_eq!(result["context"]["mode"], "workspace_local", "{result}");
    }
    assert_eq!(json[3]["repos"][0]["transport"], "direct", "{json:?}");
    assert_eq!(json[4]["repos"][0]["transport"], "direct", "{json:?}");
    assert_eq!(snapshot_tree(&global), global_before);
    assert_eq!(snapshot_tree(&sibling_root), sibling_before);
    assert_eq!(
        fs::read_to_string(workspace.join("widgets/available-only-through-direct-origin.txt"))
            .unwrap(),
        "from upstream\n"
    );
}

/// Read-only workspace mounts are a supported agent deployment shape.  This
/// uses Bubblewrap rather than chmod so the child cannot bypass the restriction
/// through its host identity.  macOS and Windows have different sandbox APIs;
/// their portable-read coverage stays in the platform-neutral test below.
#[cfg(target_os = "linux")]
#[test]
fn workspace_local_reads_work_from_a_read_only_bind_mount() {
    if !bubblewrap_is_available() {
        return;
    }

    let temp = tempfile::tempdir().unwrap();
    let workspace = populated_workspace(temp.path());
    let empty_tmp = temp.path().join("empty-sandbox-tmp");
    fs::create_dir(&empty_tmp).unwrap();
    let metadata = fs::read(workspace.join(workspace::METADATA_FILE)).unwrap();
    let index = fs::read(workspace.join("alpha/.git/index")).unwrap();

    let mut command = confined_workspace_command(&workspace, &empty_tmp);
    command
        .args(["--ro-bind"])
        .arg(&workspace)
        .args([
            "/mnt",
            "--setenv",
            "GIT_NO_LAZY_FETCH",
            "1",
            "--",
            "/runtime/bin/sh",
            "-ceu",
            "\"$1\" --json st; \"$1\" --json repo ls; set +e; \"$1\" --json doctor; doctor_status=$?; set -e; test \"$doctor_status\" -eq 0 -o \"$doctor_status\" -eq 1",
            "read-only-check",
        ])
        .arg("/runtime/wsp");
    let output = command.output().unwrap();

    assert!(
        output.status.success(),
        "bubblewrap read-only workspace commands failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let json = json_stream(&output.stdout);
    assert_eq!(json.len(), 3, "one JSON result per read-only command");
    for result in json {
        assert_eq!(result["context"]["mode"], "workspace_local", "{result}");
    }
    assert_eq!(
        fs::read(workspace.join(workspace::METADATA_FILE)).unwrap(),
        metadata
    );
    assert_eq!(fs::read(workspace.join("alpha/.git/index")).unwrap(), index);
}

#[test]
fn isolated_fetch_and_sync_use_clone_origin_for_a_populated_workspace() {
    let temp = tempfile::tempdir().unwrap();
    let remotes = temp.path().join("transport-remotes");
    create_remote(&remotes, "acme", "widgets");
    let daemon = git_daemon(&remotes);
    let url = remote_url(&daemon, "acme", "widgets");
    let workspace = empty_workspace(temp.path());
    let added = isolated_command(&workspace, temp.path())
        .args(["--json", "repo", "add", &url])
        .output()
        .unwrap();
    assert!(
        added.status.success(),
        "add: {}",
        String::from_utf8_lossy(&added.stderr)
    );
    let added_json: serde_json::Value = serde_json::from_slice(&added.stdout).unwrap();
    assert_eq!(added_json["context"]["mode"], "workspace_local");
    upstream_commit(
        &remotes.join("acme/widgets.git"),
        temp.path(),
        "fetched.txt",
    );

    let output = isolated_command(&workspace, temp.path())
        .args(["--json", "repo", "fetch", "--prune"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "fetch stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let fetch: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(fetch["repos"][0]["ok"], true, "{fetch}");
    let fetch_stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        fetch_stderr.contains("Fetching 1 repo(s)..."),
        "{fetch_stderr}"
    );
    assert!(fetch_stderr.contains("ok    widgets"), "{fetch_stderr}");
    assert!(
        !temp.path().join("absent-global").exists(),
        "local fetch created global state"
    );

    let output = isolated_command(&workspace, temp.path())
        .args(["--json", "sync"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "sync: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let sync: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(sync["repos"][0]["status"], "ok", "{sync}");
    let sync_stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        sync_stderr.contains("Fetching 1 repo(s)..."),
        "{sync_stderr}"
    );
    assert!(sync_stderr.contains("ok    widgets"), "{sync_stderr}");
    assert!(workspace.join("widgets/fetched.txt").is_file());
    assert!(!temp.path().join("absent-global").exists());
}

#[test]
fn isolated_repo_rm_blocks_dirty_clone_then_force_removes_workspace_member() {
    let temp = tempfile::tempdir().unwrap();
    let remotes = temp.path().join("remove-remotes");
    create_remote(&remotes, "acme", "widgets");
    let daemon = git_daemon(&remotes);
    let workspace = add_remote_locally(temp.path(), &remote_url(&daemon, "acme", "widgets"));
    let identity = "127.0.0.1/acme/widgets".to_string();
    fs::write(workspace.join("widgets/dirty.txt"), "dirty\n").unwrap();

    let output = isolated_command(&workspace, temp.path())
        .args(["--json", "repo", "rm", "widgets"])
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "dirty removal unexpectedly succeeded"
    );
    assert!(workspace.join("widgets").is_dir());
    assert!(
        workspace::load_metadata(&workspace)
            .unwrap()
            .repos
            .contains_key(&identity)
    );

    let output = isolated_command(&workspace, temp.path())
        .args(["--json", "repo", "rm", "--force", "widgets"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "force remove: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!workspace.join("widgets").exists());
    assert!(
        !workspace::load_metadata(&workspace)
            .unwrap()
            .repos
            .contains_key(&identity)
    );
    assert!(!temp.path().join("absent-global").exists());
}

#[test]
fn isolated_repo_rm_preserves_member_when_direct_origin_refresh_fails() {
    let temp = tempfile::tempdir().unwrap();
    let remotes = temp.path().join("failed-refresh-remotes");
    create_remote(&remotes, "acme", "widgets");
    let daemon = git_daemon(&remotes);
    let workspace = add_remote_locally(temp.path(), &remote_url(&daemon, "acme", "widgets"));
    let identity = "127.0.0.1/acme/widgets".to_string();
    let clone = workspace.join("widgets");
    git(
        &clone,
        &[
            "remote",
            "set-url",
            "origin",
            "/definitely/missing/wsp-upstream.git",
        ],
    );

    let output = isolated_command(&workspace, temp.path())
        .args(["repo", "rm", "widgets"])
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "removal with inaccessible origin succeeded"
    );
    assert!(workspace.join("widgets").is_dir());
    assert!(
        workspace::load_metadata(&workspace)
            .unwrap()
            .repos
            .contains_key(&identity)
    );
    assert!(!temp.path().join("absent-global").exists());
}

fn git_output(dir: &std::path::Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn git(dir: &std::path::Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A real ordinary clone fixture, rather than hand-written repository-shaped
/// directories. This makes the portable read checks exercise Git's branch,
/// remote, status, diff, log, exec, and doctor paths together.
fn populated_workspace(root: &std::path::Path) -> std::path::PathBuf {
    let workspace_dir = empty_workspace(root);
    let repo_dir = workspace_dir.join("alpha");
    fs::create_dir(&repo_dir).unwrap();
    git(&repo_dir, &["init", "--initial-branch=feature/local"]);
    git(&repo_dir, &["config", "user.email", "test@test.local"]);
    git(&repo_dir, &["config", "user.name", "Test"]);
    git(&repo_dir, &["config", "commit.gpgsign", "false"]);
    fs::write(repo_dir.join("committed.txt"), "committed\n").unwrap();
    git(&repo_dir, &["add", "committed.txt"]);
    git(&repo_dir, &["commit", "-m", "portable fixture"]);
    git(
        &repo_dir,
        &["remote", "add", "origin", "https://test.local/u/alpha.git"],
    );
    // Make this an otherwise-complete promisor fixture whose remote cannot be
    // contacted.  Portable reads run with GIT_NO_LAZY_FETCH below, proving they
    // do not turn an inspection into a network-backed object lookup.
    git(&repo_dir, &["config", "remote.origin.promisor", "true"]);
    git(
        &repo_dir,
        &["config", "remote.origin.partialclonefilter", "blob:none"],
    );
    git(&repo_dir, &["config", "extensions.partialClone", "origin"]);
    fs::write(repo_dir.join("committed.txt"), "modified\n").unwrap();

    let mut metadata = workspace::load_metadata(&workspace_dir).unwrap();
    metadata.repos.insert(
        "test.local/u/alpha".into(),
        Some(workspace::WorkspaceRepoRef {
            r#ref: String::new(),
            url: Some("https://test.local/u/alpha.git".into()),
        }),
    );
    workspace::save_metadata(&workspace_dir, &metadata).unwrap();
    // Doctor validates generated agent guidance too. Generate it before taking
    // the snapshot: a read-only portable command must not repair it.
    wsp_core::agentmd::update(&workspace_dir, &metadata).unwrap();
    workspace_dir
}

#[test]
fn populated_portable_reads_exec_and_doctor_use_only_mounted_workspace() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = populated_workspace(temp.path());
    let global = temp.path().join("absent-global");
    let home = temp.path().join("absent-home");
    let metadata = fs::read(workspace.join(workspace::METADATA_FILE)).unwrap();
    let index_path = workspace.join("alpha/.git/index");
    let index = fs::read(&index_path).unwrap();
    let mut before: Vec<_> = fs::read_dir(&workspace)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    before.sort();

    // Each command has an observable real-clone result, so a route that merely
    // accepts empty metadata cannot satisfy this test. JSON parsing is checked
    // separately because agents consume that contract directly.
    let checks: &[(&[&str], &str)] = &[
        (&[], "alpha"),
        (&["st"], "1 modified"),
        (&["diff", "--", "--name-only"], "committed.txt"),
        (&["log", "--", "-1"], "portable fixture"),
        (&["repo", "ls"], "alpha"),
        (
            &["exec", "--", "git", "rev-parse", "--show-toplevel"],
            "alpha",
        ),
        (&["doctor"], "Checking workspace"),
    ];
    for (args, expected) in checks {
        let output = isolated_command(&workspace, temp.path())
            .env("GIT_NO_LAZY_FETCH", "1")
            .args(*args)
            .output()
            .unwrap();
        // Doctor returns nonzero when it reports a diagnostic warning. It is
        // still a supported local command, so assert its actual report rather
        // than treating warnings as an invocation failure.
        if args != &["doctor"] {
            assert!(
                output.status.success(),
                "wsp {args:?} failed:\nstdout: {}\nstderr: {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(combined.contains(expected), "wsp {args:?}: {combined}");
        assert!(!global.exists(), "wsp {args:?} created global state");
        assert!(!home.exists(), "wsp {args:?} created HOME state");
    }

    for args in [
        &["st"][..],
        &["diff", "--", "--name-only"],
        &["log", "--", "-1"],
        &["repo", "ls"],
    ] {
        let output = isolated_command(&workspace, temp.path())
            .arg("--json")
            .env("GIT_NO_LAZY_FETCH", "1")
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "wsp --json {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            json["context"]["mode"], "workspace_local",
            "{args:?}: {json}"
        );
        assert!(!global.exists(), "wsp --json {args:?} created global state");
        assert!(!home.exists(), "wsp --json {args:?} created HOME state");
    }

    let output = isolated_command(&workspace, temp.path())
        .env("GIT_NO_LAZY_FETCH", "1")
        .args(["--json", "doctor"])
        .output()
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["context"]["mode"], "workspace_local", "{json}");
    assert!(
        json["checks"].as_array().unwrap().iter().any(|check| {
            check["check"] == "global-state-availability"
                && check["scope"] == "global"
                && check["details"]["availability"] == "absent"
                && check["details"]["checks_skipped"] == true
        }),
        "workspace-local doctor must expose that it inspected only the mounted workspace: {json}"
    );
    assert!(
        json["checks"].as_array().unwrap().iter().any(|check| {
            check["check"] == "unregistered-repos"
                && check["status"] == "ok"
                && check["details"]["identities"]
                    .as_array()
                    .is_some_and(|identities| {
                        identities.iter().any(|id| id == "test.local/u/alpha")
                    })
        }),
        "workspace-local doctor must accept the clone origin as the membership authority: {json}"
    );

    assert_eq!(
        fs::read(workspace.join(workspace::METADATA_FILE)).unwrap(),
        metadata,
        "portable reads must not rewrite membership metadata"
    );
    assert_eq!(
        fs::read(&index_path).unwrap(),
        index,
        "portable Git reads must not refresh .git/index"
    );
    assert!(
        !workspace.join("alpha/.git/index.lock").exists(),
        "portable Git reads must not leave an index lock"
    );
    let mut after: Vec<_> = fs::read_dir(&workspace)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    after.sort();
    assert_eq!(
        after, before,
        "portable reads must not create workspace files"
    );
}

#[test]
fn local_unregistered_members_require_a_matching_origin_identity() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = populated_workspace(temp.path());
    let mut metadata = workspace::load_metadata(&workspace).unwrap();
    let member = metadata.repos.remove("test.local/u/alpha").unwrap();
    metadata.repos.insert("test.local/u/other".into(), member);
    metadata
        .dirs
        .insert("test.local/u/other".into(), "alpha".into());
    workspace::save_metadata(&workspace, &metadata).unwrap();

    let mismatch = isolated_command(&workspace, temp.path())
        .args(["--json", "doctor"])
        .output()
        .unwrap();
    assert!(!mismatch.status.success());
    let mismatch: serde_json::Value = serde_json::from_slice(&mismatch.stdout).unwrap();
    assert!(
        mismatch["checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| { check["check"] == "identity-match" && check["status"] == "error" }),
        "{mismatch}"
    );

    let mut metadata = workspace::load_metadata(&workspace).unwrap();
    let member = metadata.repos.remove("test.local/u/other").unwrap();
    metadata.repos.insert("test.local/u/alpha".into(), member);
    metadata.dirs.clear();
    workspace::save_metadata(&workspace, &metadata).unwrap();
    git(&workspace.join("alpha"), &["remote", "remove", "origin"]);

    let missing = isolated_command(&workspace, temp.path())
        .args(["--json", "doctor"])
        .output()
        .unwrap();
    assert!(!missing.status.success());
    let missing: serde_json::Value = serde_json::from_slice(&missing.stdout).unwrap();
    assert!(
        missing["checks"].as_array().unwrap().iter().any(|check| {
            check["check"] == "origin-remote-exists" && check["status"] == "error"
        }),
        "{missing}"
    );
}

/// A tiny local Git daemon lets these real-binary tests use URLs which are
/// valid `wsp` identities without depending on an external network service.
struct GitDaemon {
    child: std::process::Child,
    url_base: String,
}

impl Drop for GitDaemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn git_daemon(root: &std::path::Path) -> GitDaemon {
    use std::net::{TcpListener, TcpStream};
    use std::time::Duration;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let mut child = Command::new("git")
        .args([
            "daemon",
            "--reuseaddr",
            "--export-all",
            &format!("--base-path={}", root.display()),
            "--listen=127.0.0.1",
            &format!("--port={port}"),
            root.to_str().unwrap(),
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    for _ in 0..50 {
        if TcpStream::connect_timeout(
            &format!("127.0.0.1:{port}").parse().unwrap(),
            Duration::from_millis(20),
        )
        .is_ok()
        {
            return GitDaemon {
                child,
                url_base: format!("git://127.0.0.1:{port}"),
            };
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = child.kill();
    let _ = child.wait();
    panic!("git daemon did not start on port {port}");
}

fn remote_url(daemon: &GitDaemon, owner: &str, repo: &str) -> String {
    format!("{}/{owner}/{repo}.git", daemon.url_base)
}

fn add_remote_locally(root: &std::path::Path, url: &str) -> std::path::PathBuf {
    let workspace = empty_workspace(root);
    let output = isolated_command(&workspace, root)
        .args(["repo", "add", url])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "local add: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    workspace
}

fn upstream_commit(upstream: &std::path::Path, root: &std::path::Path, file: &str) {
    let writer = root.join(format!("writer-{file}"));
    let output = Command::new("git")
        .args([
            "clone",
            upstream.to_str().unwrap(),
            writer.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    git(&writer, &["config", "user.email", "test@example.invalid"]);
    git(&writer, &["config", "user.name", "Test"]);
    git(&writer, &["config", "commit.gpgsign", "false"]);
    fs::write(writer.join(file), "from upstream\n").unwrap();
    git(&writer, &["add", file]);
    git(&writer, &["commit", "-m", file]);
    git(&writer, &["push", "origin", "main"]);
}

fn create_remote(root: &std::path::Path, owner: &str, repo: &str) {
    let bare = root.join(owner).join(format!("{repo}.git"));
    fs::create_dir_all(bare.parent().unwrap()).unwrap();
    wsp_core::testutil::setup_bare_repo(&bare);
    let source = tempfile::tempdir().unwrap();
    git(source.path(), &["init", "--initial-branch=main"]);
    wsp_core::testutil::local_commit(source.path(), "README.md", repo);
    git(
        source.path(),
        &["remote", "add", "origin", bare.to_str().unwrap()],
    );
    git(source.path(), &["push", "origin", "main"]);
}

fn host_command(workspace: &std::path::Path, root: &std::path::Path) -> Command {
    let mut command = Command::new(WSP);
    command
        .current_dir(workspace)
        .env("XDG_DATA_HOME", root.join("host-data"))
        .env("HOME", root.join("host-home"))
        .env("USERPROFILE", root.join("host-home"))
        .env_remove("WSP_PWD")
        .env_remove("WSP_SHELL");
    command
}

fn host_config(root: &std::path::Path) -> std::path::PathBuf {
    let data = root.join("host-data/wsp");
    wsp_core::config::Config::default()
        .save_to(&data.join("config.yaml"))
        .unwrap();
    data
}

fn json_command_allow_failure(command: &mut Command, args: &[&str]) -> serde_json::Value {
    let output = command.args(["--json"]).args(args).output().unwrap();
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "wsp {args:?} did not produce JSON: {error}; stdout: {}; stderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn json_command(command: &mut Command, args: &[&str]) -> serde_json::Value {
    let output = command.args(["--json"]).args(args).output().unwrap();
    assert!(
        output.status.success(),
        "wsp {args:?} failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn host_fetch_and_sync_reject_a_wrong_identity_mirror_without_falling_back_to_origin() {
    let temp = tempfile::tempdir().unwrap();
    let remotes = temp.path().join("remotes");
    create_remote(&remotes, "acme", "widgets");
    create_remote(&remotes, "other", "impostor");
    let daemon = git_daemon(&remotes);
    let widgets_url = remote_url(&daemon, "acme", "widgets");
    let impostor_url = remote_url(&daemon, "other", "impostor");
    let workspace = add_remote_locally(temp.path(), &widgets_url);
    let clone = workspace.join("widgets");

    // Make origin observably ahead. A fallback to the clone's origin would
    // advance refs/remotes/origin/main and let sync update the worktree.
    upstream_commit(
        &remotes.join("acme/widgets.git"),
        temp.path(),
        "only-at-real-origin.txt",
    );
    let main_before = git_output(&clone, &["rev-parse", "main"]);
    let remote_before = git_output(&clone, &["rev-parse", "refs/remotes/origin/main"]);

    let data = host_config(temp.path());
    let expected_mirror = data.join("mirrors/127.0.0.1/acme/widgets.git");
    fs::create_dir_all(expected_mirror.parent().unwrap()).unwrap();
    let output = Command::new("git")
        .args([
            "clone",
            "--mirror",
            &impostor_url,
            expected_mirror.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "creating impostor mirror failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let fetched = json_command_allow_failure(
        &mut host_command(&workspace, temp.path()),
        &["repo", "fetch", "--prune"],
    );
    assert_eq!(fetched["repos"][0]["ok"], false, "{fetched}");
    assert!(
        fetched["repos"][0]["error"]
            .as_str()
            .is_some_and(|error| error.contains("does not match workspace repository")),
        "{fetched}"
    );
    assert_eq!(git_output(&clone, &["rev-parse", "main"]), main_before);
    assert_eq!(
        git_output(&clone, &["rev-parse", "refs/remotes/origin/main"]),
        remote_before,
        "fetch must not fall back to origin after selecting a mismatched mirror"
    );

    let synced = json_command_allow_failure(&mut host_command(&workspace, temp.path()), &["sync"]);
    assert_eq!(synced["repos"][0]["status"], "failed", "{synced}");
    assert!(
        synced["repos"][0]["error"]
            .as_str()
            .is_some_and(|error| error.contains("does not match workspace repository")),
        "{synced}"
    );
    assert_eq!(git_output(&clone, &["rev-parse", "main"]), main_before);
    assert_eq!(
        git_output(&clone, &["rev-parse", "refs/remotes/origin/main"]),
        remote_before,
        "sync must not fall back to origin after selecting a mismatched mirror"
    );
    assert!(
        !clone.join("only-at-real-origin.txt").exists(),
        "sync must not update from origin after rejecting the expected mirror"
    );
}

#[test]
fn repo_fetch_rejects_a_clone_refspec_that_writes_local_main() {
    let temp = tempfile::tempdir().unwrap();
    let remotes = temp.path().join("remotes");
    create_remote(&remotes, "acme", "widgets");
    let daemon = git_daemon(&remotes);
    let workspace = add_remote_locally(temp.path(), &remote_url(&daemon, "acme", "widgets"));
    let clone = workspace.join("widgets");
    upstream_commit(
        &remotes.join("acme/widgets.git"),
        temp.path(),
        "would-overwrite-main.txt",
    );
    let main_before = git_output(&clone, &["rev-parse", "main"]);

    git(&clone, &["config", "--unset-all", "remote.origin.fetch"]);
    git(
        &clone,
        &[
            "config",
            "--add",
            "remote.origin.fetch",
            "+refs/heads/main:refs/heads/main",
        ],
    );

    let fetched = json_command_allow_failure(
        &mut isolated_command(&workspace, temp.path()),
        &["repo", "fetch", "--prune"],
    );
    assert_eq!(fetched["repos"][0]["ok"], false, "{fetched}");
    assert!(
        fetched["repos"][0]["error"]
            .as_str()
            .is_some_and(|error| error.contains("writes outside refs/remotes/origin/")),
        "{fetched}"
    );
    assert_eq!(
        git_output(&clone, &["rev-parse", "main"]),
        main_before,
        "wsp repo fetch must not let a clone-controlled refspec advance main"
    );
    assert!(
        !clone.join("would-overwrite-main.txt").exists(),
        "an unsafe fetch refspec must not update the checkout"
    );
}

#[test]
fn repo_fetch_never_follows_symbolic_tracking_refs() {
    let temp = tempfile::tempdir().unwrap();
    let remotes = temp.path().join("remotes");
    create_remote(&remotes, "acme", "widgets");
    let daemon = git_daemon(&remotes);
    let workspace = add_remote_locally(temp.path(), &remote_url(&daemon, "acme", "widgets"));
    let clone = workspace.join("widgets");

    git(&clone, &["checkout", "main"]);
    wsp_core::testutil::local_commit(&clone, "local-only.txt", "unpublished local work");
    let main_before = git_output(&clone, &["rev-parse", "main"]);
    git(&clone, &["branch", "protected-local", &main_before]);
    upstream_commit(
        &remotes.join("acme/widgets.git"),
        temp.path(),
        "upstream-only.txt",
    );
    let upstream_main = git_output(
        &remotes.join("acme/widgets.git"),
        &["rev-parse", "refs/heads/main"],
    );

    // A tracking ref is developer-controlled clone state. If update-ref
    // dereferences it, importing a remote ref can reset a checked-out branch;
    // pruning a stale one can delete an unrelated local branch.
    git(
        &clone,
        &[
            "symbolic-ref",
            "refs/remotes/origin/main",
            "refs/heads/main",
        ],
    );
    git(
        &clone,
        &[
            "symbolic-ref",
            "refs/remotes/origin/stale",
            "refs/heads/protected-local",
        ],
    );
    let fetched = json_command(
        &mut isolated_command(&workspace, temp.path()),
        &["repo", "fetch", "--prune"],
    );
    assert_eq!(fetched["repos"][0]["transport"], "direct", "{fetched}");
    assert_eq!(git_output(&clone, &["rev-parse", "HEAD"]), main_before);
    assert_eq!(git_output(&clone, &["rev-parse", "main"]), main_before);
    assert_eq!(
        git_output(&clone, &["rev-parse", "protected-local"]),
        main_before
    );
    assert_eq!(git_output(&clone, &["status", "--porcelain"]), "");
    assert!(clone.join("local-only.txt").exists());
    assert_eq!(
        git_output(&clone, &["rev-parse", "refs/remotes/origin/main"]),
        upstream_main
    );
    assert!(
        !Command::new("git")
            .args(["symbolic-ref", "-q", "refs/remotes/origin/main"])
            .current_dir(&clone)
            .status()
            .unwrap()
            .success(),
        "the refreshed tracking ref must replace its symbolic ref itself"
    );
    assert!(
        !Command::new("git")
            .args([
                "show-ref",
                "--verify",
                "--quiet",
                "refs/remotes/origin/stale"
            ])
            .current_dir(&clone)
            .status()
            .unwrap()
            .success(),
        "prune must remove the symbolic tracking ref, not its referent"
    );
}

#[test]
fn repo_fetch_prune_preserves_tracking_refs_outside_the_refspec_destination() {
    let temp = tempfile::tempdir().unwrap();
    let remotes = temp.path().join("remotes");
    create_remote(&remotes, "acme", "widgets");
    let daemon = git_daemon(&remotes);
    let workspace = add_remote_locally(temp.path(), &remote_url(&daemon, "acme", "widgets"));
    let clone = workspace.join("widgets");
    let main = git_output(&clone, &["rev-parse", "refs/remotes/origin/main"]);
    git(&clone, &["update-ref", "refs/remotes/origin/other", &main]);
    git(&clone, &["config", "--unset-all", "remote.origin.fetch"]);
    git(
        &clone,
        &[
            "config",
            "--add",
            "remote.origin.fetch",
            "+refs/heads/main:refs/remotes/origin/main",
        ],
    );
    upstream_commit(
        &remotes.join("acme/widgets.git"),
        temp.path(),
        "main-only-refresh.txt",
    );

    let fetched = json_command(
        &mut isolated_command(&workspace, temp.path()),
        &["repo", "fetch", "--prune"],
    );
    assert_eq!(fetched["repos"][0]["ok"], true, "{fetched}");
    assert_eq!(
        git_output(&clone, &["rev-parse", "refs/remotes/origin/other"]),
        main,
        "--prune must only remove refs selected by the configured destination"
    );
}

#[test]
fn repo_fetch_prune_requires_a_nonempty_wildcard_match() {
    let temp = tempfile::tempdir().unwrap();
    let remotes = temp.path().join("remotes");
    create_remote(&remotes, "acme", "widgets");
    let daemon = git_daemon(&remotes);
    let workspace = add_remote_locally(temp.path(), &remote_url(&daemon, "acme", "widgets"));
    let clone = workspace.join("widgets");
    let main = git_output(&clone, &["rev-parse", "refs/remotes/origin/main"]);
    git(&clone, &["update-ref", "refs/remotes/origin/x", &main]);
    git(&clone, &["config", "--unset-all", "remote.origin.fetch"]);
    git(
        &clone,
        &[
            "config",
            "--add",
            "remote.origin.fetch",
            "+refs/heads/x*:refs/remotes/origin/x*x",
        ],
    );

    let fetched = json_command(
        &mut isolated_command(&workspace, temp.path()),
        &["repo", "fetch", "--prune"],
    );
    assert_eq!(fetched["repos"][0]["ok"], true, "{fetched}");
    assert_eq!(
        git_output(&clone, &["rev-parse", "refs/remotes/origin/x"]),
        main,
        "an empty wildcard substitution must not select a tracking ref"
    );
}

#[test]
fn repeated_direct_fetches_do_not_leave_temporary_pack_protection() {
    let temp = tempfile::tempdir().unwrap();
    let remotes = temp.path().join("remotes");
    create_remote(&remotes, "acme", "widgets");
    let daemon = git_daemon(&remotes);
    let workspace = add_remote_locally(temp.path(), &remote_url(&daemon, "acme", "widgets"));
    let clone = workspace.join("widgets");

    for file in ["first-upstream-change.txt", "second-upstream-change.txt"] {
        upstream_commit(&remotes.join("acme/widgets.git"), temp.path(), file);
        let fetched = json_command(
            &mut isolated_command(&workspace, temp.path()),
            &["repo", "fetch"],
        );
        assert_eq!(fetched["repos"][0]["ok"], true, "{fetched}");
        let keep_files: Vec<_> = fs::read_dir(clone.join(".git/objects/pack"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "keep"))
            .collect();
        assert!(
            keep_files.is_empty(),
            "a completed direct fetch must release its temporary pack protection: {keep_files:?}"
        );
    }
}

#[test]
fn repo_fetch_preserves_safe_custom_remote_tracking_refspecs_when_pruning() {
    let temp = tempfile::tempdir().unwrap();
    let remotes = temp.path().join("remotes");
    create_remote(&remotes, "acme", "widgets");
    let daemon = git_daemon(&remotes);
    let workspace = add_remote_locally(temp.path(), &remote_url(&daemon, "acme", "widgets"));
    let clone = workspace.join("widgets");
    let old_main = git_output(&clone, &["rev-parse", "refs/remotes/origin/main"]);

    // This is a safe refspec: it writes only beneath origin's remote-tracking
    // namespace.  `--prune` must not treat it as stale merely because the
    // direct-fetch stage used a heads-only refspec of its own.
    git(
        &clone,
        &[
            "config",
            "--add",
            "remote.origin.fetch",
            "+refs/heads/main:refs/remotes/origin/special-main",
        ],
    );
    git(
        &clone,
        &["update-ref", "refs/remotes/origin/special-main", &old_main],
    );
    upstream_commit(
        &remotes.join("acme/widgets.git"),
        temp.path(),
        "custom-refspec-must-survive-prune.txt",
    );

    let fetched = json_command(
        &mut isolated_command(&workspace, temp.path()),
        &["repo", "fetch", "--prune"],
    );
    assert_eq!(fetched["repos"][0]["ok"], true, "{fetched}");
    assert_eq!(
        git_output(&clone, &["rev-parse", "refs/remotes/origin/special-main"]),
        git_output(&clone, &["rev-parse", "refs/remotes/origin/main"]),
        "the configured safe tracking refspec must be refreshed, not pruned"
    );
}

#[test]
fn repo_fetch_uses_the_literal_origin_when_clone_config_has_an_insteadof_rule() {
    let temp = tempfile::tempdir().unwrap();
    let remotes = temp.path().join("remotes");
    create_remote(&remotes, "acme", "widgets");
    let daemon = git_daemon(&remotes);
    let url = remote_url(&daemon, "acme", "widgets");
    let workspace = add_remote_locally(temp.path(), &url);
    let clone = workspace.join("widgets");
    upstream_commit(
        &remotes.join("acme/widgets.git"),
        temp.path(),
        "available-only-at-the-captured-origin.txt",
    );

    // `git remote get-url` would apply this mapping. The workspace transport
    // reads the literal remote.origin.url and performs its network fetch in a
    // config-free stage, so a clone-local rewrite cannot redirect it.
    git(
        &clone,
        &["config", "url.https://invalid.example/.insteadOf", &url],
    );

    let mut command = isolated_command(&workspace, temp.path());
    // Environment config has higher precedence than every config file, and
    // Git applies it to literal fetch URLs too. The staging process must not
    // inherit this rewrite either.
    command
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "url.https://invalid.example/.insteadOf")
        .env("GIT_CONFIG_VALUE_0", &url);
    // Git copies template files during `git init`. A template config is
    // therefore another way an inherited environment could insert a URL
    // rewrite into the private fetch stage.
    let template = temp.path().join("git-template");
    fs::create_dir(&template).unwrap();
    fs::write(
        template.join("config"),
        format!("[url \"https://invalid.example/\"]\n\tinsteadOf = {url}\n"),
    )
    .unwrap();
    command.env("GIT_TEMPLATE_DIR", &template);
    // A normal host Git command must honour this standard override, but a
    // workspace-local direct refresh has an explicit captured endpoint and
    // must ignore its transport rewrite.
    let global = temp.path().join("inherited-gitconfig");
    fs::write(
        &global,
        format!("[url \"https://invalid.example/\"]\n\tinsteadOf = {url}\n"),
    )
    .unwrap();
    command.env("GIT_CONFIG_GLOBAL", global);
    let fetched = json_command(&mut command, &["repo", "fetch", "--prune"]);
    assert_eq!(fetched["repos"][0]["ok"], true, "{fetched}");
    assert_eq!(fetched["repos"][0]["transport"], "direct", "{fetched}");
    assert_ne!(
        git_output(&clone, &["rev-parse", "main"]),
        git_output(&clone, &["rev-parse", "refs/remotes/origin/main"]),
        "the staged fetch must update the actual origin tracking ref"
    );
}

#[test]
fn repo_fetch_ignores_inherited_git_repository_and_object_routing() {
    let temp = tempfile::tempdir().unwrap();
    let remotes = temp.path().join("remotes");
    create_remote(&remotes, "acme", "widgets");
    let daemon = git_daemon(&remotes);
    let url = remote_url(&daemon, "acme", "widgets");
    let workspace = add_remote_locally(temp.path(), &url);
    let clone = workspace.join("widgets");

    // A parent Git process can export GIT_DIR.  Make that alternate repository
    // look plausible enough that an unsanitized preflight would accept it, then
    // retain a ref outside the namespace wsp is allowed to modify.
    let outside = temp.path().join("outside-git");
    git(temp.path(), &["init", outside.to_str().unwrap()]);
    git(&outside, &["config", "user.email", "test@example.com"]);
    git(&outside, &["config", "user.name", "Test User"]);
    git(&outside, &["config", "commit.gpgsign", "false"]);
    fs::write(outside.join("sentinel"), "outside\n").unwrap();
    git(&outside, &["add", "sentinel"]);
    git(&outside, &["commit", "-m", "sentinel"]);
    git(&outside, &["config", "remote.origin.url", &url]);
    git(
        &outside,
        &[
            "config",
            "remote.origin.fetch",
            "+refs/heads/*:refs/remotes/origin/*",
        ],
    );
    let sentinel_oid = git_output(&outside, &["rev-parse", "HEAD"]);
    git(
        &outside,
        &["update-ref", "refs/wsp-sentinel", &sentinel_oid],
    );
    upstream_commit(
        &remotes.join("acme/widgets.git"),
        temp.path(),
        "must-reach-the-workspace-clone.txt",
    );

    let mut command = isolated_command(&workspace, temp.path());
    command.env("GIT_DIR", outside.join(".git"));
    let fetched = json_command(&mut command, &["repo", "fetch"]);
    assert_eq!(fetched["repos"][0]["ok"], true, "{fetched}");
    assert_eq!(
        git_output(&outside, &["rev-parse", "refs/wsp-sentinel"]),
        sentinel_oid,
        "an inherited GIT_DIR must not redirect the workspace refresh"
    );
    assert_ne!(
        git_output(&clone, &["rev-parse", "main"]),
        git_output(&clone, &["rev-parse", "refs/remotes/origin/main"]),
        "the real workspace clone must receive the fetched tracking ref"
    );

    // index-pack normally honours GIT_OBJECT_DIRECTORY.  It must not create
    // pack files outside the mounted workspace while importing a staged fetch.
    let outside_objects = temp.path().join("outside-objects");
    fs::create_dir_all(outside_objects.join("pack")).unwrap();
    let outside_before = snapshot_tree(&outside_objects);
    upstream_commit(
        &remotes.join("acme/widgets.git"),
        temp.path(),
        "must-not-write-outside-objects.txt",
    );
    let mut command = isolated_command(&workspace, temp.path());
    command.env("GIT_OBJECT_DIRECTORY", &outside_objects);
    let fetched = json_command(&mut command, &["repo", "fetch"]);
    assert_eq!(fetched["repos"][0]["ok"], true, "{fetched}");
    assert_eq!(
        snapshot_tree(&outside_objects),
        outside_before,
        "an inherited GIT_OBJECT_DIRECTORY must not receive imported objects"
    );
    assert_ne!(
        git_output(&clone, &["rev-parse", "main"]),
        git_output(&clone, &["rev-parse", "refs/remotes/origin/main"]),
        "the real clone must remain the import target"
    );
}

#[test]
fn host_fetch_uses_origin_when_the_mirror_store_is_unavailable() {
    let temp = tempfile::tempdir().unwrap();
    let remotes = temp.path().join("remotes");
    create_remote(&remotes, "acme", "widgets");
    let daemon = git_daemon(&remotes);
    let workspace = add_remote_locally(temp.path(), &remote_url(&daemon, "acme", "widgets"));
    let clone = workspace.join("widgets");
    upstream_commit(
        &remotes.join("acme/widgets.git"),
        temp.path(),
        "available-from-origin.txt",
    );
    host_config(temp.path());

    // host_config deliberately does not create the mirror store. The host can
    // still use the member's direct origin without creating a global mirror.
    let fetched = json_command(
        &mut host_command(&workspace, temp.path()),
        &["repo", "fetch", "--prune"],
    );
    assert_eq!(fetched["repos"][0]["ok"], true, "{fetched}");
    assert_eq!(fetched["context"]["mode"], "host", "{fetched}");
    assert_eq!(fetched["repos"][0]["transport"], "direct", "{fetched}");
    assert_eq!(
        fetched["repos"][0]["fallback_reason"], "mirror_store_absent",
        "{fetched}"
    );
    assert_ne!(
        git_output(&clone, &["rev-parse", "main"]),
        git_output(&clone, &["rev-parse", "refs/remotes/origin/main"]),
        "fetch should update only the remote-tracking ref before sync"
    );
    assert!(
        !temp.path().join("host-data/wsp/mirrors").exists(),
        "a mirror-unavailable host fetch must not create the shared mirror store"
    );
    let synced = json_command(&mut host_command(&workspace, temp.path()), &["sync"]);
    assert_eq!(synced["context"]["mode"], "host", "{synced}");
    assert_eq!(synced["repos"][0]["transport"], "direct", "{synced}");
    assert_eq!(
        synced["repos"][0]["fallback_reason"], "mirror_store_absent",
        "{synced}"
    );
}

#[test]
fn local_add_survives_host_retry_registry_registration_and_isolation_return() {
    let temp = tempfile::tempdir().unwrap();
    let remotes = temp.path().join("remotes");
    create_remote(&remotes, "acme", "api");
    let daemon = git_daemon(&remotes);
    let url = remote_url(&daemon, "acme", "api");
    let identity = "127.0.0.1/acme/api";
    let workspace = empty_workspace(temp.path());

    let local = json_command(
        &mut isolated_command(&workspace, temp.path()),
        &["repo", "add", &url],
    );
    assert_eq!(local["repos"][0]["identity"], identity);
    assert_eq!(local["repos"][0]["clone"], "created");
    assert_eq!(local["repos"][0]["transport"], "direct");
    let clone = workspace.join("api");
    let origin_before = wsp_core::git::remote_get_url(&clone, "origin").unwrap();
    wsp_core::testutil::local_commit(&clone, "local.txt", "kept across contexts");
    let metadata_before = fs::read(workspace.join(workspace::METADATA_FILE)).unwrap();

    let data = host_config(temp.path());
    let config_before = fs::read(data.join("config.yaml")).unwrap();
    let retried = json_command(
        &mut host_command(&workspace, temp.path()),
        &["repo", "add", &url],
    );
    assert_eq!(retried["repos"][0]["clone"], "already_present");
    assert_eq!(retried["repos"][0]["membership"], "unchanged");
    assert_eq!(fs::read(data.join("config.yaml")).unwrap(), config_before);
    assert!(
        !data.join("mirrors").exists(),
        "retry must not create a mirror"
    );
    assert_eq!(
        fs::read(workspace.join(workspace::METADATA_FILE)).unwrap(),
        metadata_before
    );
    assert_eq!(
        wsp_core::git::remote_get_url(&clone, "origin").unwrap(),
        origin_before
    );
    assert_eq!(
        fs::read_to_string(clone.join("local.txt")).unwrap(),
        "kept across contexts"
    );
    let short_retry = json_command(
        &mut host_command(&workspace, temp.path()),
        &["repo", "add", "api"],
    );
    assert_eq!(short_retry["repos"][0]["clone"], "already_present");
    assert_eq!(fs::read(data.join("config.yaml")).unwrap(), config_before);
    assert!(
        !data.join("mirrors").exists(),
        "short retry must not create a mirror"
    );

    let listed = json_command(
        &mut isolated_command(&workspace, temp.path()),
        &["repo", "ls"],
    );
    assert_eq!(listed["repos"][0]["identity"], identity);
    // Parse and successfully run a populated status from isolation after the host retry.
    let _status = json_command(&mut isolated_command(&workspace, temp.path()), &["st"]);

    // Registration is explicit host infrastructure. It must leave the ordinary
    // clone and its workspace membership exactly as the isolated invocation made them.
    json_command(
        &mut host_command(&workspace, temp.path()),
        &["registry", "add", &url],
    );
    let cfg = wsp_core::config::Config::load_from(&data.join("config.yaml")).unwrap();
    assert_eq!(cfg.upstream_url(identity), Some(url.as_str()));
    assert!(data.join("mirrors/127.0.0.1/acme/api.git").is_dir());
    assert_eq!(
        fs::read(workspace.join(workspace::METADATA_FILE)).unwrap(),
        metadata_before
    );
    assert_eq!(
        wsp_core::git::remote_get_url(&clone, "origin").unwrap(),
        origin_before
    );
    assert_eq!(
        fs::read_to_string(clone.join("local.txt")).unwrap(),
        "kept across contexts"
    );
}

#[test]
fn workspace_local_p0_release_journey_preserves_authorities_across_contexts() {
    // This is deliberately one real-binary journey. It makes the transitions
    // that a sandboxed agent and its host take visible on the same workspace,
    // rather than proving each state from a fresh fixture.
    let temp = tempfile::tempdir().unwrap();
    let remotes = temp.path().join("remotes");
    create_remote(&remotes, "acme", "api");
    create_remote(&remotes, "other", "api");
    let daemon = git_daemon(&remotes);
    let acme_url = remote_url(&daemon, "acme", "api");
    let other_url = remote_url(&daemon, "other", "api");
    let acme_identity = "127.0.0.1/acme/api";
    let other_identity = "127.0.0.1/other/api";
    let workspace = empty_workspace(temp.path());
    let data = host_config(temp.path());
    fs::create_dir_all(temp.path().join("host-home/dev/workspaces")).unwrap();

    // Start as a normal host process. The empty workspace is visible through
    // its mounted root, while the global registry remains the host authority.
    let host_initial = json_command(&mut host_command(&workspace, temp.path()), &["st"]);
    assert!(
        host_initial["repos"].as_array().unwrap().is_empty(),
        "{host_initial}"
    );
    let host_before_local = snapshot_tree(&data);

    // A sandbox sees no global state, so a URL add must publish only inside the
    // workspace and preserve the URL as the clone's origin.
    let local_add = json_command(
        &mut isolated_command(&workspace, temp.path()),
        &["repo", "add", &acme_url],
    );
    assert_eq!(
        local_add["context"]["mode"], "workspace_local",
        "{local_add}"
    );
    assert_eq!(local_add["repos"][0]["identity"], acme_identity);
    assert_eq!(local_add["repos"][0]["clone"], "created");
    assert_eq!(local_add["repos"][0]["transport"], "direct");
    assert_eq!(snapshot_tree(&data), host_before_local);

    let acme_clone = workspace.join("api");
    let acme_origin = wsp_core::git::remote_get_url(&acme_clone, "origin").unwrap();
    assert_eq!(acme_origin, acme_url);
    wsp_core::testutil::local_commit(&acme_clone, "local.txt", "kept through every transition");
    let metadata_after_local_add = fs::read(workspace.join(workspace::METADATA_FILE)).unwrap();
    let acme_after_local_add = snapshot_tree(&acme_clone);

    // Host retry must recognize the workspace member before any host-side
    // registration, mirror, setup, or discovery work.
    let host_retry = json_command(
        &mut host_command(&workspace, temp.path()),
        &["repo", "add", &acme_url],
    );
    assert_eq!(host_retry["repos"][0]["clone"], "already_present");
    assert_eq!(host_retry["repos"][0]["membership"], "unchanged");
    assert_eq!(snapshot_tree(&data), host_before_local);
    assert_eq!(
        fs::read(workspace.join(workspace::METADATA_FILE)).unwrap(),
        metadata_after_local_add
    );
    assert_eq!(snapshot_tree(&acme_clone), acme_after_local_add);

    // Continue from isolation after the host retry. Reads and direct refreshes
    // use the workspace/clone authorities and cannot alter host state.
    let local_list = json_command(
        &mut isolated_command(&workspace, temp.path()),
        &["repo", "ls"],
    );
    assert_eq!(
        local_list["context"]["mode"], "workspace_local",
        "{local_list}"
    );
    assert_eq!(local_list["repos"][0]["identity"], acme_identity);
    let local_fetch = json_command(
        &mut isolated_command(&workspace, temp.path()),
        &["repo", "fetch", "--prune"],
    );
    assert_eq!(
        local_fetch["context"]["mode"], "workspace_local",
        "{local_fetch}"
    );
    assert_eq!(
        local_fetch["repos"][0]["transport"], "direct",
        "{local_fetch}"
    );
    assert_eq!(snapshot_tree(&data), host_before_local);
    assert_eq!(
        wsp_core::git::remote_get_url(&acme_clone, "origin").unwrap(),
        acme_origin
    );
    assert_eq!(
        fs::read_to_string(acme_clone.join("local.txt")).unwrap(),
        "kept through every transition"
    );
    let acme_before_registration = snapshot_tree(&acme_clone);

    // Registry registration is an explicit host operation. It creates host
    // infrastructure without normalizing the existing workspace clone.
    let registered = json_command(
        &mut host_command(&workspace, temp.path()),
        &["registry", "add", &acme_url],
    );
    assert_eq!(registered["ok"], true, "{registered}");
    let config_after_registration =
        wsp_core::config::Config::load_from(&data.join("config.yaml")).unwrap();
    assert_eq!(
        config_after_registration.upstream_url(acme_identity),
        Some(acme_url.as_str())
    );
    assert!(data.join("mirrors/127.0.0.1/acme/api.git").is_dir());
    assert_eq!(
        fs::read(workspace.join(workspace::METADATA_FILE)).unwrap(),
        metadata_after_local_add
    );
    assert_eq!(snapshot_tree(&acme_clone), acme_before_registration);

    // A mixed host request carries the already-present member alongside a new
    // same-basename URL. The existing fixed mapping stays `api`; only the new
    // member receives host registration/mirror effects and its disambiguator.
    let mixed = json_command(
        &mut host_command(&workspace, temp.path()),
        &["repo", "add", &acme_url, &other_url],
    );
    let mixed_repos = mixed["repos"].as_array().unwrap();
    let existing = mixed_repos
        .iter()
        .find(|repo| repo["identity"] == acme_identity)
        .unwrap();
    let new = mixed_repos
        .iter()
        .find(|repo| repo["identity"] == other_identity)
        .unwrap();
    assert_eq!(existing["clone"], "already_present", "{mixed}");
    assert_eq!(existing["membership"], "unchanged", "{mixed}");
    assert_eq!(new["clone"], "created", "{mixed}");
    assert_eq!(new["transport"], "mirror", "{mixed}");
    assert_eq!(snapshot_tree(&acme_clone), acme_before_registration);

    let metadata_after_mixed = workspace::load_metadata(&workspace).unwrap();
    assert_eq!(metadata_after_mixed.repos.len(), 2);
    assert_eq!(metadata_after_mixed.dir_name(acme_identity).unwrap(), "api");
    assert_eq!(
        metadata_after_mixed.dir_name(other_identity).unwrap(),
        "other-api"
    );
    let other_clone = workspace.join("other-api");
    let other_origin = wsp_core::git::remote_get_url(&other_clone, "origin").unwrap();
    assert_eq!(other_origin, other_url);
    let config_after_mixed =
        wsp_core::config::Config::load_from(&data.join("config.yaml")).unwrap();
    assert_eq!(
        config_after_mixed.upstream_url(acme_identity),
        Some(acme_url.as_str())
    );
    assert_eq!(
        config_after_mixed.upstream_url(other_identity),
        Some(other_url.as_str())
    );
    assert!(data.join("mirrors/127.0.0.1/acme/api.git").is_dir());
    assert!(data.join("mirrors/127.0.0.1/other/api.git").is_dir());

    // Doctor may repair host-wide housekeeping, but must not reinterpret valid
    // workspace-local membership, origins, or collision mappings.
    let doctor = json_command_allow_failure(
        &mut host_command(&workspace, temp.path()),
        &["doctor", "--fix"],
    );
    assert!(doctor["checks"].is_array(), "{doctor}");
    let metadata_after_doctor = workspace::load_metadata(&workspace).unwrap();
    assert_eq!(metadata_after_doctor.repos, metadata_after_mixed.repos);
    assert_eq!(metadata_after_doctor.dirs, metadata_after_mixed.dirs);
    assert_eq!(
        wsp_core::git::remote_get_url(&acme_clone, "origin").unwrap(),
        acme_origin
    );
    assert_eq!(
        wsp_core::git::remote_get_url(&other_clone, "origin").unwrap(),
        other_origin
    );
    let config_after_doctor =
        wsp_core::config::Config::load_from(&data.join("config.yaml")).unwrap();
    assert_eq!(
        config_after_doctor.upstream_url(acme_identity),
        Some(acme_url.as_str())
    );
    assert_eq!(
        config_after_doctor.upstream_url(other_identity),
        Some(other_url.as_str())
    );

    // Return to the restricted process after all host activity. It must still
    // see both member mappings and use direct transport without touching the
    // host registry or mirror tree.
    let host_before_return = snapshot_tree(&data);
    let returned_status = json_command(&mut isolated_command(&workspace, temp.path()), &["st"]);
    assert_eq!(
        returned_status["repos"].as_array().unwrap().len(),
        2,
        "{returned_status}"
    );
    let returned_fetch = json_command(
        &mut isolated_command(&workspace, temp.path()),
        &["repo", "fetch", "--prune"],
    );
    assert_eq!(
        returned_fetch["context"]["mode"], "workspace_local",
        "{returned_fetch}"
    );
    assert_eq!(
        returned_fetch["repos"].as_array().unwrap().len(),
        2,
        "{returned_fetch}"
    );
    for repo in returned_fetch["repos"].as_array().unwrap() {
        assert_eq!(repo["transport"], "direct", "{returned_fetch}");
    }
    assert_eq!(snapshot_tree(&data), host_before_return);
    let final_metadata = workspace::load_metadata(&workspace).unwrap();
    assert_eq!(final_metadata.repos, metadata_after_mixed.repos);
    assert_eq!(final_metadata.dirs, metadata_after_mixed.dirs);
    assert_eq!(
        wsp_core::git::remote_get_url(&acme_clone, "origin").unwrap(),
        acme_origin
    );
    assert_eq!(
        wsp_core::git::remote_get_url(&other_clone, "origin").unwrap(),
        other_origin
    );
    assert_eq!(
        fs::read_to_string(acme_clone.join("local.txt")).unwrap(),
        "kept through every transition"
    );
}

#[test]
fn host_add_uses_populated_registered_mirrors_after_offline_batch_refresh() {
    let temp = tempfile::tempdir().unwrap();
    let remotes = temp.path().join("registered-remotes");
    create_remote(&remotes, "acme", "alpha");
    create_remote(&remotes, "acme", "beta");
    let daemon = git_daemon(&remotes);
    let alpha_url = remote_url(&daemon, "acme", "alpha");
    let beta_url = remote_url(&daemon, "acme", "beta");
    let workspace = empty_workspace(temp.path());

    // Register while upstream is reachable so both global mirrors contain a
    // complete clone. The workspace has no members yet.
    host_config(temp.path());
    for url in [&alpha_url, &beta_url] {
        let registered = host_command(&workspace, temp.path())
            .args(["registry", "add", url])
            .output()
            .unwrap();
        assert!(
            registered.status.success(),
            "registry add {url}: {}",
            String::from_utf8_lossy(&registered.stderr)
        );
    }
    // Keep the local mirror content but make its upstream unreachable. The
    // configured URL still identifies the same repository, so this isolates a
    // refresh failure from the already-populated mirror used for cloning.
    let data = temp.path().join("host-data/wsp");
    for name in ["alpha", "beta"] {
        git(
            &data.join(format!("mirrors/127.0.0.1/acme/{name}.git")),
            &[
                "remote",
                "set-url",
                "origin",
                &format!("git://127.0.0.1:1/acme/{name}.git"),
            ],
        );
    }

    let output = host_command(&workspace, temp.path())
        .args(["--json", "repo", "add", &alpha_url, &beta_url])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "host add should clone from populated mirrors after offline refresh:
{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["repos"].as_array().unwrap().len(), 2, "{value}");
    for repo in value["repos"].as_array().unwrap() {
        assert_eq!(repo["clone"], "created", "{repo}");
        assert_eq!(repo["transport"], "mirror", "{repo}");
    }
    assert!(workspace.join("alpha/.git").is_dir());
    assert!(workspace.join("beta/.git").is_dir());

    // Registered members are prefetched as a batch. stderr remains useful for
    // people and leaves stdout as one JSON document for callers.
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Fetching 2 mirrors..."), "{stderr}");
    assert!(stderr.contains("FAIL  127.0.0.1"), "{stderr}");
}

#[test]
fn host_mixed_add_only_registers_and_mirrors_genuinely_new_members() {
    let temp = tempfile::tempdir().unwrap();
    let remotes = temp.path().join("remotes");
    create_remote(&remotes, "acme", "existing");
    create_remote(&remotes, "acme", "new");
    let daemon = git_daemon(&remotes);
    let existing_url = remote_url(&daemon, "acme", "existing");
    let new_url = remote_url(&daemon, "acme", "new");
    let workspace = empty_workspace(temp.path());

    json_command(
        &mut isolated_command(&workspace, temp.path()),
        &["repo", "add", &existing_url],
    );
    let existing_clone = workspace.join("existing");
    wsp_core::testutil::local_commit(&existing_clone, "local.txt", "must survive mixed add");
    let origin = wsp_core::git::remote_get_url(&existing_clone, "origin").unwrap();
    host_config(temp.path());

    let out = json_command(
        &mut host_command(&workspace, temp.path()),
        &["repo", "add", &existing_url, &new_url],
    );
    let repos = out["repos"].as_array().unwrap();
    assert_eq!(repos.len(), 2);
    let existing = repos
        .iter()
        .find(|repo| repo["identity"] == "127.0.0.1/acme/existing")
        .unwrap();
    let new = repos
        .iter()
        .find(|repo| repo["identity"] == "127.0.0.1/acme/new")
        .unwrap();
    assert_eq!(existing["clone"], "already_present");
    assert_eq!(new["clone"], "created");
    assert_eq!(new["transport"], "mirror");

    let data = temp.path().join("host-data/wsp");
    let cfg = wsp_core::config::Config::load_from(&data.join("config.yaml")).unwrap();
    assert!(!cfg.repos.contains_key("127.0.0.1/acme/existing"));
    assert_eq!(
        cfg.upstream_url("127.0.0.1/acme/new"),
        Some(new_url.as_str())
    );
    assert!(!data.join("mirrors/127.0.0.1/acme/existing.git").exists());
    assert!(data.join("mirrors/127.0.0.1/acme/new.git").is_dir());
    assert_eq!(
        wsp_core::git::remote_get_url(&existing_clone, "origin").unwrap(),
        origin
    );
    assert_eq!(
        fs::read_to_string(existing_clone.join("local.txt")).unwrap(),
        "must survive mixed add"
    );
    let metadata = workspace::load_metadata(&workspace).unwrap();
    assert_eq!(metadata.repos.len(), 2);
}

#[test]
fn host_doctor_and_fix_preserve_an_unregistered_workspace_local_clone() {
    let temp = tempfile::tempdir().unwrap();
    let remotes = temp.path().join("remotes");
    create_remote(&remotes, "acme", "api");
    let daemon = git_daemon(&remotes);
    let url = remote_url(&daemon, "acme", "api");
    let identity = "127.0.0.1/acme/api";
    let workspace = add_remote_locally(temp.path(), &url);
    let clone = workspace.join("api");
    wsp_core::testutil::local_commit(&clone, "local.txt", "must survive doctor");
    let origin = wsp_core::git::remote_get_url(&clone, "origin").unwrap();
    let metadata = fs::read(workspace.join(workspace::METADATA_FILE)).unwrap();
    let data = host_config(temp.path());
    fs::create_dir_all(temp.path().join("host-home/dev/workspaces")).unwrap();

    for args in [&["doctor"][..], &["doctor", "--fix"]] {
        // A fresh host fixture deliberately lacks its normal workspaces root,
        // which doctor reports and --fix repairs.  The transition assertion is
        // about the workspace member, so inspect the structured diagnostic
        // instead of treating unrelated host setup guidance as a failure.
        let doctor = json_command_allow_failure(&mut host_command(&workspace, temp.path()), args);
        assert!(
            doctor["checks"].as_array().unwrap().iter().any(|check| {
                check["check"] == "unregistered-repos"
                    && check["status"] == "ok"
                    && check["details"]["identities"]
                        .as_array()
                        .is_some_and(|ids| ids.iter().any(|id| id == identity))
            }),
            "doctor must report workspace-local membership as supported: {doctor}"
        );
    }

    assert_eq!(
        fs::read(workspace.join(workspace::METADATA_FILE)).unwrap(),
        metadata
    );
    assert_eq!(
        wsp_core::git::remote_get_url(&clone, "origin").unwrap(),
        origin
    );
    assert_eq!(
        fs::read_to_string(clone.join("local.txt")).unwrap(),
        "must survive doctor"
    );
    assert!(
        !wsp_core::config::Config::load_from(&data.join("config.yaml"))
            .unwrap()
            .repos
            .contains_key(identity),
        "doctor --fix must not turn an unregistered clone into global registry state"
    );
}

#[test]
fn host_registry_registration_does_not_prevent_isolated_operations() {
    let temp = tempfile::tempdir().unwrap();
    let remotes = temp.path().join("remotes");
    create_remote(&remotes, "acme", "api");
    let daemon = git_daemon(&remotes);
    let url = remote_url(&daemon, "acme", "api");
    let workspace = empty_workspace(temp.path());
    let data = host_config(temp.path());

    json_command(
        &mut host_command(&workspace, temp.path()),
        &["registry", "add", &url],
    );
    let global_before = snapshot_tree(&data);

    let added = json_command(
        &mut isolated_command(&workspace, temp.path()),
        &["repo", "add", &url],
    );
    assert_eq!(added["context"]["mode"], "workspace_local", "{added}");
    assert_eq!(added["repos"][0]["transport"], "direct", "{added}");
    let fetched = json_command(
        &mut isolated_command(&workspace, temp.path()),
        &["repo", "fetch", "--prune"],
    );
    assert_eq!(fetched["context"]["mode"], "workspace_local", "{fetched}");
    assert_eq!(fetched["repos"][0]["transport"], "direct", "{fetched}");
    assert_eq!(
        snapshot_tree(&data),
        global_before,
        "isolated operations must not mutate host registry or mirrors"
    );
}

#[test]
fn same_basename_members_keep_their_mapping_across_host_operations() {
    let temp = tempfile::tempdir().unwrap();
    let remotes = temp.path().join("remotes");
    create_remote(&remotes, "acme", "api");
    create_remote(&remotes, "other", "api");
    let daemon = git_daemon(&remotes);
    let acme_url = remote_url(&daemon, "acme", "api");
    let other_url = remote_url(&daemon, "other", "api");
    let acme_identity = "127.0.0.1/acme/api";
    let other_identity = "127.0.0.1/other/api";
    let workspace = empty_workspace(temp.path());

    json_command(
        &mut isolated_command(&workspace, temp.path()),
        &["repo", "add", &acme_url, &other_url],
    );
    let initial = workspace::load_metadata(&workspace).unwrap();
    // Membership paths are stable: the first member keeps its conventional
    // clone name and the later colliding member receives a disambiguator.
    // Host operations must route by this recorded map rather than recomputing
    // both names from the current registry.
    assert_eq!(initial.dir_name(acme_identity).unwrap(), "api");
    assert_eq!(initial.dir_name(other_identity).unwrap(), "other-api");
    host_config(temp.path());
    fs::create_dir_all(temp.path().join("host-home/dev/workspaces")).unwrap();

    let status = json_command(&mut host_command(&workspace, temp.path()), &["st"]);
    assert_eq!(status["repos"].as_array().unwrap().len(), 2, "{status}");
    let fetched = json_command(
        &mut host_command(&workspace, temp.path()),
        &["repo", "fetch", "--prune"],
    );
    assert_eq!(fetched["repos"].as_array().unwrap().len(), 2, "{fetched}");
    let synced = json_command(&mut host_command(&workspace, temp.path()), &["sync"]);
    assert_eq!(synced["repos"].as_array().unwrap().len(), 2, "{synced}");
    let executed = json_command(
        &mut host_command(&workspace, temp.path()),
        &["exec", "--", "git", "rev-parse", "--show-toplevel"],
    );
    let paths: Vec<_> = executed["repos"]
        .as_array()
        .unwrap()
        .iter()
        .map(|repo| repo["path"].as_str().unwrap())
        .collect();
    assert!(
        paths.iter().any(|path| path.ends_with("/api")),
        "{executed}"
    );
    assert!(
        paths.iter().any(|path| path.ends_with("/other-api")),
        "{executed}"
    );
    // The fixture intentionally leaves the optional branch-prefix unset, so
    // doctor returns a structured warning while still completing all of its
    // workspace checks.
    let doctor =
        json_command_allow_failure(&mut host_command(&workspace, temp.path()), &["doctor"]);
    assert!(
        doctor["checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check["check"] == "unregistered-repos"),
        "{doctor}"
    );

    json_command(
        &mut host_command(&workspace, temp.path()),
        &["repo", "rm", "--force", other_identity],
    );
    let remaining = workspace::load_metadata(&workspace).unwrap();
    assert!(remaining.repos.contains_key(acme_identity));
    assert!(!remaining.repos.contains_key(other_identity));
    assert_eq!(remaining.dir_name(acme_identity).unwrap(), "api");
    assert!(workspace.join("api/.git").is_dir());
    assert!(!workspace.join("other-api").exists());
}
