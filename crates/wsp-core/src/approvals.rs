//! Persistent store for setup-command approval decisions.
//!
//! Approvals are keyed by `(repo_identity, sha256_hex_of_commands)`.
//! An [`ApprovalDecision::Always`] record means wsp runs setup commands on
//! future clones of the same repo without prompting — until the commands
//! change (different hash → re-prompt). This is the same model direnv uses
//! for `.envrc` files.
//!
//! The store lives at `<data_dir>/approvals.yaml` (next to `config.yaml`).
//! Reads are lock-free (load + check). Writes acquire an exclusive `FileLock`
//! on `approvals.yaml.lock` for the full load-check-modify-save cycle, then
//! atomically rename a `NamedTempFile` into place to avoid torn writes.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};

use crate::filelock::FileLock;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalDecision {
    /// Run these commands for this repo on every future clone, without prompting.
    Always,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalEntry {
    pub identity: String,
    /// SHA-256 hex of the command list (one command per line, trailing newline).
    pub commands_hash: String,
    pub decision: ApprovalDecision,
    pub approved_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ApprovalStore {
    #[serde(default)]
    pub entries: Vec<ApprovalEntry>,
}

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

pub fn approvals_path(data_dir: &Path) -> PathBuf {
    data_dir.join("approvals.yaml")
}

// ---------------------------------------------------------------------------
// Hashing
// ---------------------------------------------------------------------------

/// Compute a stable SHA-256 hex of a command list.
///
/// Each command is hashed as `<cmd>\n` so that `["a", "b"]` differs from
/// `["ab"]`. The result is stable across runs and platforms.
pub fn commands_hash(commands: &[String]) -> String {
    let mut hasher = Sha256::new();
    for cmd in commands {
        hasher.update(cmd.as_bytes());
        hasher.update(b"\n");
    }
    format!("{:x}", hasher.finalize())
}

// ---------------------------------------------------------------------------
// I/O
// ---------------------------------------------------------------------------

/// Load the approval store. Returns an empty store if the file doesn't exist.
pub fn load(data_dir: &Path) -> Result<ApprovalStore> {
    let path = approvals_path(data_dir);
    let content = match crate::util::read_yaml_file(&path) {
        Ok(s) => s,
        Err(e) if is_not_found(&e) => return Ok(ApprovalStore::default()),
        Err(e) => return Err(e),
    };
    let store: ApprovalStore = serde_yaml_ng::from_str(&content)
        .map_err(|e| anyhow::anyhow!("parsing approvals store: {}", e))?;
    Ok(store)
}

fn is_not_found(e: &anyhow::Error) -> bool {
    e.chain().any(|cause| {
        if let Some(io) = cause.downcast_ref::<std::io::Error>() {
            io.kind() == std::io::ErrorKind::NotFound
        } else {
            false
        }
    })
}

fn save(data_dir: &Path, store: &ApprovalStore) -> Result<()> {
    let path = approvals_path(data_dir);
    let parent = path.parent().context("approvals path has no parent")?;
    std::fs::create_dir_all(parent)?;
    let content = serde_yaml_ng::to_string(store)
        .map_err(|e| anyhow::anyhow!("serializing approvals store: {}", e))?;
    // Atomic write: temp file in same dir + persist (rename). Using NamedTempFile
    // ensures the temp file is cleaned up even if persist fails or the process crashes.
    let mut tmp =
        tempfile::NamedTempFile::new_in(parent).context("creating temp file for approvals")?;
    std::io::Write::write_all(&mut tmp, content.as_bytes())
        .context("writing approvals to temp file")?;
    tmp.persist(&path)
        .context("renaming temp file to approvals.yaml")?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Query / mutation
// ---------------------------------------------------------------------------

/// Returns `true` if `(identity, hash)` has an `Always` approval in the store.
pub fn is_approved(store: &ApprovalStore, identity: &str, hash: &str) -> bool {
    store.entries.iter().any(|e| {
        e.identity == identity && e.commands_hash == hash && e.decision == ApprovalDecision::Always
    })
}

/// Append an `Always` entry for `(identity, hash)`. Idempotent: if an
/// identical entry already exists the store is not modified.
pub fn record_always(data_dir: &Path, identity: &str, hash: &str) -> Result<()> {
    let path = approvals_path(data_dir);
    let _lock = FileLock::acquire(&path, Duration::from_secs(30))?;
    let mut store = load(data_dir)?;
    if is_approved(&store, identity, hash) {
        return Ok(());
    }
    store.entries.push(ApprovalEntry {
        identity: identity.to_string(),
        commands_hash: hash.to_string(),
        decision: ApprovalDecision::Always,
        approved_at: Utc::now(),
    });
    save(data_dir, &store)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_hash_is_deterministic() {
        let cmds = vec!["task setup".to_string(), "lefthook install".to_string()];
        assert_eq!(commands_hash(&cmds), commands_hash(&cmds));
    }

    #[test]
    fn commands_hash_differs_by_content() {
        let a = commands_hash(&["task setup".to_string()]);
        let b = commands_hash(&["npm install".to_string()]);
        assert_ne!(a, b);
    }

    #[test]
    fn commands_hash_differs_by_order() {
        let a = commands_hash(&["a".to_string(), "b".to_string()]);
        let b = commands_hash(&["b".to_string(), "a".to_string()]);
        assert_ne!(a, b);
    }

    #[test]
    fn commands_hash_empty_differs_from_nonempty() {
        let a = commands_hash(&[]);
        let b = commands_hash(&["x".to_string()]);
        assert_ne!(a, b);
    }

    #[test]
    fn is_approved_empty_store() {
        let store = ApprovalStore::default();
        assert!(!is_approved(&store, "github.com/org/repo", "abc123"));
    }

    #[test]
    fn is_approved_matching_entry() {
        let store = ApprovalStore {
            entries: vec![ApprovalEntry {
                identity: "github.com/org/repo".to_string(),
                commands_hash: "abc123".to_string(),
                decision: ApprovalDecision::Always,
                approved_at: Utc::now(),
            }],
        };
        assert!(is_approved(&store, "github.com/org/repo", "abc123"));
        assert!(!is_approved(
            &store,
            "github.com/org/repo",
            "different_hash"
        ));
        assert!(!is_approved(&store, "github.com/org/other", "abc123"));
    }

    #[test]
    fn record_always_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path();
        record_always(data_dir, "github.com/org/repo", "abc123").unwrap();
        let store = load(data_dir).unwrap();
        assert!(is_approved(&store, "github.com/org/repo", "abc123"));
    }

    #[test]
    fn record_always_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path();
        record_always(data_dir, "github.com/org/repo", "abc123").unwrap();
        record_always(data_dir, "github.com/org/repo", "abc123").unwrap();
        let store = load(data_dir).unwrap();
        assert_eq!(store.entries.len(), 1);
    }

    #[test]
    fn multiple_repos_independent() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path();
        record_always(data_dir, "github.com/org/a", "hash1").unwrap();
        record_always(data_dir, "github.com/org/b", "hash2").unwrap();
        let store = load(data_dir).unwrap();
        assert!(is_approved(&store, "github.com/org/a", "hash1"));
        assert!(is_approved(&store, "github.com/org/b", "hash2"));
        assert!(!is_approved(&store, "github.com/org/a", "hash2"));
    }

    #[test]
    fn load_missing_file_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let store = load(dir.path()).unwrap();
        assert!(store.entries.is_empty());
    }
}
