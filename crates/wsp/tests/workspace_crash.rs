//! Real-binary crash recovery tests. Enabled only by `just crash-test`.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use std::{panic, thread};

use wsp_core::config::Config;
use wsp_core::crash_barrier::{self, Message, Operation, Point, Selection};
use wsp_core::workspace::{self, Metadata};

const WSP: &str = env!("CARGO_BIN_EXE_wsp");
const PARENT_DEADLINE: Duration = Duration::from_secs(5);
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Owns an instrumented `wsp` child so a failing assertion cannot leave it
/// paused at a controller barrier or leave a zombie behind.
struct CrashChild(Option<std::process::Child>);

impl CrashChild {
    fn spawn(command: &mut Command) -> Self {
        Self(Some(command.spawn().expect("spawn instrumented wsp child")))
    }

    fn id(&self) -> u32 {
        self.0.as_ref().expect("child already reaped").id()
    }

    fn kill(&mut self) -> std::io::Result<()> {
        self.0.as_mut().expect("child already reaped").kill()
    }

    fn wait(&mut self) -> std::io::Result<std::process::ExitStatus> {
        self.0.as_mut().expect("child already reaped").wait()
    }

    fn wait_with_output(mut self) -> std::io::Result<std::process::Output> {
        self.0
            .take()
            .expect("child already reaped")
            .wait_with_output()
    }
}

impl Drop for CrashChild {
    fn drop(&mut self) {
        let Some(child) = self.0.as_mut() else { return };
        match child.try_wait() {
            Ok(Some(_)) | Err(_) => {}
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

struct Controller {
    listener: TcpListener,
    session: String,
    token: String,
}

impl Controller {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(false).unwrap();
        Self {
            listener,
            session: "crash-session-1".into(),
            token: "crash-token-1".into(),
        }
    }

    fn address(&self) -> String {
        self.listener.local_addr().unwrap().to_string()
    }

    fn accept_hello(&self, child_pid: u32) -> Result<TcpStream, String> {
        self.accept_hello_until(child_pid, PARENT_DEADLINE)
    }

    fn accept_hello_until(&self, child_pid: u32, timeout: Duration) -> Result<TcpStream, String> {
        self.listener
            .set_nonblocking(true)
            .map_err(|error| format!("configure controller listener: {error}"))?;
        let deadline = Instant::now() + timeout;
        let (mut stream, _) = loop {
            match self.listener.accept() {
                Ok(connection) => break connection,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return Err(format!(
                            "timed out after {}s waiting for child {child_pid} crash-barrier hello",
                            timeout.as_secs_f32()
                        ));
                    }
                    std::thread::sleep(POLL_INTERVAL);
                }
                Err(error) => {
                    return Err(format!("accept child crash-barrier connection: {error}"));
                }
            }
        };
        stream
            .set_read_timeout(Some(PARENT_DEADLINE))
            .map_err(|error| format!("set crash-barrier read deadline: {error}"))?;
        stream
            .set_write_timeout(Some(PARENT_DEADLINE))
            .map_err(|error| format!("set crash-barrier write deadline: {error}"))?;
        match crash_barrier::read_message(&mut stream)
            .map_err(|error| format!("read child crash-barrier hello: {error:#}"))?
        {
            Message::Hello {
                version,
                catalog_version,
                session,
                token,
                pid,
            } => {
                if version != 1 || catalog_version != 4 {
                    return Err(
                        "child sent an unsupported crash-barrier protocol/catalog version".into(),
                    );
                }
                if session != self.session || token != self.token {
                    return Err(
                        "child crash-barrier hello did not match this controller session".into(),
                    );
                }
                if pid != child_pid {
                    return Err(format!(
                        "child crash-barrier hello pid {pid} did not match spawned child {child_pid}"
                    ));
                }
            }
            other => return Err(format!("expected child crash-barrier hello, got {other:?}")),
        }
        Ok(stream)
    }

    fn wait_for(
        &self,
        child_pid: u32,
        selected: Selection,
    ) -> (TcpStream, wsp_core::crash_barrier::Reached) {
        let mut stream = self
            .accept_hello(child_pid)
            .expect("validate child crash-barrier hello");
        crash_barrier::write_message(
            &mut stream,
            &Message::Select {
                version: 1,
                session: self.session.clone(),
                token: self.token.clone(),
                selections: vec![selected.clone()],
            },
        )
        .unwrap();
        let Message::Reached(reached) =
            crash_barrier::read_message(&mut stream).expect("read selected crash-barrier event")
        else {
            panic!("expected selected crash barrier event");
        };
        assert_eq!(reached.session, self.session, "reached event session");
        assert_eq!(reached.sequence, 1, "first selected event sequence");
        assert_eq!(
            reached.operation, selected.operation,
            "reached event operation"
        );
        assert_eq!(
            reached.repository, selected.repository,
            "reached event repository"
        );
        assert_eq!(reached.point, selected.point, "reached event point");
        assert_eq!(
            reached.occurrence, selected.occurrence,
            "reached event occurrence"
        );
        (stream, reached)
    }
}

fn empty_workspace(root: &std::path::Path) -> std::path::PathBuf {
    let workspace = root.join("mounted");
    fs::create_dir(&workspace).unwrap();
    workspace::save_metadata(
        &workspace,
        &Metadata {
            version: 0,
            name: "mounted".into(),
            branch: "main".into(),
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
    workspace
}

fn git(path: &std::path::Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn create_remote(root: &std::path::Path) {
    let bare = root.join("acme/api.git");
    fs::create_dir_all(bare.parent().unwrap()).unwrap();
    wsp_core::testutil::setup_bare_repo(&bare);
    let source = tempfile::tempdir().unwrap();
    git(source.path(), &["init", "--initial-branch=main"]);
    wsp_core::testutil::local_commit(source.path(), "README.md", "api");
    git(
        source.path(),
        &["remote", "add", "origin", bare.to_str().unwrap()],
    );
    git(source.path(), &["push", "origin", "main"]);
}

struct GitDaemon(CrashChild, String);
impl Drop for GitDaemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn git_daemon(root: &std::path::Path) -> GitDaemon {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let mut command = Command::new("git");
    command
        .args([
            "daemon",
            "--reuseaddr",
            "--export-all",
            &format!("--base-path={}", root.display()),
            "--listen=127.0.0.1",
            &format!("--port={port}"),
            root.to_str().unwrap(),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = CrashChild::spawn(&mut command);
    for _ in 0..50 {
        if TcpStream::connect_timeout(
            &format!("127.0.0.1:{port}").parse().unwrap(),
            Duration::from_millis(20),
        )
        .is_ok()
        {
            return GitDaemon(child, format!("git://127.0.0.1:{port}"));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = child.kill();
    let _ = child.wait();
    panic!("git daemon did not start");
}

fn isolated_command(workspace: &std::path::Path, root: &std::path::Path) -> Command {
    let mut command = Command::new(WSP);
    command
        .current_dir(workspace)
        .env("XDG_DATA_HOME", root.join("no-global"))
        .env("HOME", root.join("no-home"))
        .env("USERPROFILE", root.join("no-home"))
        .env_remove("WSP_PWD")
        .env_remove("WSP_SHELL");
    command
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

fn assert_startup_protocol_failure(reply: impl FnOnce(&mut TcpStream, &Controller)) {
    let temp = tempfile::tempdir().unwrap();
    let controller = Controller::new();
    let mut command = Command::new(WSP);
    command
        .args(["--json", "st"])
        .env("WSP_TEST_CRASH_ADDR", controller.address())
        .env("WSP_TEST_CRASH_TOKEN", &controller.token)
        .env("WSP_TEST_CRASH_SESSION", &controller.session)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .current_dir(temp.path());
    let child = CrashChild::spawn(&mut command);
    let mut stream = controller
        .accept_hello(child.id())
        .expect("validate child crash-barrier hello");
    reply(&mut stream, &controller);
    drop(stream);
    let output = child.wait_with_output().unwrap();
    assert!(
        !output.status.success(),
        "malformed crash-barrier control input must prevent command dispatch"
    );
    let diagnostic = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        diagnostic.contains("crash-barrier"),
        "expected a crash-barrier initialization error, got: {diagnostic}"
    );
}

#[test]
fn crash_controller_rejects_selection_with_wrong_session() {
    assert_startup_protocol_failure(|stream, controller| {
        crash_barrier::write_message(
            stream,
            &Message::Select {
                version: 1,
                session: "other-session".into(),
                token: controller.token.clone(),
                selections: vec![],
            },
        )
        .unwrap();
    });
}

#[test]
fn crash_controller_rejects_selection_with_wrong_token() {
    assert_startup_protocol_failure(|stream, controller| {
        crash_barrier::write_message(
            stream,
            &Message::Select {
                version: 1,
                session: controller.session.clone(),
                token: "other-token".into(),
                selections: vec![],
            },
        )
        .unwrap();
    });
}

#[test]
fn crash_controller_rejects_eof_before_selection() {
    assert_startup_protocol_failure(|_, _| {});
}

#[test]
fn crash_controller_rejects_a_non_selection_message() {
    assert_startup_protocol_failure(|stream, controller| {
        crash_barrier::write_message(
            stream,
            &Message::Continue {
                session: controller.session.clone(),
                sequence: 1,
            },
        )
        .unwrap();
    });
}

#[test]
fn crash_controller_rejects_an_oversize_frame() {
    assert_startup_protocol_failure(|stream, _| {
        stream.write_all(&(16_384_u32 + 1).to_be_bytes()).unwrap();
        stream.flush().unwrap();
    });
}

#[test]
fn crash_controller_times_out_before_the_child_watchdog() {
    let controller = Controller::new();
    let started = Instant::now();
    let error = controller
        .accept_hello_until(42, Duration::from_millis(50))
        .expect_err("a missing child hello must time out");
    assert!(
        error.contains("timed out"),
        "unexpected timeout error: {error}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "parent deadline must not wait for the child watchdog"
    );
}

#[test]
fn crash_controller_rejects_a_hello_from_the_wrong_process() {
    let controller = Controller::new();
    let mut stream = TcpStream::connect(controller.address()).unwrap();
    crash_barrier::write_message(
        &mut stream,
        &Message::Hello {
            version: 1,
            catalog_version: 4,
            session: controller.session.clone(),
            token: controller.token.clone(),
            pid: 999_999,
        },
    )
    .unwrap();
    let error = controller
        .accept_hello(123_456)
        .expect_err("a different process must not control this child");
    assert!(error.contains("did not match spawned child"));
}

#[test]
fn crash_controller_rejects_an_unselected_reached_event() {
    let controller = Controller::new();
    let address = controller.address();
    let session = controller.session.clone();
    let token = controller.token.clone();
    let child = thread::spawn(move || {
        let mut stream = TcpStream::connect(address).unwrap();
        crash_barrier::write_message(
            &mut stream,
            &Message::Hello {
                version: 1,
                catalog_version: 4,
                session: session.clone(),
                token,
                pid: 123,
            },
        )
        .unwrap();
        let Message::Select { .. } = crash_barrier::read_message(&mut stream).unwrap() else {
            panic!("controller did not select a barrier");
        };
        crash_barrier::write_message(
            &mut stream,
            &Message::Reached(wsp_core::crash_barrier::Reached {
                session,
                sequence: 1,
                operation: Operation::Add,
                repository: "unexpected/repository".into(),
                point: Point::StageCreated,
                occurrence: 1,
                lock_held: false,
            }),
        )
        .unwrap();
    });
    let selected = Selection {
        operation: Operation::Add,
        repository: "expected/repository".into(),
        point: Point::StageCreated,
        occurrence: 1,
    };
    let result = panic::catch_unwind(|| controller.wait_for(123, selected));
    child.join().unwrap();
    assert!(
        result.is_err(),
        "controller accepted an unselected reached event"
    );
}

fn host_config(root: &std::path::Path) -> std::path::PathBuf {
    let data = root.join("host-data/wsp");
    Config::default()
        .save_to(&data.join("config.yaml"))
        .unwrap();
    data
}

fn crash_add_at_membership_commit(
    workspace: &std::path::Path,
    root: &std::path::Path,
    url: &str,
) -> (CrashChild, TcpStream) {
    let controller = Controller::new();
    let mut command = isolated_command(workspace, root);
    command
        .args(["--json", "repo", "add", url])
        .env("WSP_TEST_CRASH_ADDR", controller.address())
        .env("WSP_TEST_CRASH_TOKEN", &controller.token)
        .env("WSP_TEST_CRASH_SESSION", &controller.session)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let child = CrashChild::spawn(&mut command);
    let selected = Selection {
        operation: Operation::Add,
        repository: "127.0.0.1/acme/api".into(),
        point: Point::MembershipCommitted,
        occurrence: 1,
    };
    let (stream, reached) = controller.wait_for(child.id(), selected);
    assert_eq!(reached.point, Point::MembershipCommitted);
    assert!(reached.lock_held);
    (child, stream)
}

fn crash_add_at(
    workspace: &std::path::Path,
    root: &std::path::Path,
    url: &str,
    point: Point,
) -> (CrashChild, TcpStream, wsp_core::crash_barrier::Reached) {
    let controller = Controller::new();
    let mut command = isolated_command(workspace, root);
    command
        .args(["--json", "repo", "add", url])
        .env("WSP_TEST_CRASH_ADDR", controller.address())
        .env("WSP_TEST_CRASH_TOKEN", &controller.token)
        .env("WSP_TEST_CRASH_SESSION", &controller.session)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let child = CrashChild::spawn(&mut command);
    let selected = Selection {
        operation: Operation::Add,
        repository: "127.0.0.1/acme/api".into(),
        point,
        occurrence: 1,
    };
    let (stream, reached) = controller.wait_for(child.id(), selected);
    (child, stream, reached)
}

fn assert_add_retry_succeeds(workspace: &std::path::Path, root: &std::path::Path, url: &str) {
    let recovered = isolated_command(workspace, root)
        .args(["--json", "repo", "add", url])
        .output()
        .unwrap();
    assert!(
        recovered.status.success(),
        "recovery failed: {}",
        String::from_utf8_lossy(&recovered.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&recovered.stdout).unwrap();
    assert!(
        ["created", "adopted"].contains(&result["repos"][0]["clone"].as_str().unwrap()),
        "unexpected recovery result: {result}"
    );
    assert!(
        workspace::load_metadata(workspace)
            .unwrap()
            .repos
            .contains_key("127.0.0.1/acme/api")
    );
}

#[test]
fn killed_after_stage_creation_leaves_no_member_or_final_slot_and_retries() {
    let temp = tempfile::tempdir().unwrap();
    let remotes = temp.path().join("remotes");
    create_remote(&remotes);
    let daemon = git_daemon(&remotes);
    let url = format!("{}/acme/api.git", daemon.1);
    let workspace = empty_workspace(temp.path());
    let (mut child, stream, reached) =
        crash_add_at(&workspace, temp.path(), &url, Point::StageCreated);

    assert_eq!(reached.point, Point::StageCreated);
    assert!(!reached.lock_held);
    assert!(!workspace.join("api").exists());
    assert!(
        workspace::load_metadata(&workspace)
            .unwrap()
            .repos
            .is_empty()
    );
    child.kill().unwrap();
    drop(stream);
    assert!(!child.wait().unwrap().success());

    assert_add_retry_succeeds(&workspace, temp.path(), &url);
}

#[test]
fn killed_after_clone_staging_leaves_no_member_or_final_slot_and_retries() {
    let temp = tempfile::tempdir().unwrap();
    let remotes = temp.path().join("remotes");
    create_remote(&remotes);
    let daemon = git_daemon(&remotes);
    let url = format!("{}/acme/api.git", daemon.1);
    let workspace = empty_workspace(temp.path());
    let (mut child, stream, reached) =
        crash_add_at(&workspace, temp.path(), &url, Point::CloneStaged);

    assert_eq!(reached.point, Point::CloneStaged);
    assert!(!reached.lock_held);
    assert!(!workspace.join("api").exists());
    assert!(
        workspace::load_metadata(&workspace)
            .unwrap()
            .repos
            .is_empty()
    );
    child.kill().unwrap();
    drop(stream);
    assert!(!child.wait().unwrap().success());

    assert_add_retry_succeeds(&workspace, temp.path(), &url);
}

#[test]
fn killed_after_adoption_validation_preserves_the_unrecorded_clone_for_retry() {
    let temp = tempfile::tempdir().unwrap();
    let remotes = temp.path().join("remotes");
    create_remote(&remotes);
    let daemon = git_daemon(&remotes);
    let url = format!("{}/acme/api.git", daemon.1);
    let workspace = empty_workspace(temp.path());
    let (mut publisher, publisher_stream, reached) =
        crash_add_at(&workspace, temp.path(), &url, Point::ClonePublished);
    assert_eq!(reached.point, Point::ClonePublished);
    publisher.kill().unwrap();
    drop(publisher_stream);
    assert!(!publisher.wait().unwrap().success());

    let clone = workspace.join("api");
    fs::write(clone.join("README.md"), "developer edit before adoption\n").unwrap();
    git(&clone, &["config", "user.name", "Developer choice"]);
    let (mut adopter, adopter_stream, reached) =
        crash_add_at(&workspace, temp.path(), &url, Point::AdoptionValidated);
    assert_eq!(reached.point, Point::AdoptionValidated);
    assert!(reached.lock_held);
    assert!(
        workspace::load_metadata(&workspace)
            .unwrap()
            .repos
            .is_empty()
    );
    assert_eq!(
        fs::read_to_string(clone.join("README.md")).unwrap(),
        "developer edit before adoption\n"
    );
    adopter.kill().unwrap();
    drop(adopter_stream);
    assert!(!adopter.wait().unwrap().success());

    assert_add_retry_succeeds(&workspace, temp.path(), &url);
    assert_eq!(
        fs::read_to_string(clone.join("README.md")).unwrap(),
        "developer edit before adoption\n"
    );
    assert_eq!(
        wsp_core::git::run(Some(&clone), &["config", "--get", "user.name"]).unwrap(),
        "Developer choice"
    );
}

#[test]
fn killed_after_clone_publication_is_adopted_by_a_fresh_local_invocation() {
    let temp = tempfile::tempdir().unwrap();
    let remotes = temp.path().join("remotes");
    create_remote(&remotes);
    let daemon = git_daemon(&remotes);
    let url = format!("{}/acme/api.git", daemon.1);
    let workspace = empty_workspace(temp.path());
    let controller = Controller::new();
    let mut command = isolated_command(&workspace, temp.path());
    command
        .args(["--json", "repo", "add", &url])
        .env("WSP_TEST_CRASH_ADDR", controller.address())
        .env("WSP_TEST_CRASH_TOKEN", &controller.token)
        .env("WSP_TEST_CRASH_SESSION", &controller.session)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = CrashChild::spawn(&mut command);
    let selected = Selection {
        operation: Operation::Add,
        repository: "127.0.0.1/acme/api".into(),
        point: Point::ClonePublished,
        occurrence: 1,
    };
    let (stream, reached) = controller.wait_for(child.id(), selected);
    assert_eq!(reached.operation, Operation::Add);
    assert_eq!(reached.repository, "127.0.0.1/acme/api");
    assert_eq!(reached.point, Point::ClonePublished);
    assert!(reached.lock_held);
    let clone = workspace.join("api");
    assert!(
        clone.join(".git").is_dir(),
        "publication must precede the event"
    );
    let interrupted = workspace::load_metadata(&workspace).unwrap();
    assert!(
        interrupted.repos.is_empty(),
        "membership must not precede the event"
    );
    child.kill().unwrap();
    drop(stream);
    let status = child.wait().unwrap();
    assert!(!status.success(), "the selected child must be terminated");

    fs::write(clone.join("README.md"), "developer edit after crash\n").unwrap();
    let recovered = isolated_command(&workspace, temp.path())
        .args(["--json", "repo", "add", &url])
        .output()
        .unwrap();
    assert!(
        recovered.status.success(),
        "recovery failed: {}",
        String::from_utf8_lossy(&recovered.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&recovered.stdout).unwrap();
    assert_eq!(result["repos"][0]["clone"], "adopted");
    assert_eq!(result["repos"][0]["membership"], "updated");
    let recovered_meta = workspace::load_metadata(&workspace).unwrap();
    assert!(recovered_meta.repos.contains_key("127.0.0.1/acme/api"));
    assert_eq!(
        fs::read_to_string(clone.join("README.md")).unwrap(),
        "developer edit after crash\n"
    );
    assert!(!temp.path().join("no-global").exists());
}

#[test]
fn killed_after_membership_commit_retries_locally_without_replaying_the_add() {
    let temp = tempfile::tempdir().unwrap();
    let remotes = temp.path().join("remotes");
    create_remote(&remotes);
    let daemon = git_daemon(&remotes);
    let url = format!("{}/acme/api.git", daemon.1);
    let workspace = empty_workspace(temp.path());
    let (mut child, stream) = crash_add_at_membership_commit(&workspace, temp.path(), &url);

    let clone = workspace.join("api");
    let interrupted = workspace::load_metadata(&workspace).unwrap();
    assert!(interrupted.repos.contains_key("127.0.0.1/acme/api"));
    assert_eq!(interrupted.dir_name("127.0.0.1/acme/api").unwrap(), "api");
    assert!(!workspace.join("AGENTS.md").exists());
    child.kill().unwrap();
    drop(stream);
    assert!(!child.wait().unwrap().success());

    fs::write(clone.join("README.md"), "developer edit after commit\n").unwrap();
    git(&clone, &["config", "user.name", "Developer choice"]);
    let recovered = isolated_command(&workspace, temp.path())
        .args(["--json", "repo", "add", &url])
        .output()
        .unwrap();
    assert!(
        recovered.status.success(),
        "recovery failed: {}",
        String::from_utf8_lossy(&recovered.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&recovered.stdout).unwrap();
    assert_eq!(result["repos"][0]["clone"], "already_present");
    assert_eq!(result["repos"][0]["membership"], "unchanged");
    assert_eq!(result["repos"][0]["guidance"], "updated");
    assert_eq!(
        fs::read_to_string(clone.join("README.md")).unwrap(),
        "developer edit after commit\n"
    );
    assert_eq!(
        wsp_core::git::run(Some(&clone), &["config", "--get", "user.name"]).unwrap(),
        "Developer choice"
    );
    assert!(workspace.join("AGENTS.md").is_file());
    assert!(!temp.path().join("no-global").exists());
}

#[test]
fn killed_after_membership_commit_retries_from_host_without_registering_again() {
    let temp = tempfile::tempdir().unwrap();
    let remotes = temp.path().join("remotes");
    create_remote(&remotes);
    let daemon = git_daemon(&remotes);
    let url = format!("{}/acme/api.git", daemon.1);
    let workspace = empty_workspace(temp.path());
    let (mut child, stream) = crash_add_at_membership_commit(&workspace, temp.path(), &url);
    let clone = workspace.join("api");
    assert!(
        workspace::load_metadata(&workspace)
            .unwrap()
            .repos
            .contains_key("127.0.0.1/acme/api")
    );
    child.kill().unwrap();
    drop(stream);
    assert!(!child.wait().unwrap().success());

    fs::write(
        clone.join("README.md"),
        "developer edit before host retry\n",
    )
    .unwrap();
    let data = host_config(temp.path());
    let config_before = fs::read(data.join("config.yaml")).unwrap();
    let recovered = host_command(&workspace, temp.path())
        .args(["--json", "repo", "add", &url])
        .output()
        .unwrap();
    assert!(
        recovered.status.success(),
        "host recovery failed: {}",
        String::from_utf8_lossy(&recovered.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&recovered.stdout).unwrap();
    assert_eq!(result["repos"][0]["clone"], "already_present");
    assert_eq!(result["repos"][0]["membership"], "unchanged");
    assert_eq!(fs::read(data.join("config.yaml")).unwrap(), config_before);
    assert!(
        !data.join("mirrors").exists(),
        "an existing member must bypass host registration and mirror creation"
    );
    assert_eq!(
        fs::read_to_string(clone.join("README.md")).unwrap(),
        "developer edit before host retry\n"
    );
}

fn crash_add_at_guidance(
    workspace: &std::path::Path,
    root: &std::path::Path,
    url: &str,
    point: Point,
) -> (CrashChild, TcpStream, wsp_core::crash_barrier::Reached) {
    let controller = Controller::new();
    let mut command = isolated_command(workspace, root);
    command
        .args(["--json", "repo", "add", url])
        .env("WSP_TEST_CRASH_ADDR", controller.address())
        .env("WSP_TEST_CRASH_TOKEN", &controller.token)
        .env("WSP_TEST_CRASH_SESSION", &controller.session)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let child = CrashChild::spawn(&mut command);
    let selected = Selection {
        operation: Operation::Add,
        repository: "workspace/mounted".into(),
        point,
        occurrence: 1,
    };
    let (stream, reached) = controller.wait_for(child.id(), selected);
    (child, stream, reached)
}

#[test]
fn description_commit_is_durable_before_the_child_can_be_killed() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = empty_workspace(temp.path());
    workspace::save_metadata(
        &workspace,
        &Metadata {
            description: Some("old description".into()),
            ..workspace::load_metadata(&workspace).unwrap()
        },
    )
    .unwrap();
    let controller = Controller::new();
    let mut command = isolated_command(&workspace, temp.path());
    command
        .args(["--json", "describe", "--", "new", "description"])
        .env("WSP_TEST_CRASH_ADDR", controller.address())
        .env("WSP_TEST_CRASH_TOKEN", &controller.token)
        .env("WSP_TEST_CRASH_SESSION", &controller.session)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = CrashChild::spawn(&mut command);
    let selected = Selection {
        operation: Operation::Describe,
        repository: "workspace/mounted".into(),
        point: Point::DescriptionCommitted,
        occurrence: 1,
    };
    let (stream, reached) = controller.wait_for(child.id(), selected);
    assert!(reached.lock_held);
    assert_eq!(
        workspace::load_metadata(&workspace)
            .unwrap()
            .description
            .as_deref(),
        Some("new description"),
        "metadata replacement must precede description_committed"
    );
    child.kill().unwrap();
    drop(stream);
    assert!(!child.wait().unwrap().success());

    let retried = isolated_command(&workspace, temp.path())
        .args(["--json", "describe", "--", "new", "description"])
        .output()
        .unwrap();
    assert!(
        retried.status.success(),
        "description retry failed: {}",
        String::from_utf8_lossy(&retried.stderr)
    );
    assert_eq!(
        workspace::load_metadata(&workspace)
            .unwrap()
            .description
            .as_deref(),
        Some("new description")
    );
    assert!(!temp.path().join("no-global").exists());
}

#[test]
fn guidance_snapshot_holds_the_latest_membership_and_resumes() {
    let temp = tempfile::tempdir().unwrap();
    let remotes = temp.path().join("remotes");
    create_remote(&remotes);
    let daemon = git_daemon(&remotes);
    let url = format!("{}/acme/api.git", daemon.1);
    let workspace = empty_workspace(temp.path());
    let (mut child, mut stream, reached) =
        crash_add_at_guidance(&workspace, temp.path(), &url, Point::GuidanceSnapshot);
    assert!(reached.lock_held);
    assert!(
        workspace::load_metadata(&workspace)
            .unwrap()
            .repos
            .contains_key("127.0.0.1/acme/api"),
        "guidance must snapshot the committed membership"
    );
    assert!(
        !workspace.join("AGENTS.md").exists(),
        "AGENTS.md must not precede the snapshot barrier"
    );
    resume(&mut stream, &reached);
    drop(stream);
    assert!(child.wait().unwrap().success());
    assert!(workspace.join("AGENTS.md").is_file());
}

#[test]
fn killed_after_agents_commit_preserves_user_guidance_and_retry_converges() {
    let temp = tempfile::tempdir().unwrap();
    let remotes = temp.path().join("remotes");
    create_remote(&remotes);
    let daemon = git_daemon(&remotes);
    let url = format!("{}/acme/api.git", daemon.1);
    let workspace = empty_workspace(temp.path());
    fs::write(
        workspace.join("AGENTS.md"),
        "# User guidance\n\nKeep this exact text.\n",
    )
    .unwrap();
    let (mut child, stream, reached) = crash_add_at_guidance(
        &workspace,
        temp.path(),
        &url,
        Point::GuidanceAgentsCommitted,
    );
    assert!(reached.lock_held);
    let interrupted_agents = fs::read_to_string(workspace.join("AGENTS.md")).unwrap();
    assert!(interrupted_agents.contains("Keep this exact text."));
    assert!(
        interrupted_agents.contains("github.com/127.0.0.1/acme/api")
            || interrupted_agents.contains("127.0.0.1/acme/api")
    );
    assert!(
        workspace::load_metadata(&workspace)
            .unwrap()
            .repos
            .contains_key("127.0.0.1/acme/api")
    );
    child.kill().unwrap();
    drop(stream);
    assert!(!child.wait().unwrap().success());

    let retried = isolated_command(&workspace, temp.path())
        .args(["--json", "repo", "add", &url])
        .output()
        .unwrap();
    assert!(
        retried.status.success(),
        "guidance retry failed: {}",
        String::from_utf8_lossy(&retried.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&retried.stdout).unwrap();
    assert_eq!(result["repos"][0]["clone"], "already_present");
    assert_eq!(result["repos"][0]["guidance"], "updated");
    let repaired_agents = fs::read_to_string(workspace.join("AGENTS.md")).unwrap();
    assert!(repaired_agents.contains("Keep this exact text."));
    assert!(repaired_agents.contains("<!-- wsp:begin -->"));
}

#[test]
fn local_add_and_host_describe_serialize_without_losing_membership_or_guidance() {
    let temp = tempfile::tempdir().unwrap();
    let remotes = temp.path().join("remotes");
    create_remote(&remotes);
    let daemon = git_daemon(&remotes);
    let url = format!("{}/acme/api.git", daemon.1);
    let workspace = empty_workspace(temp.path());

    // Hold the local add after its membership is durable and while it owns the
    // metadata lock for guidance. A host describe launched here must preserve
    // that member when the lock becomes available.
    let (mut local, mut local_stream, local_reached) =
        crash_add_at_guidance(&workspace, temp.path(), &url, Point::GuidanceSnapshot);
    assert!(local_reached.lock_held);
    assert!(
        workspace::load_metadata(&workspace)
            .unwrap()
            .repos
            .contains_key("127.0.0.1/acme/api")
    );

    let host_controller = Controller::new();
    let mut host_command = host_command(&workspace, temp.path());
    host_command
        .args(["--json", "describe", "--", "host", "description"])
        .env("WSP_TEST_CRASH_ADDR", host_controller.address())
        .env("WSP_TEST_CRASH_TOKEN", &host_controller.token)
        .env("WSP_TEST_CRASH_SESSION", &host_controller.session)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let host = CrashChild::spawn(&mut host_command);

    // The handshake proves the host command has started. It cannot reach its
    // commit barrier until the paused local operation releases the same lock.
    let mut host_stream = host_controller
        .accept_hello(host.id())
        .expect("validate host describe crash-barrier hello");
    let host_selection = Selection {
        operation: Operation::Describe,
        repository: "workspace/mounted".into(),
        point: Point::DescriptionCommitted,
        occurrence: 1,
    };
    crash_barrier::write_message(
        &mut host_stream,
        &Message::Select {
            version: 1,
            session: host_controller.session.clone(),
            token: host_controller.token.clone(),
            selections: vec![host_selection],
        },
    )
    .unwrap();

    resume(&mut local_stream, &local_reached);
    drop(local_stream);
    assert!(local.wait().unwrap().success(), "resumed local add failed");

    let Message::Reached(host_reached) = crash_barrier::read_message(&mut host_stream)
        .expect("read host description commit barrier")
    else {
        panic!("expected host description commit barrier");
    };
    assert_eq!(host_reached.operation, Operation::Describe);
    assert_eq!(host_reached.point, Point::DescriptionCommitted);
    assert!(host_reached.lock_held);
    assert!(
        workspace::load_metadata(&workspace)
            .unwrap()
            .repos
            .contains_key("127.0.0.1/acme/api")
    );
    assert_eq!(
        workspace::load_metadata(&workspace)
            .unwrap()
            .description
            .as_deref(),
        Some("host description")
    );
    let guidance = fs::read_to_string(workspace.join("AGENTS.md")).unwrap();
    assert!(guidance.contains("127.0.0.1/acme/api"));

    resume(&mut host_stream, &host_reached);
    drop(host_stream);
    let output = host.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "resumed host describe failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn guidance_complete_is_after_agents_and_before_a_resumed_command_returns() {
    let temp = tempfile::tempdir().unwrap();
    let remotes = temp.path().join("remotes");
    create_remote(&remotes);
    let daemon = git_daemon(&remotes);
    let url = format!("{}/acme/api.git", daemon.1);
    let workspace = empty_workspace(temp.path());
    let (mut child, mut stream, reached) =
        crash_add_at_guidance(&workspace, temp.path(), &url, Point::GuidanceComplete);
    assert!(reached.lock_held);
    assert!(workspace.join("AGENTS.md").is_file());
    resume(&mut stream, &reached);
    drop(stream);
    assert!(child.wait().unwrap().success());
}

fn crash_host_add_at(
    workspace: &std::path::Path,
    root: &std::path::Path,
    url: &str,
    point: Point,
) -> (
    std::process::Child,
    TcpStream,
    wsp_core::crash_barrier::Reached,
) {
    let controller = Controller::new();
    let child = host_command(workspace, root)
        .args(["--json", "repo", "add", url])
        .env("WSP_TEST_CRASH_ADDR", controller.address())
        .env("WSP_TEST_CRASH_TOKEN", &controller.token)
        .env("WSP_TEST_CRASH_SESSION", &controller.session)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let selected = Selection {
        operation: Operation::Add,
        repository: "127.0.0.1/acme/api".into(),
        point,
        occurrence: 1,
    };
    let (stream, reached) = controller.wait_for(child.id(), selected);
    (child, stream, reached)
}

fn push_remote_commit(remote: &std::path::Path) {
    let source = tempfile::tempdir().unwrap();
    git(source.path(), &["clone", remote.to_str().unwrap(), "."]);
    wsp_core::testutil::local_commit(source.path(), "new.txt", "new remote commit");
    git(source.path(), &["push", "origin", "main"]);
}

fn fail(stream: &mut TcpStream, reached: &wsp_core::crash_barrier::Reached) {
    crash_barrier::write_message(
        stream,
        &Message::Fail {
            session: reached.session.clone(),
            sequence: reached.sequence,
        },
    )
    .unwrap();
}

#[test]
fn host_add_admission_has_no_effects_and_a_resumed_child_registers_and_adds() {
    let temp = tempfile::tempdir().unwrap();
    let remotes = temp.path().join("remotes");
    create_remote(&remotes);
    let daemon = git_daemon(&remotes);
    let url = format!("{}/acme/api.git", daemon.1);
    let workspace = empty_workspace(temp.path());
    let data = host_config(temp.path());
    let (mut child, mut stream, reached) =
        crash_host_add_at(&workspace, temp.path(), &url, Point::AddAdmitted);
    assert!(!reached.lock_held);
    assert!(
        workspace::load_metadata(&workspace)
            .unwrap()
            .repos
            .is_empty()
    );
    assert!(
        Config::load_from(&data.join("config.yaml"))
            .unwrap()
            .repos
            .is_empty()
    );
    assert!(!data.join("mirrors").exists());
    resume(&mut stream, &reached);
    drop(stream);
    assert!(child.wait().unwrap().success());
    assert!(
        workspace::load_metadata(&workspace)
            .unwrap()
            .repos
            .contains_key("127.0.0.1/acme/api")
    );
    assert!(
        Config::load_from(&data.join("config.yaml"))
            .unwrap()
            .repos
            .contains_key("127.0.0.1/acme/api")
    );
}

#[test]
fn killed_after_mirror_preparation_leaves_an_unregistered_orphan_mirror() {
    let temp = tempfile::tempdir().unwrap();
    let remotes = temp.path().join("remotes");
    create_remote(&remotes);
    let daemon = git_daemon(&remotes);
    let url = format!("{}/acme/api.git", daemon.1);
    let workspace = empty_workspace(temp.path());
    let data = host_config(temp.path());
    let (mut child, stream, reached) =
        crash_host_add_at(&workspace, temp.path(), &url, Point::MirrorPrepared);
    assert!(!reached.lock_held);
    let parsed = wsp_core::giturl::parse(&url).unwrap();
    let mirror = wsp_core::mirror::dir(&data.join("mirrors"), &parsed);
    assert!(mirror.is_dir(), "mirror preparation must precede the event");
    assert!(
        Config::load_from(&data.join("config.yaml"))
            .unwrap()
            .repos
            .is_empty()
    );
    assert!(
        workspace::load_metadata(&workspace)
            .unwrap()
            .repos
            .is_empty()
    );
    child.kill().unwrap();
    drop(stream);
    assert!(!child.wait().unwrap().success());
}

#[test]
fn killed_after_registry_commit_keeps_global_registration_and_host_retry_adds_member() {
    let temp = tempfile::tempdir().unwrap();
    let remotes = temp.path().join("remotes");
    create_remote(&remotes);
    let daemon = git_daemon(&remotes);
    let url = format!("{}/acme/api.git", daemon.1);
    let workspace = empty_workspace(temp.path());
    let data = host_config(temp.path());
    let (mut child, stream, reached) =
        crash_host_add_at(&workspace, temp.path(), &url, Point::RegistryCommitted);
    assert!(reached.lock_held);
    assert!(
        Config::load_from(&data.join("config.yaml"))
            .unwrap()
            .repos
            .contains_key("127.0.0.1/acme/api")
    );
    assert!(
        workspace::load_metadata(&workspace)
            .unwrap()
            .repos
            .is_empty()
    );
    child.kill().unwrap();
    drop(stream);
    assert!(!child.wait().unwrap().success());

    let retried = host_command(&workspace, temp.path())
        .args(["--json", "repo", "add", &url])
        .output()
        .unwrap();
    assert!(
        retried.status.success(),
        "host retry failed: {}",
        String::from_utf8_lossy(&retried.stderr)
    );
    assert!(
        workspace::load_metadata(&workspace)
            .unwrap()
            .repos
            .contains_key("127.0.0.1/acme/api")
    );
}

#[test]
fn isolated_pre_membership_failure_is_adopted_by_a_host_retry() {
    let temp = tempfile::tempdir().unwrap();
    let remotes = temp.path().join("remotes");
    create_remote(&remotes);
    let daemon = git_daemon(&remotes);
    let url = format!("{}/acme/api.git", daemon.1);
    let workspace = empty_workspace(temp.path());
    let controller = Controller::new();
    let child = isolated_command(&workspace, temp.path())
        .args(["--json", "repo", "add", &url])
        .env("WSP_TEST_CRASH_ADDR", controller.address())
        .env("WSP_TEST_CRASH_TOKEN", &controller.token)
        .env("WSP_TEST_CRASH_SESSION", &controller.session)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let selected = Selection {
        operation: Operation::Add,
        repository: "127.0.0.1/acme/api".into(),
        point: Point::MetadataReplacePending,
        occurrence: 1,
    };
    let (mut stream, reached) = controller.wait_for(child.id(), selected);
    assert!(reached.lock_held);
    let clone = workspace.join("api");
    assert!(
        clone.join(".git").is_dir(),
        "publication must precede the failure seam"
    );
    assert!(
        workspace::load_metadata(&workspace)
            .unwrap()
            .repos
            .is_empty()
    );
    fail(&mut stream, &reached);
    drop(stream);
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["repos"][0]["membership"], "failed");
    assert!(
        workspace::load_metadata(&workspace)
            .unwrap()
            .repos
            .is_empty()
    );

    fs::write(
        clone.join("README.md"),
        "developer edit after failed metadata save\n",
    )
    .unwrap();
    // The retry intentionally has normal host state.  A failed isolated add
    // publishes an ordinary clone but no membership, and host re-entry must
    // adopt that clone rather than attempt a second clone or discard edits.
    let data = host_config(temp.path());
    let retried = host_command(&workspace, temp.path())
        .args(["--json", "repo", "add", &url])
        .output()
        .unwrap();
    assert!(
        retried.status.success(),
        "adoption retry failed: {}",
        String::from_utf8_lossy(&retried.stderr)
    );
    let retried_result: serde_json::Value = serde_json::from_slice(&retried.stdout).unwrap();
    assert_eq!(retried_result["repos"][0]["clone"], "adopted");
    assert_eq!(retried_result["repos"][0]["membership"], "updated");
    assert!(
        Config::load_from(&data.join("config.yaml"))
            .unwrap()
            .repos
            .is_empty(),
        "adopting an isolated member must not implicitly register it"
    );
    assert_eq!(
        fs::read_to_string(clone.join("README.md")).unwrap(),
        "developer edit after failed metadata save\n"
    );
}

#[test]
fn failed_selected_direct_refresh_leaves_remote_tracking_refs_unchanged() {
    let temp = tempfile::tempdir().unwrap();
    let remotes = temp.path().join("remotes");
    create_remote(&remotes);
    let daemon = git_daemon(&remotes);
    let url = format!("{}/acme/api.git", daemon.1);
    let workspace = empty_workspace(temp.path());
    add_workspace_member(&workspace, temp.path(), &url);
    let clone = workspace.join("api");
    let before =
        wsp_core::git::run(Some(&clone), &["rev-parse", "refs/remotes/origin/main"]).unwrap();
    push_remote_commit(&remotes.join("acme/api.git"));

    let controller = Controller::new();
    let child = isolated_command(&workspace, temp.path())
        .args(["--json", "repo", "fetch"])
        .env("WSP_TEST_CRASH_ADDR", controller.address())
        .env("WSP_TEST_CRASH_TOKEN", &controller.token)
        .env("WSP_TEST_CRASH_SESSION", &controller.session)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let selected = Selection {
        operation: Operation::Refresh,
        repository: "127.0.0.1/acme/api".into(),
        point: Point::RefreshSelected,
        occurrence: 1,
    };
    let (mut stream, reached) = controller.wait_for(child.id(), selected);
    assert!(!reached.lock_held);
    fail(&mut stream, &reached);
    drop(stream);
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["repos"][0]["ok"], false);
    assert_eq!(
        wsp_core::git::run(Some(&clone), &["rev-parse", "refs/remotes/origin/main"]).unwrap(),
        before,
        "the selected transport must not fetch after a synthetic pre-fetch failure"
    );
}

#[test]
fn refresh_complete_fires_only_after_direct_refs_change_and_resumes() {
    let temp = tempfile::tempdir().unwrap();
    let remotes = temp.path().join("remotes");
    create_remote(&remotes);
    let daemon = git_daemon(&remotes);
    let url = format!("{}/acme/api.git", daemon.1);
    let workspace = empty_workspace(temp.path());
    add_workspace_member(&workspace, temp.path(), &url);
    let clone = workspace.join("api");
    let before =
        wsp_core::git::run(Some(&clone), &["rev-parse", "refs/remotes/origin/main"]).unwrap();
    push_remote_commit(&remotes.join("acme/api.git"));

    let controller = Controller::new();
    let child = isolated_command(&workspace, temp.path())
        .args(["--json", "repo", "fetch"])
        .env("WSP_TEST_CRASH_ADDR", controller.address())
        .env("WSP_TEST_CRASH_TOKEN", &controller.token)
        .env("WSP_TEST_CRASH_SESSION", &controller.session)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let selected = Selection {
        operation: Operation::Refresh,
        repository: "127.0.0.1/acme/api".into(),
        point: Point::RefreshComplete,
        occurrence: 1,
    };
    let (mut stream, reached) = controller.wait_for(child.id(), selected);
    assert!(!reached.lock_held);
    assert_ne!(
        wsp_core::git::run(Some(&clone), &["rev-parse", "refs/remotes/origin/main"]).unwrap(),
        before,
        "ref propagation must precede refresh_complete"
    );
    resume(&mut stream, &reached);
    drop(stream);
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "resumed fetch failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn add_workspace_member(workspace: &std::path::Path, root: &std::path::Path, url: &str) {
    let added = isolated_command(workspace, root)
        .args(["--json", "repo", "add", url])
        .output()
        .unwrap();
    assert!(
        added.status.success(),
        "adding crash-test member failed: {}",
        String::from_utf8_lossy(&added.stderr)
    );
}

fn crash_remove_at(
    workspace: &std::path::Path,
    root: &std::path::Path,
    repository: &str,
    point: Point,
) -> (CrashChild, TcpStream, wsp_core::crash_barrier::Reached) {
    let controller = Controller::new();
    let mut command = isolated_command(workspace, root);
    command
        .args(["--json", "repo", "rm", "--force", repository])
        .env("WSP_TEST_CRASH_ADDR", controller.address())
        .env("WSP_TEST_CRASH_TOKEN", &controller.token)
        .env("WSP_TEST_CRASH_SESSION", &controller.session)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let child = CrashChild::spawn(&mut command);
    let selected = Selection {
        operation: Operation::Remove,
        repository: "127.0.0.1/acme/api".into(),
        point,
        occurrence: 1,
    };
    let (stream, reached) = controller.wait_for(child.id(), selected);
    (child, stream, reached)
}

fn resume(stream: &mut TcpStream, reached: &wsp_core::crash_barrier::Reached) {
    crash_barrier::write_message(
        stream,
        &Message::Continue {
            session: reached.session.clone(),
            sequence: reached.sequence,
        },
    )
    .unwrap();
}

#[test]
fn removal_recheck_is_before_deletion_and_a_resumed_child_completes() {
    let temp = tempfile::tempdir().unwrap();
    let remotes = temp.path().join("remotes");
    create_remote(&remotes);
    let daemon = git_daemon(&remotes);
    let url = format!("{}/acme/api.git", daemon.1);
    let workspace = empty_workspace(temp.path());
    add_workspace_member(&workspace, temp.path(), &url);

    let (mut child, mut stream, reached) =
        crash_remove_at(&workspace, temp.path(), "api", Point::RemoveRechecked);
    assert_eq!(reached.operation, Operation::Remove);
    assert_eq!(reached.point, Point::RemoveRechecked);
    assert!(reached.lock_held);
    assert!(workspace.join("api/.git").is_dir());
    assert!(
        workspace::load_metadata(&workspace)
            .unwrap()
            .repos
            .contains_key("127.0.0.1/acme/api")
    );

    resume(&mut stream, &reached);
    drop(stream);
    assert!(child.wait().unwrap().success());
    assert!(!workspace.join("api").exists());
    assert!(
        !workspace::load_metadata(&workspace)
            .unwrap()
            .repos
            .contains_key("127.0.0.1/acme/api")
    );
}

#[test]
fn killed_after_clone_deletion_requires_force_to_clear_missing_membership() {
    let temp = tempfile::tempdir().unwrap();
    let remotes = temp.path().join("remotes");
    create_remote(&remotes);
    let daemon = git_daemon(&remotes);
    let url = format!("{}/acme/api.git", daemon.1);
    let workspace = empty_workspace(temp.path());
    add_workspace_member(&workspace, temp.path(), &url);

    let (mut child, stream, reached) =
        crash_remove_at(&workspace, temp.path(), "api", Point::CloneDeleted);
    assert_eq!(reached.point, Point::CloneDeleted);
    assert!(reached.lock_held);
    assert!(!workspace.join("api").exists());
    assert!(
        workspace::load_metadata(&workspace)
            .unwrap()
            .repos
            .contains_key("127.0.0.1/acme/api"),
        "the clone deletion must precede metadata persistence"
    );
    child.kill().unwrap();
    drop(stream);
    assert!(!child.wait().unwrap().success());

    let ordinary_retry = isolated_command(&workspace, temp.path())
        .args(["--json", "repo", "rm", "api"])
        .output()
        .unwrap();
    assert!(!ordinary_retry.status.success());
    assert!(
        workspace::load_metadata(&workspace)
            .unwrap()
            .repos
            .contains_key("127.0.0.1/acme/api"),
        "ordinary retry must retain a missing member"
    );

    let (mut forced, mut stream, missing) =
        crash_remove_at(&workspace, temp.path(), "api", Point::MissingCloneConfirmed);
    assert_eq!(missing.point, Point::MissingCloneConfirmed);
    assert!(missing.lock_held);
    assert!(!workspace.join("api").exists());
    assert!(
        workspace::load_metadata(&workspace)
            .unwrap()
            .repos
            .contains_key("127.0.0.1/acme/api")
    );
    resume(&mut stream, &missing);
    drop(stream);
    assert!(forced.wait().unwrap().success());
    assert!(
        !workspace::load_metadata(&workspace)
            .unwrap()
            .repos
            .contains_key("127.0.0.1/acme/api")
    );
    assert!(!temp.path().join("no-global").exists());
}

#[test]
fn killed_after_removal_commit_leaves_the_member_removed() {
    let temp = tempfile::tempdir().unwrap();
    let remotes = temp.path().join("remotes");
    create_remote(&remotes);
    let daemon = git_daemon(&remotes);
    let url = format!("{}/acme/api.git", daemon.1);
    let workspace = empty_workspace(temp.path());
    add_workspace_member(&workspace, temp.path(), &url);

    let (mut child, stream, reached) =
        crash_remove_at(&workspace, temp.path(), "api", Point::RemovalCommitted);
    assert_eq!(reached.point, Point::RemovalCommitted);
    assert!(reached.lock_held);
    assert!(!workspace.join("api").exists());
    assert!(
        !workspace::load_metadata(&workspace)
            .unwrap()
            .repos
            .contains_key("127.0.0.1/acme/api"),
        "metadata persistence must precede removal_committed"
    );
    child.kill().unwrap();
    drop(stream);
    assert!(!child.wait().unwrap().success());

    let repeated = isolated_command(&workspace, temp.path())
        .args(["--json", "repo", "rm", "api"])
        .output()
        .unwrap();
    assert!(!repeated.status.success());
    assert!(
        !repeated.stdout.is_empty(),
        "expected a structured error, stderr: {}",
        String::from_utf8_lossy(&repeated.stderr)
    );
    assert!(!temp.path().join("no-global").exists());
}

#[test]
fn removal_rechecks_the_clone_after_a_barrier_before_recursive_deletion() {
    let temp = tempfile::tempdir().unwrap();
    let remotes = temp.path().join("remotes");
    create_remote(&remotes);
    let daemon = git_daemon(&remotes);
    let url = format!("{}/acme/api.git", daemon.1);
    let workspace = empty_workspace(temp.path());
    add_workspace_member(&workspace, temp.path(), &url);

    let (mut child, mut stream, reached) =
        crash_remove_at(&workspace, temp.path(), "api", Point::RemoveRechecked);
    let clone = workspace.join("api");
    let original = workspace.join("original-api");
    fs::rename(&clone, &original).unwrap();
    fs::create_dir(&clone).unwrap();
    git(&clone, &["init", "--initial-branch=main"]);
    fs::write(clone.join("do-not-delete"), "foreign\n").unwrap();

    resume(&mut stream, &reached);
    drop(stream);
    assert!(!child.wait().unwrap().success());
    assert!(original.join(".git").is_dir());
    assert_eq!(
        fs::read_to_string(clone.join("do-not-delete")).unwrap(),
        "foreign\n"
    );
    assert!(
        workspace::load_metadata(&workspace)
            .unwrap()
            .repos
            .contains_key("127.0.0.1/acme/api"),
        "failed post-barrier validation must preserve membership"
    );
}

#[cfg(unix)]
#[test]
fn removal_never_recursively_deletes_a_replacement_after_quarantine() {
    let temp = tempfile::tempdir().unwrap();
    let remotes = temp.path().join("remotes");
    create_remote(&remotes);
    let daemon = git_daemon(&remotes);
    let url = format!("{}/acme/api.git", daemon.1);
    let workspace = empty_workspace(temp.path());
    add_workspace_member(&workspace, temp.path(), &url);

    let (mut child, mut stream, reached) =
        crash_remove_at(&workspace, temp.path(), "api", Point::CloneQuarantined);
    let quarantine = fs::read_dir(&workspace)
        .unwrap()
        .map(Result::unwrap)
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with(".wsp-remove-"))
        })
        .expect("removal must atomically move the validated clone aside");
    let preserved = workspace.join("preserved-api");
    fs::rename(&quarantine, &preserved).unwrap();
    fs::create_dir(&quarantine).unwrap();
    fs::write(quarantine.join("do-not-delete"), "foreign\n").unwrap();

    resume(&mut stream, &reached);
    drop(stream);
    assert!(!child.wait().unwrap().success());
    assert!(preserved.join(".git").is_dir());
    assert_eq!(
        fs::read_to_string(quarantine.join("do-not-delete")).unwrap(),
        "foreign\n"
    );
    assert!(
        workspace::load_metadata(&workspace)
            .unwrap()
            .repos
            .contains_key("127.0.0.1/acme/api"),
        "a substituted quarantine path must retain membership for a safe retry"
    );
    let retry = isolated_command(&workspace, temp.path())
        .args(["--json", "repo", "rm", "--force", "api"])
        .output()
        .unwrap();
    assert!(!retry.status.success());
    assert!(
        workspace::load_metadata(&workspace)
            .unwrap()
            .repos
            .contains_key("127.0.0.1/acme/api"),
        "force must not discard membership while a malformed quarantine remains"
    );
}

#[cfg(unix)]
#[test]
fn retry_recovers_a_clone_left_in_quarantine_by_a_crash() {
    let temp = tempfile::tempdir().unwrap();
    let remotes = temp.path().join("remotes");
    create_remote(&remotes);
    let daemon = git_daemon(&remotes);
    let url = format!("{}/acme/api.git", daemon.1);
    let workspace = empty_workspace(temp.path());
    add_workspace_member(&workspace, temp.path(), &url);

    let (mut child, stream, _reached) =
        crash_remove_at(&workspace, temp.path(), "api", Point::CloneQuarantined);
    assert!(!workspace.join("api").is_dir());
    child.kill().unwrap();
    drop(stream);
    assert!(!child.wait().unwrap().success());

    let retry = isolated_command(&workspace, temp.path())
        .args(["--json", "repo", "rm", "api"])
        .output()
        .unwrap();
    assert!(
        retry.status.success(),
        "retry failed: {}",
        String::from_utf8_lossy(&retry.stderr)
    );
    assert!(
        !workspace::load_metadata(&workspace)
            .unwrap()
            .repos
            .contains_key("127.0.0.1/acme/api")
    );
    assert!(fs::read_dir(&workspace).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".wsp-remove-")
    }));
}
