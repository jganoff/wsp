use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::Serialize;

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

#[derive(Serialize)]
pub struct WorkspaceListOutput {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    pub workspaces: Vec<WorkspaceListEntry>,
}

#[derive(Serialize)]
pub struct WorkspaceListEntry {
    pub name: String,
    pub branch: String,
    pub repo_count: usize,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
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
    pub exit_code: i32,
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
pub struct RecoverListOutput {
    #[serde(rename = "workspaces")]
    pub entries: Vec<crate::gc::GcListEntry>,
    pub retention_days: u32,
}

#[derive(Serialize)]
pub struct RecoverShowOutput {
    pub entry: crate::gc::GcShowEntry,
    pub retention_days: u32,
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
            hint: None,
            workspaces: vec![WorkspaceListEntry {
                name: "my-feature".into(),
                branch: "my-feature".into(),
                repo_count: 2,
                path: "/home/user/dev/workspaces/my-feature".into(),
                description: Some("migrating billing to stripe v3".into()),
                created: "2026-03-01T10:00:00+00:00".into(),
                last_used: Some("2026-03-06T15:30:00+00:00".into()),
                created_from: Some("backend".into()),
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
            }],
            root: vec![],
            verbose: false,
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
            repos: vec![ExecRepoResult {
                identity: "github.com/acme/api-gateway".into(),
                shortname: "api-gateway".into(),
                path: "/home/user/dev/workspaces/my-feature/api-gateway".into(),
                directory: "api-gateway".into(),
                exit_code: 0,
                ok: true,
                stdout: Some("hello\n".into()),
                stderr: None,
                error: None,
            }],
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
impl RecoverListOutput {
    pub fn sample() -> Self {
        use chrono::Utc;
        Self {
            entries: vec![crate::gc::GcListEntry {
                entry: crate::gc::GcEntry {
                    name: "my-feature".into(),
                    branch: "jganoff/my-feature".into(),
                    trashed_at: "2026-01-01T00:00:00Z"
                        .parse::<chrono::DateTime<Utc>>()
                        .unwrap(),
                    original_path: "~/dev/workspaces/my-feature".into(),
                },
                repo_count: 3,
            }],
            retention_days: 7,
        }
    }
}

#[cfg(feature = "codegen")]
impl RecoverShowOutput {
    pub fn sample() -> Self {
        use chrono::Utc;
        Self {
            entry: crate::gc::GcShowEntry {
                entry: crate::gc::GcEntry {
                    name: "my-feature".into(),
                    branch: "jganoff/my-feature".into(),
                    trashed_at: "2026-01-01T00:00:00Z"
                        .parse::<chrono::DateTime<Utc>>()
                        .unwrap(),
                    original_path: "~/dev/workspaces/my-feature".into(),
                },
                repos: vec![
                    "github.com/acme/api-gateway".into(),
                    "github.com/acme/user-service".into(),
                ],
                disk_bytes: 52_428_800,
                gc_path: "~/.local/share/wsp/gc/my-feature__20260101T000000.000".into(),
            },
            retention_days: 7,
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
    RecoverList(RecoverListOutput),
    RecoverShow(RecoverShowOutput),
    Path(PathOutput),
    Doctor(DoctorOutput),
    SetupCommands(SetupCommandsOutput),
    None,
}
