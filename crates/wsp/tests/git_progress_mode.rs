//! Real-binary coverage of the Git progress configuration and argument contract.
use std::path::PathBuf;
use std::process::{Command, Output};

struct Fixture {
    root: tempfile::TempDir,
    config: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("gitconfig"), "").unwrap();
        let config = root.path().join("data/wsp/config.yaml");
        Self { root, config }
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_wsp"))
            .args(args)
            .current_dir(self.root.path())
            .env("XDG_DATA_HOME", self.root.path().join("data"))
            .env("GIT_CONFIG_GLOBAL", self.root.path().join("gitconfig"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("WSP_SHELL")
            .env_remove("WSP_CD_FILE")
            .env_remove("WSP_PWD")
            .output()
            .unwrap()
    }

    fn success(&self, args: &[&str]) -> Output {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "{args:?}: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        output
    }

    fn get(&self, key: &str) -> serde_json::Value {
        let output = self.success(&["config", "get", key, "--global", "--json"]);
        serde_json::from_slice(&output.stdout).unwrap()
    }
}

#[test]
fn git_progress_global_mode_round_trips_and_unsets_to_default() {
    let fixture = Fixture::new();
    assert_eq!(fixture.get("progress.mode")["value"], "parallel");
    for mode in ["native", "parallel"] {
        fixture.success(&["config", "set", "progress.mode", mode, "--global"]);
        assert_eq!(
            fixture.get("progress.mode"),
            serde_json::json!({"key": "progress.mode", "value": mode}),
        );
    }
    fixture.success(&["config", "set", "progress.mode", "native", "--global"]);
    fixture.success(&["config", "unset", "progress.mode", "--global"]);
    assert_eq!(fixture.get("progress.mode")["value"], "parallel");
}

#[test]
fn git_progress_repository_overrides_preserve_literal_identity_and_other_settings() {
    let fixture = Fixture::new();
    let first = "progress.repos.github.com/team/service_api";
    let second = "progress.repos.git.example.com/team/service-api";
    for (key, mode) in [
        ("progress.mode", "native"),
        (first, "parallel"),
        (second, "native"),
    ] {
        fixture.success(&["config", "set", key, mode, "--global"]);
        assert_eq!(
            fixture.get(key),
            serde_json::json!({"key": key, "value": mode})
        );
    }
    let output = fixture.success(&["config", "ls", "--global", "--json"]);
    let listing: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    for (key, value) in [(first, "parallel"), (second, "native")] {
        let entries: Vec<_> = listing["settings"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|entry| entry["key"] == key)
            .collect();
        assert_eq!(entries.len(), 1, "missing or duplicate override: {listing}");
        assert_eq!(entries[0]["value"], value);
    }
    fixture.success(&["config", "unset", "progress.mode", "--global"]);
    assert_eq!(fixture.get(first)["value"], "parallel");
    assert_eq!(fixture.get(second)["value"], "native");
    fixture.success(&["config", "unset", first, "--global"]);
    assert_eq!(fixture.get(first)["value"], serde_json::Value::Null);
    assert_eq!(fixture.get(second)["value"], "native");
    assert_eq!(fixture.get("progress.mode")["value"], "parallel");
}

#[test]
fn git_progress_invalid_mode_does_not_mutate_config() {
    let fixture = Fixture::new();
    let repository = "progress.repos.github.com/team/service_api";
    fixture.success(&["config", "set", "progress.mode", "native", "--global"]);
    fixture.success(&["config", "set", repository, "parallel", "--global"]);
    let before = std::fs::read(&fixture.config).unwrap();
    for key in ["progress.mode", repository] {
        for invalid in ["auto", "Native", "", "true"] {
            let output = fixture.run(&["config", "set", key, invalid, "--global"]);
            assert!(!output.status.success(), "accepted {key}={invalid:?}");
            let error = String::from_utf8_lossy(&output.stderr);
            assert!(
                error.contains("progress mode must be 'parallel' or 'native'"),
                "{error}"
            );
            assert_eq!(
                std::fs::read(&fixture.config).unwrap(),
                before,
                "mutated config for {key}={invalid:?}"
            );
        }
    }
}

#[test]
fn git_progress_flag_accepts_both_modes_without_persisting_them() {
    let fixture = Fixture::new();
    fixture.success(&["config", "set", "progress.mode", "parallel", "--global"]);
    let before = std::fs::read(&fixture.config).unwrap();
    for mode in ["parallel", "native"] {
        for args in [
            vec![
                "--git-progress",
                mode,
                "config",
                "get",
                "progress.mode",
                "--json",
            ],
            vec![
                "config",
                "get",
                "progress.mode",
                "--git-progress",
                mode,
                "--json",
            ],
        ] {
            let output = fixture.success(&args);
            let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(
                value,
                serde_json::json!({"key": "progress.mode", "value": "parallel"})
            );
            assert!(
                !output.stdout.contains(&0x1b),
                "JSON contains terminal escapes"
            );
            assert_eq!(std::fs::read(&fixture.config).unwrap(), before);
        }
    }
}

#[test]
fn git_progress_invalid_flag_is_rejected_before_creating_a_workspace() {
    let fixture = Fixture::new();
    let workspaces = fixture.root.path().join("workspaces");
    fixture.success(&[
        "config",
        "set",
        "workspaces-dir",
        workspaces.to_str().unwrap(),
        "--global",
    ]);
    let before = std::fs::read(&fixture.config).unwrap();
    for invalid in ["auto", "Native", "true"] {
        let output = fixture.run(&[
            "new",
            "must-not-exist",
            "--empty",
            "--git-progress",
            invalid,
        ]);
        assert!(
            !output.status.success(),
            "accepted --git-progress {invalid}"
        );
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(
            error.contains("invalid value")
                && error.contains("--git-progress")
                && error.contains("parallel")
                && error.contains("native"),
            "{error}"
        );
        assert!(
            !workspaces.join("must-not-exist").exists(),
            "invalid flag created workspace"
        );
        assert_eq!(std::fs::read(&fixture.config).unwrap(), before);
    }
}
