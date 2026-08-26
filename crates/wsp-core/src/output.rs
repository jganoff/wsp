use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Pure string helpers (no rendering deps)
// ---------------------------------------------------------------------------

pub fn format_repo_status(
    ahead: u32,
    behind: u32,
    modified: u32,
    has_upstream: bool,
    expected_branch: &Option<String>,
) -> String {
    let mut parts = Vec::new();
    if let Some(expected) = expected_branch {
        parts.push(format!("not on workspace branch ({})", expected));
    }
    if ahead > 0 {
        if has_upstream {
            parts.push(format!("{} ahead", ahead));
        } else {
            parts.push(format!("{} ahead (no upstream)", ahead));
        }
    }
    if behind > 0 {
        parts.push(format!("{} behind", behind));
    }
    if modified > 0 {
        parts.push(format!("{} modified", modified));
    }
    if parts.is_empty() {
        return "clean".to_string();
    }
    parts.join(", ")
}

pub fn format_error(err: &dyn std::fmt::Display) -> String {
    format!("ERROR: {}", err)
}

// ---------------------------------------------------------------------------
// JSON-serializable output types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct RepoListOutput {
    pub repos: Vec<RepoListEntry>,
}

#[derive(Serialize)]
pub struct RepoListEntry {
    pub identity: String,
    pub shortname: String,
    pub url: String,
}

#[derive(Serialize)]
pub struct TemplateListOutput {
    pub templates: Vec<TemplateListEntry>,
}

#[derive(Serialize)]
pub struct TemplateListEntry {
    pub name: String,
    pub repo_count: usize,
}

#[derive(Serialize)]
pub struct TemplateShowOutput {
    pub name: String,
    pub repos: Vec<TemplateShowRepo>,
}

#[derive(Serialize)]
pub struct TemplateShowRepo {
    pub url: String,
    pub identity: String,
}

/// Which set a `wsp ls` listing covers.
///
/// An enum rather than a string because the text renderer selects the whole
/// column layout from it: a typo in a string literal would silently print
/// removed workspaces under `Created`/`Description` headers.
#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum ListState {
    Active,
    Removed,
}

#[derive(Serialize)]
pub struct WorkspaceListOutput {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// Which set this listing covers.
    ///
    /// Lives here rather than on each entry because a listing is homogeneous --
    /// `wsp ls` lists active workspaces, `wsp ls --removed` lists removed ones.
    /// An entry read on its own is still unambiguous: `removed_at` is set if and
    /// only if the workspace was removed.
    pub state: ListState,
    pub workspaces: Vec<WorkspaceListEntry>,
}

#[derive(Serialize)]
pub struct WorkspaceListEntry {
    pub name: String,
    pub branch: String,
    pub repo_count: usize,
    /// Repo names. Free to include: producing `repo_count` already enumerates
    /// them, and it is the detail `wsp recover show` existed to provide.
    #[serde(default)]
    pub repos: Vec<String>,
    /// Where the workspace is now: its normal path when active, its location
    /// under the gc directory when removed.
    pub path: String,
    /// When the workspace was removed. Set if and only if it was -- this is
    /// what distinguishes a removed entry from an active one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub removed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Empty when unknown: removed workspaces do not keep one, and an entry
    /// whose metadata will not parse has none to read. Omitted rather than
    /// serialized as `""`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub created: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_used: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_from: Option<String>,
}

#[derive(Serialize)]
pub struct StatusOutput {
    pub workspace: String,
    pub branch: String,
    pub workspace_dir: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub created: DateTime<Utc>,
    pub repos: Vec<RepoStatusEntry>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub root: Vec<String>,
    #[serde(skip)]
    pub verbose: bool,
    /// Whether PR fetching is enabled in config. Controls PR column visibility.
    #[serde(skip)]
    pub pr_enabled: bool,
}

/// PR state fetched from the hosting forge (GitHub via `gh`).
/// Only populated when `pr = true` is set in config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrInfo {
    pub number: u64,
    pub url: String,
    /// "OPEN", "MERGED", or "CLOSED"
    pub state: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_draft: bool,
}

#[derive(Serialize)]
pub struct RepoStatusEntry {
    pub identity: String,
    pub shortname: String,
    pub path: String,
    pub branch: String,
    pub ahead: u32,
    pub behind: u32,
    pub changed: u32,
    pub has_upstream: bool,
    pub role: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Set when an active repo's HEAD is on a different branch than the workspace branch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_branch: Option<String>,
    /// PR for the workspace branch on this repo's forge. Only set when `pr = true`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pr: Option<PrInfo>,
}

#[derive(Serialize)]
pub struct DiffOutput {
    pub workspace: String,
    pub branch: String,
    pub workspace_dir: PathBuf,
    pub repos: Vec<RepoDiffEntry>,
}

#[derive(Serialize)]
pub struct RepoDiffEntry {
    pub identity: String,
    pub shortname: String,
    pub path: String,
    pub diff: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Serialize)]
pub struct LogOutput {
    pub workspace: String,
    pub branch: String,
    pub workspace_dir: PathBuf,
    #[serde(skip)]
    pub oneline: bool,
    pub repos: Vec<RepoLogEntry>,
}

#[derive(Serialize)]
pub struct RepoLogEntry {
    pub identity: String,
    pub shortname: String,
    pub path: String,
    pub commits: Vec<LogCommit>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Serialize, Clone)]
pub struct LogCommit {
    pub hash: String,
    pub authored_at: String,
    /// Unix timestamp — used by renderers for relative time, skipped in JSON.
    #[serde(skip)]
    pub timestamp: i64,
    pub subject: String,
}

#[derive(Serialize)]
pub struct ConfigListOutput {
    #[serde(rename = "settings")]
    pub entries: Vec<ConfigListEntry>,
}

#[derive(Serialize)]
pub struct ConfigListEntry {
    pub key: String,
    pub value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub experimental: bool,
}

#[derive(Serialize)]
pub struct ConfigGetOutput {
    pub key: String,
    pub value: Option<String>,
}

#[derive(Serialize)]
pub struct WorkspaceRepoListOutput {
    pub workspace: String,
    pub branch: String,
    pub workspace_dir: PathBuf,
    pub repos: Vec<WorkspaceRepoListEntry>,
}

#[derive(Serialize)]
pub struct WorkspaceRepoListEntry {
    pub identity: String,
    pub shortname: String,
    pub dir_name: String,
}

#[derive(Serialize)]
pub struct ExecOutput {
    pub workspace: String,
    pub repos: Vec<ExecRepoResult>,
}

#[derive(Serialize)]
pub struct ExecRepoResult {
    pub identity: String,
    pub shortname: String,
    pub path: String,
    pub directory: String,
    /// The code the process exited with, or `-1` if it never chose one -- a
    /// signal killed it, or it could not be run at all. `signal` and `error`
    /// say which.
    ///
    /// `-1` is safe as a sentinel because no process can exit with it: unix
    /// exit codes are 0-255. That is what makes it preferable to the shell's
    /// `128 + signal`, which cannot be told apart from a process that genuinely
    /// called `exit(141)`.
    pub exit_code: i32,
    /// The signal that killed the process, absent if it exited normally.
    ///
    /// The reason `exit_code` can be `-1` without being ambiguous. Unix only:
    /// Windows has no signals, so this is never set there.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signal: Option<i32>,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stdout: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stderr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Serialize)]
pub struct FetchOutput {
    pub workspace: String,
    pub repos: Vec<FetchRepoResult>,
}

#[derive(Serialize)]
pub struct FetchRepoResult {
    pub identity: String,
    pub shortname: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Serialize)]
pub struct MutationOutput {
    pub ok: bool,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
}

impl MutationOutput {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            ok: true,
            message: message.into(),
            duration_ms: None,
            hint: None,
            workspace: None,
            path: None,
            branch: None,
        }
    }

    pub fn with_duration(mut self, duration_ms: u64) -> Self {
        self.duration_ms = Some(duration_ms);
        self
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn with_workspace(
        mut self,
        name: impl Into<String>,
        path: impl Into<String>,
        branch: impl Into<String>,
    ) -> Self {
        self.workspace = Some(name.into());
        self.path = Some(path.into());
        self.branch = Some(branch.into());
        self
    }
}

#[derive(Serialize)]
pub struct PathOutput {
    pub path: String,
}

#[derive(Serialize)]
pub struct ErrorOutput {
    pub error: String,
}

#[derive(Serialize)]
pub struct ImportOutput {
    pub registered: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub failed: Vec<ImportFailure>,
}

#[derive(Serialize)]
pub struct ImportFailure {
    pub name: String,
    pub error: String,
}

#[derive(Serialize)]
pub struct SyncOutput {
    pub workspace: String,
    pub branch: String,
    pub dry_run: bool,
    pub repos: Vec<SyncRepoResult>,
}

#[derive(Serialize)]
pub struct SyncRepoResult {
    pub identity: String,
    pub shortname: String,
    pub path: String,
    pub action: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Absolute path to repo dir — used by renderer for conflict footer.
    #[serde(skip)]
    pub repo_dir: PathBuf,
    /// The git target ref (e.g. "origin/main") — used in conflict footer.
    #[serde(skip)]
    pub target: String,
    /// The strategy used (e.g. "rebase", "merge") — used in conflict footer.
    #[serde(skip)]
    pub strategy: String,
}

#[derive(Serialize)]
pub struct SyncAbortOutput {
    pub workspace: String,
    pub repos: Vec<SyncAbortRepoResult>,
}

#[derive(Serialize)]
pub struct SyncAbortRepoResult {
    pub identity: String,
    pub shortname: String,
    pub path: String,
    pub action: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// Doctor output types (moved from cli/doctor.rs so Output enum can reference them)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct DoctorOutput {
    pub ok: bool,
    pub checks: Vec<DoctorCheck>,
    pub summary: DoctorSummary,
}

#[derive(Debug, Clone, Serialize)]
pub struct DoctorCheck {
    pub scope: String,
    pub check: String,
    pub status: CheckStatus,
    pub message: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub fixable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Ok,
    Warn,
    Error,
}

#[derive(Debug, Clone, Serialize)]
pub struct DoctorSummary {
    pub total: usize,
    pub ok: usize,
    pub warn: usize,
    pub error: usize,
    pub fixed: usize,
}

// ---------------------------------------------------------------------------
// Sample constructors for SKILL.md generation (codegen only)
// ---------------------------------------------------------------------------

#[cfg(feature = "codegen")]
impl RepoListOutput {
    pub fn sample() -> Self {
        Self {
            repos: vec![RepoListEntry {
                identity: "github.com/acme/api-gateway".into(),
                shortname: "api-gateway".into(),
                url: "git@github.com:acme/api-gateway.git".into(),
            }],
        }
    }
}

#[cfg(feature = "codegen")]
impl TemplateListOutput {
    pub fn sample() -> Self {
        Self {
            templates: vec![TemplateListEntry {
                name: "backend".into(),
                repo_count: 3,
            }],
        }
    }
}

#[cfg(feature = "codegen")]
impl TemplateShowOutput {
    pub fn sample() -> Self {
        Self {
            name: "backend".into(),
            repos: vec![
                TemplateShowRepo {
                    url: "git@github.com:acme/api-gateway.git".into(),
                    identity: "github.com/acme/api-gateway".into(),
                },
                TemplateShowRepo {
                    url: "git@github.com:acme/user-service.git".into(),
                    identity: "github.com/acme/user-service".into(),
                },
            ],
        }
    }
}

#[cfg(feature = "codegen")]
impl WorkspaceListOutput {
    pub fn sample() -> Self {
        Self {
            hint: Some("1 removed workspace recoverable (wsp ls --removed)".into()),
            state: ListState::Active,
            workspaces: vec![WorkspaceListEntry {
                name: "my-feature".into(),
                branch: "my-feature".into(),
                repo_count: 2,
                repos: vec![
                    "github.com/acme/api-gateway".into(),
                    "github.com/acme/user-service".into(),
                ],
                path: "/home/user/dev/workspaces/my-feature".into(),
                removed_at: None,
                expires_at: None,
                description: Some("migrating billing to stripe v3".into()),
                created: "2026-03-01T10:00:00+00:00".into(),
                last_used: Some("2026-03-06T15:30:00+00:00".into()),
                created_from: Some("backend".into()),
            }],
        }
    }
}

#[cfg(feature = "codegen")]
impl WorkspaceListOutput {
    /// The `--removed` listing. A separate sample because `state`, `removed_at`
    /// and `expires_at` are absent from the active one, and an agent cannot
    /// guess a shape it has never seen.
    pub fn sample_removed() -> Self {
        Self {
            hint: None,
            state: ListState::Removed,
            workspaces: vec![WorkspaceListEntry {
                name: "old-feature".into(),
                branch: "old-feature".into(),
                repo_count: 1,
                repos: vec!["github.com/acme/api-gateway".into()],
                path: "/home/user/.local/share/wsp/gc/old-feature__20260301T100000.000".into(),
                removed_at: Some("2026-03-01T10:00:00+00:00".into()),
                expires_at: Some("2026-03-08T10:00:00+00:00".into()),
                description: None,
                created: String::new(),
                last_used: None,
                created_from: None,
            }],
        }
    }
}

#[cfg(feature = "codegen")]
impl StatusOutput {
    pub fn sample() -> Self {
        Self {
            workspace: "my-feature".into(),
            branch: "my-feature".into(),
            description: Some("migrating billing to stripe v3".into()),
            workspace_dir: PathBuf::from("/home/user/dev/workspaces/my-feature"),
            created: "2026-01-15T10:00:00Z".parse::<DateTime<Utc>>().unwrap(),
            repos: vec![RepoStatusEntry {
                identity: "github.com/acme/api-gateway".into(),
                shortname: "api-gateway".into(),
                path: "/home/user/dev/workspaces/my-feature/api-gateway".into(),
                branch: "my-feature".into(),
                ahead: 2,
                behind: 0,
                changed: 1,
                has_upstream: true,
                role: "active".into(),
                files: vec![],
                error: None,
                expected_branch: None,
                pr: None,
            }],
            root: vec![],
            verbose: false,
            pr_enabled: false,
        }
    }
}

#[cfg(feature = "codegen")]
impl DiffOutput {
    pub fn sample() -> Self {
        Self {
            workspace: "my-feature".into(),
            branch: "my-feature".into(),
            workspace_dir: PathBuf::from("/home/user/dev/workspaces/my-feature"),
            repos: vec![RepoDiffEntry {
                identity: "github.com/acme/api-gateway".into(),
                shortname: "api-gateway".into(),
                path: "/home/user/dev/workspaces/my-feature/api-gateway".into(),
                diff: "--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1,3 +1,4 @@\n+use std::io;\n ..."
                    .into(),
                error: None,
            }],
        }
    }
}

#[cfg(feature = "codegen")]
impl LogOutput {
    pub fn sample() -> Self {
        Self {
            workspace: "my-feature".into(),
            branch: "my-feature".into(),
            workspace_dir: PathBuf::from("/home/user/dev/workspaces/my-feature"),
            oneline: false,
            repos: vec![RepoLogEntry {
                identity: "github.com/acme/api-gateway".into(),
                shortname: "api-gateway".into(),
                path: "/home/user/dev/workspaces/my-feature/api-gateway".into(),
                commits: vec![LogCommit {
                    hash: "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2".into(),
                    authored_at: "2023-11-14T22:13:20+00:00".into(),
                    timestamp: 1700000000,
                    subject: "feat: add billing endpoint".into(),
                }],
                raw: None,
                error: None,
            }],
        }
    }
}

#[cfg(feature = "codegen")]
impl SyncOutput {
    pub fn sample() -> Self {
        Self {
            workspace: "my-feature".into(),
            branch: "my-feature".into(),
            dry_run: false,
            repos: vec![SyncRepoResult {
                identity: "github.com/acme/api-gateway".into(),
                shortname: "api-gateway".into(),
                path: "/home/user/dev/workspaces/my-feature/api-gateway".into(),
                action: "rebase onto origin/main".into(),
                ok: true,
                detail: Some("2 commit(s) rebased".into()),
                error: None,
                repo_dir: PathBuf::from("/tmp"),
                target: String::new(),
                strategy: String::new(),
            }],
        }
    }
}

#[cfg(feature = "codegen")]
impl SyncAbortOutput {
    pub fn sample() -> Self {
        Self {
            workspace: "my-feature".into(),
            repos: vec![
                SyncAbortRepoResult {
                    identity: "github.com/acme/api-gateway".into(),
                    shortname: "api-gateway".into(),
                    path: "/home/user/dev/workspaces/my-feature/api-gateway".into(),
                    action: "skip".into(),
                    ok: true,
                    error: None,
                },
                SyncAbortRepoResult {
                    identity: "github.com/acme/user-service".into(),
                    shortname: "user-service".into(),
                    path: "/home/user/dev/workspaces/my-feature/user-service".into(),
                    action: "rebase aborted".into(),
                    ok: true,
                    error: None,
                },
            ],
        }
    }
}

#[cfg(feature = "codegen")]
impl ConfigListOutput {
    pub fn sample() -> Self {
        Self {
            entries: vec![
                ConfigListEntry {
                    key: "branch-prefix".into(),
                    value: "jg".into(),
                    source: None,
                    experimental: false,
                },
                ConfigListEntry {
                    key: "workspaces-dir".into(),
                    value: "~/dev/workspaces".into(),
                    source: None,
                    experimental: false,
                },
                ConfigListEntry {
                    key: "sync-strategy".into(),
                    value: "rebase".into(),
                    source: Some("workspace".into()),
                    experimental: false,
                },
            ],
        }
    }
}

#[cfg(feature = "codegen")]
impl ConfigGetOutput {
    pub fn sample() -> Self {
        Self {
            key: "branch-prefix".into(),
            value: Some("jg".into()),
        }
    }
}

#[cfg(feature = "codegen")]
impl WorkspaceRepoListOutput {
    pub fn sample() -> Self {
        Self {
            workspace: "my-feature".into(),
            branch: "my-feature".into(),
            workspace_dir: PathBuf::from("/home/user/dev/workspaces/my-feature"),
            repos: vec![
                WorkspaceRepoListEntry {
                    identity: "github.com/acme/api-gateway".into(),
                    shortname: "api-gateway".into(),
                    dir_name: "api-gateway".into(),
                },
                WorkspaceRepoListEntry {
                    identity: "github.com/acme/shared-lib".into(),
                    shortname: "shared-lib".into(),
                    dir_name: "shared-lib".into(),
                },
            ],
        }
    }
}

#[cfg(feature = "codegen")]
impl ExecOutput {
    pub fn sample() -> Self {
        Self {
            workspace: "my-feature".into(),
            repos: vec![
                ExecRepoResult {
                    identity: "github.com/acme/api-gateway".into(),
                    shortname: "api-gateway".into(),
                    path: "/home/user/dev/workspaces/my-feature/api-gateway".into(),
                    directory: "api-gateway".into(),
                    exit_code: 0,
                    signal: None,
                    ok: true,
                    stdout: Some("hello\n".into()),
                    stderr: None,
                    error: None,
                },
                // A second repo that was signalled, so the generated docs show
                // `signal` at all. It is omitted when absent, so a sample with
                // only a successful repo would never mention it and an agent
                // would have no way to learn it exists.
                ExecRepoResult {
                    identity: "github.com/acme/user-service".into(),
                    shortname: "user-service".into(),
                    path: "/home/user/dev/workspaces/my-feature/user-service".into(),
                    directory: "user-service".into(),
                    exit_code: -1,
                    signal: Some(15),
                    ok: false,
                    stdout: Some(String::new()),
                    stderr: None,
                    error: None,
                },
            ],
        }
    }
}

#[cfg(feature = "codegen")]
impl FetchOutput {
    pub fn sample() -> Self {
        Self {
            workspace: "my-feature".into(),
            repos: vec![FetchRepoResult {
                identity: "github.com/acme/api-gateway".into(),
                shortname: "api-gateway".into(),
                ok: true,
                error: None,
            }],
        }
    }
}

#[cfg(feature = "codegen")]
impl MutationOutput {
    pub fn sample() -> Self {
        Self {
            ok: true,
            message: "Registered github.com/acme/api-gateway".into(),
            duration_ms: None,
            hint: None,
            workspace: None,
            path: None,
            branch: None,
        }
    }
}

#[cfg(feature = "codegen")]
impl ErrorOutput {
    pub fn sample() -> Self {
        Self {
            error: "repo \"foo\" not found".into(),
        }
    }
}

#[cfg(feature = "codegen")]
impl ImportOutput {
    pub fn sample() -> Self {
        Self {
            registered: vec![
                "github.com/acme/api-gateway".into(),
                "github.com/acme/user-service".into(),
            ],
            skipped: vec!["github.com/acme/shared-lib".into()],
            failed: vec![],
        }
    }
}

#[cfg(feature = "codegen")]
impl DoctorOutput {
    pub fn sample() -> Self {
        Self {
            ok: false,
            checks: vec![
                DoctorCheck {
                    scope: "global".into(),
                    check: "config-parseable".into(),
                    status: CheckStatus::Ok,
                    message: "config is valid (5 registered repos)".into(),
                    fixable: false,
                    details: None,
                },
                DoctorCheck {
                    scope: "workspace/my-feature/bar".into(),
                    check: "origin-url-match".into(),
                    status: CheckStatus::Warn,
                    message: "bar: origin URL differs from registered URL".into(),
                    fixable: true,
                    details: Some(serde_json::json!({
                        "clone_url": "git@github.com:acme/bar.git",
                        "registered_url": "https://github.com/acme/bar"
                    })),
                },
            ],
            summary: DoctorSummary {
                total: 8,
                ok: 7,
                warn: 1,
                error: 0,
                fixed: 0,
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Setup commands output
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct SetupCommandsOutput {
    pub repo: String,
    pub commands: Vec<SetupCommandEntry>,
}

#[derive(Debug, Serialize)]
pub struct SetupCommandEntry {
    pub command: String,
    pub source: String,
}

// ---------------------------------------------------------------------------
// Output enum — returned by all command handlers
// ---------------------------------------------------------------------------

pub enum Output {
    RepoList(RepoListOutput),
    TemplateList(TemplateListOutput),
    TemplateShow(TemplateShowOutput),
    WorkspaceList(WorkspaceListOutput),
    WorkspaceRepoList(WorkspaceRepoListOutput),
    Status(StatusOutput),
    Diff(DiffOutput),
    Log(LogOutput),
    Exec(ExecOutput),
    Fetch(FetchOutput),
    Sync(SyncOutput),
    SyncAbort(SyncAbortOutput),
    ConfigList(ConfigListOutput),
    ConfigGet(ConfigGetOutput),
    Mutation(MutationOutput),
    Import(ImportOutput),
    Path(PathOutput),
    Doctor(DoctorOutput),
    SetupCommands(SetupCommandsOutput),
    None,
}
