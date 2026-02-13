use std::io::Write;

use anyhow::{Result, bail};
use serde::Serialize;
use tabwriter::TabWriter;

// ---------------------------------------------------------------------------
// Table helper (existing)
// ---------------------------------------------------------------------------

pub struct Table {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
    dest: Box<dyn Write>,
}

impl Table {
    pub fn new(w: Box<dyn Write>, headers: Vec<String>) -> Self {
        Table {
            headers,
            rows: Vec::new(),
            dest: w,
        }
    }

    pub fn add_row(&mut self, columns: Vec<String>) -> Result<()> {
        if columns.len() != self.headers.len() {
            bail!(
                "row has {} columns, expected {}",
                columns.len(),
                self.headers.len()
            );
        }
        self.rows.push(columns);
        Ok(())
    }

    pub fn render(&mut self) -> Result<()> {
        if self.headers.is_empty() {
            return Ok(());
        }

        let buf = render_buf(&self.headers, &self.rows)?;
        self.dest.write_all(&buf)?;
        Ok(())
    }
}

fn render_buf(headers: &[String], rows: &[Vec<String>]) -> Result<Vec<u8>> {
    let mut tw = TabWriter::new(Vec::new()).minwidth(0).padding(2);

    let upper: Vec<String> = headers.iter().map(|h| h.to_uppercase()).collect();
    writeln!(tw, "{}", upper.join("\t"))?;

    for row in rows {
        writeln!(tw, "{}", row.join("\t"))?;
    }

    tw.flush()?;
    Ok(tw.into_inner()?)
}

pub fn format_repo_status(ahead: u32, modified: u32, has_upstream: bool) -> String {
    if ahead == 0 && modified == 0 {
        return "clean".to_string();
    }
    let mut parts = Vec::new();
    if ahead > 0 {
        if has_upstream {
            parts.push(format!("{} ahead", ahead));
        } else {
            parts.push(format!("{} ahead (no upstream)", ahead));
        }
    }
    if modified > 0 {
        parts.push(format!("{} modified", modified));
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
pub struct GroupListOutput {
    pub groups: Vec<GroupListEntry>,
}

#[derive(Serialize)]
pub struct GroupListEntry {
    pub name: String,
    pub repo_count: usize,
}

#[derive(Serialize)]
pub struct GroupShowOutput {
    pub name: String,
    pub repos: Vec<String>,
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
}

#[derive(Serialize)]
pub struct StatusOutput {
    pub workspace: String,
    pub branch: String,
    pub repos: Vec<RepoStatusEntry>,
}

#[derive(Serialize)]
pub struct RepoStatusEntry {
    pub name: String,
    pub branch: String,
    pub ahead: u32,
    pub changed: u32,
    pub has_upstream: bool,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Serialize)]
pub struct DiffOutput {
    pub repos: Vec<RepoDiffEntry>,
}

#[derive(Serialize)]
pub struct RepoDiffEntry {
    pub name: String,
    pub diff: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Serialize)]
pub struct ConfigListOutput {
    pub entries: Vec<ConfigListEntry>,
}

#[derive(Serialize)]
pub struct ConfigListEntry {
    pub key: String,
    pub value: String,
}

#[derive(Serialize)]
pub struct ConfigGetOutput {
    pub key: String,
    pub value: Option<String>,
}

#[derive(Serialize)]
pub struct FetchOutput {
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
}

#[derive(Serialize)]
pub struct PathOutput {
    pub path: String,
}

#[derive(Serialize)]
pub struct ErrorOutput {
    pub error: String,
}

// ---------------------------------------------------------------------------
// Output enum — returned by all command handlers
// ---------------------------------------------------------------------------

pub enum Output {
    RepoList(RepoListOutput),
    GroupList(GroupListOutput),
    GroupShow(GroupShowOutput),
    WorkspaceList(WorkspaceListOutput),
    Status(StatusOutput),
    Diff(DiffOutput),
    Fetch(FetchOutput),
    ConfigList(ConfigListOutput),
    ConfigGet(ConfigGetOutput),
    Mutation(MutationOutput),
    Path(PathOutput),
    None,
}

// ---------------------------------------------------------------------------
// Central render function
// ---------------------------------------------------------------------------

pub fn render(output: Output, json: bool) -> Result<()> {
    if json {
        return match output {
            Output::None => Ok(()),
            Output::RepoList(v) => print_json(&v),
            Output::GroupList(v) => print_json(&v),
            Output::GroupShow(v) => print_json(&v),
            Output::WorkspaceList(v) => print_json(&v),
            Output::Status(v) => print_json(&v),
            Output::Diff(v) => print_json(&v),
            Output::Fetch(v) => print_json(&v),
            Output::ConfigList(v) => print_json(&v),
            Output::ConfigGet(v) => print_json(&v),
            Output::Mutation(v) => print_json(&v),
            Output::Path(v) => print_json(&v),
        };
    }
    match output {
        Output::None => Ok(()),
        Output::RepoList(v) => render_repo_list_table(v),
        Output::GroupList(v) => render_group_list_table(v),
        Output::GroupShow(v) => render_group_show_text(v),
        Output::WorkspaceList(v) => render_workspace_list_table(v),
        Output::Status(v) => render_status_table(v),
        Output::Diff(v) => render_diff_text(v),
        Output::Fetch(v) => render_fetch_text(v),
        Output::ConfigList(v) => render_config_list_text(v),
        Output::ConfigGet(v) => render_config_get_text(v),
        Output::Mutation(v) => render_mutation_text(v),
        Output::Path(v) => render_path_text(v),
    }
}

/// Returns non-zero exit code for batch outputs with failures.
pub fn exit_code(output: &Output) -> i32 {
    match output {
        Output::Fetch(v) if v.repos.iter().any(|r| !r.ok) => 1,
        _ => 0,
    }
}

fn print_json(value: &impl Serialize) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

// ---------------------------------------------------------------------------
// Text/table renderers
// ---------------------------------------------------------------------------

fn render_repo_list_table(v: RepoListOutput) -> Result<()> {
    if v.repos.is_empty() {
        println!("No repos registered.");
        return Ok(());
    }
    let mut table = Table::new(
        Box::new(std::io::stdout()),
        vec![
            "Identity".to_string(),
            "Shortname".to_string(),
            "URL".to_string(),
        ],
    );
    for r in &v.repos {
        table.add_row(vec![r.identity.clone(), r.shortname.clone(), r.url.clone()])?;
    }
    table.render()
}

fn render_group_list_table(v: GroupListOutput) -> Result<()> {
    if v.groups.is_empty() {
        println!("No groups defined.");
        return Ok(());
    }
    let mut table = Table::new(
        Box::new(std::io::stdout()),
        vec!["Name".to_string(), "Repos".to_string()],
    );
    for g in &v.groups {
        table.add_row(vec![g.name.clone(), g.repo_count.to_string()])?;
    }
    table.render()
}

fn render_group_show_text(v: GroupShowOutput) -> Result<()> {
    println!("Group {:?}:", v.name);
    for r in &v.repos {
        println!("  {}", r);
    }
    Ok(())
}

fn render_workspace_list_table(v: WorkspaceListOutput) -> Result<()> {
    if let Some(hint) = &v.hint {
        println!("{}\n", hint);
    }
    if v.workspaces.is_empty() {
        println!("No workspaces.");
        return Ok(());
    }
    let mut table = Table::new(
        Box::new(std::io::stdout()),
        vec![
            "Name".to_string(),
            "Branch".to_string(),
            "Repos".to_string(),
            "Path".to_string(),
        ],
    );
    for ws in &v.workspaces {
        table.add_row(vec![
            ws.name.clone(),
            ws.branch.clone(),
            ws.repo_count.to_string(),
            ws.path.clone(),
        ])?;
    }
    table.render()
}

fn render_status_table(v: StatusOutput) -> Result<()> {
    println!("Workspace: {}  Branch: {}\n", v.workspace, v.branch);
    let mut table = Table::new(
        Box::new(std::io::stdout()),
        vec![
            "Repository".to_string(),
            "Branch".to_string(),
            "Status".to_string(),
        ],
    );
    for rs in &v.repos {
        let status = if let Some(ref e) = rs.error {
            format_error(e)
        } else {
            rs.status.clone()
        };
        table.add_row(vec![rs.name.clone(), rs.branch.clone(), status])?;
    }
    table.render()
}

fn render_diff_text(v: DiffOutput) -> Result<()> {
    let mut first = true;
    for entry in &v.repos {
        if let Some(ref e) = entry.error {
            eprintln!("[{}] error: {}", entry.name, e);
            continue;
        }
        if entry.diff.is_empty() {
            continue;
        }
        if !first {
            println!();
        }
        println!("==> [{}]", entry.name);
        println!("{}", entry.diff);
        first = false;
    }
    Ok(())
}

fn render_fetch_text(v: FetchOutput) -> Result<()> {
    let total = v.repos.len();
    let failed = v.repos.iter().filter(|r| !r.ok).count();
    if failed == 0 {
        println!("Fetched {} repo(s)", total);
    } else {
        println!("Fetched {} repo(s), {} failed", total - failed, failed);
    }
    Ok(())
}

fn render_config_list_text(v: ConfigListOutput) -> Result<()> {
    if v.entries.is_empty() {
        println!("No config values set.");
        return Ok(());
    }
    let mut table = Table::new(
        Box::new(std::io::stdout()),
        vec!["Key".to_string(), "Value".to_string()],
    );
    for e in &v.entries {
        table.add_row(vec![e.key.clone(), e.value.clone()])?;
    }
    table.render()
}

fn render_config_get_text(v: ConfigGetOutput) -> Result<()> {
    match &v.value {
        Some(val) => println!("{}", val),
        None => println!("(not set)"),
    }
    Ok(())
}

fn render_mutation_text(v: MutationOutput) -> Result<()> {
    println!("{}", v.message);
    Ok(())
}

fn render_path_text(v: PathOutput) -> Result<()> {
    println!("{}", v.path);
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn normalize_whitespace(s: &str) -> String {
        s.lines()
            .map(|line| line.trim_end())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn render_to_string(headers: &[String], rows: &[Vec<String>]) -> String {
        if headers.is_empty() {
            return String::new();
        }
        let buf = render_buf(headers, rows).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn test_table() {
        let cases: Vec<(&str, Vec<&str>, Vec<Vec<&str>>, &str)> = vec![
            (
                "single column",
                vec!["Name"],
                vec![vec!["Alice"], vec!["Bob"]],
                "NAME\nAlice\nBob\n",
            ),
            (
                "two columns aligned",
                vec!["Name", "Status"],
                vec![
                    vec!["api-gateway", "clean"],
                    vec!["user-service", "2 modified"],
                ],
                "NAME          STATUS\napi-gateway   clean\nuser-service  2 modified\n",
            ),
            (
                "three columns",
                vec!["Repository", "Branch", "Status"],
                vec![
                    vec!["api-gateway", "main", "clean"],
                    vec!["user-service", "feature-branch", "2 modified"],
                ],
                "REPOSITORY    BRANCH          STATUS\napi-gateway   main            clean\nuser-service  feature-branch  2 modified\n",
            ),
            (
                "headers only no rows",
                vec!["Name", "Age"],
                vec![],
                "NAME  AGE\n",
            ),
            ("no headers", vec![], vec![], ""),
        ];
        for (name, headers, rows, want) in cases {
            let headers_owned: Vec<String> = headers.iter().map(|s| s.to_string()).collect();
            let rows_owned: Vec<Vec<String>> = rows
                .iter()
                .map(|r| r.iter().map(|s| s.to_string()).collect())
                .collect();
            let output = render_to_string(&headers_owned, &rows_owned);
            assert_eq!(
                normalize_whitespace(&output),
                normalize_whitespace(want),
                "{}",
                name
            );
        }
    }

    #[test]
    fn test_table_column_mismatch() {
        let mut table = Table::new(Box::new(std::io::sink()), vec!["Name".into(), "Age".into()]);

        let err = table.add_row(vec!["Alice".into(), "30".into(), "extra".into()]);
        assert!(err.is_err());
        assert!(
            err.unwrap_err()
                .to_string()
                .contains("3 columns, expected 2")
        );

        let err = table.add_row(vec!["Bob".into()]);
        assert!(err.is_err());
        assert!(
            err.unwrap_err()
                .to_string()
                .contains("1 columns, expected 2")
        );
    }

    #[test]
    fn test_format_repo_status() {
        let cases = vec![
            ("clean", 0, 0, true, "clean"),
            ("clean no upstream", 0, 0, false, "clean"),
            ("modified only", 0, 5, true, "5 modified"),
            ("ahead with upstream", 3, 0, true, "3 ahead"),
            ("ahead no upstream", 3, 0, false, "3 ahead (no upstream)"),
            ("both with upstream", 2, 4, true, "2 ahead, 4 modified"),
            (
                "both no upstream",
                2,
                4,
                false,
                "2 ahead (no upstream), 4 modified",
            ),
            ("one each", 1, 1, true, "1 ahead, 1 modified"),
        ];
        for (name, ahead, modified, has_upstream, want) in cases {
            assert_eq!(
                format_repo_status(ahead, modified, has_upstream),
                want,
                "{}",
                name
            );
        }
    }

    #[test]
    fn test_format_error() {
        assert_eq!(format_error(&"something broke"), "ERROR: something broke");
    }

    #[test]
    fn test_json_repo_list() {
        let output = RepoListOutput {
            repos: vec![RepoListEntry {
                identity: "github.com/user/repo".into(),
                shortname: "repo".into(),
                url: "git@github.com:user/repo.git".into(),
            }],
        };
        let val = serde_json::to_value(&output).unwrap();
        assert!(val["repos"].is_array());
        assert_eq!(val["repos"][0]["identity"], "github.com/user/repo");
        assert_eq!(val["repos"][0]["shortname"], "repo");
        assert_eq!(val["repos"][0]["url"], "git@github.com:user/repo.git");
    }

    #[test]
    fn test_json_repo_list_empty() {
        let output = RepoListOutput { repos: vec![] };
        let val = serde_json::to_value(&output).unwrap();
        assert_eq!(val["repos"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn test_json_group_list() {
        let output = GroupListOutput {
            groups: vec![GroupListEntry {
                name: "backend".into(),
                repo_count: 3,
            }],
        };
        let val = serde_json::to_value(&output).unwrap();
        assert_eq!(val["groups"][0]["name"], "backend");
        assert_eq!(val["groups"][0]["repo_count"], 3);
    }

    #[test]
    fn test_json_group_show() {
        let output = GroupShowOutput {
            name: "backend".into(),
            repos: vec!["repo-a".into(), "repo-b".into()],
        };
        let val = serde_json::to_value(&output).unwrap();
        assert_eq!(val["name"], "backend");
        assert_eq!(val["repos"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn test_json_workspace_list() {
        let output = WorkspaceListOutput {
            hint: None,
            workspaces: vec![WorkspaceListEntry {
                name: "my-ws".into(),
                branch: "my-ws".into(),
                repo_count: 2,
                path: "/home/user/dev/workspaces/my-ws".into(),
            }],
        };
        let val = serde_json::to_value(&output).unwrap();
        assert_eq!(val["workspaces"][0]["name"], "my-ws");
        assert_eq!(val["workspaces"][0]["repo_count"], 2);
    }

    #[test]
    fn test_json_status() {
        let output = StatusOutput {
            workspace: "my-ws".into(),
            branch: "my-ws".into(),
            repos: vec![
                RepoStatusEntry {
                    name: "repo-a".into(),
                    branch: "my-ws".into(),
                    ahead: 1,
                    changed: 2,
                    has_upstream: true,
                    status: "1 ahead, 2 modified".into(),
                    error: None,
                },
                RepoStatusEntry {
                    name: "repo-b".into(),
                    branch: String::new(),
                    ahead: 0,
                    changed: 0,
                    has_upstream: false,
                    status: String::new(),
                    error: Some("parse error".into()),
                },
            ],
        };
        let val = serde_json::to_value(&output).unwrap();
        assert_eq!(val["workspace"], "my-ws");
        assert_eq!(val["repos"][0]["ahead"], 1);
        assert_eq!(val["repos"][0]["changed"], 2);
        assert_eq!(val["repos"][0]["has_upstream"], true);
        assert!(val["repos"][0].get("error").is_none());
        assert_eq!(val["repos"][1]["has_upstream"], false);
        assert_eq!(val["repos"][1]["error"], "parse error");
    }

    #[test]
    fn test_json_diff() {
        let output = DiffOutput {
            repos: vec![
                RepoDiffEntry {
                    name: "repo-a".into(),
                    diff: "--- a/file\n+++ b/file".into(),
                    error: None,
                },
                RepoDiffEntry {
                    name: "repo-b".into(),
                    diff: String::new(),
                    error: Some("not found".into()),
                },
            ],
        };
        let val = serde_json::to_value(&output).unwrap();
        assert_eq!(val["repos"][0]["diff"], "--- a/file\n+++ b/file");
        assert!(val["repos"][0].get("error").is_none());
        assert_eq!(val["repos"][1]["error"], "not found");
    }

    #[test]
    fn test_json_config_get() {
        let cases = vec![
            (
                "with value",
                ConfigGetOutput {
                    key: "branch-prefix".into(),
                    value: Some("myname".into()),
                },
            ),
            (
                "no value",
                ConfigGetOutput {
                    key: "branch-prefix".into(),
                    value: None,
                },
            ),
        ];
        for (name, output) in cases {
            let val = serde_json::to_value(&output).unwrap();
            assert_eq!(val["key"], "branch-prefix", "{}", name);
        }
    }

    #[test]
    fn test_json_mutation() {
        let output = MutationOutput {
            ok: true,
            message: "Registered repo".into(),
        };
        let val = serde_json::to_value(&output).unwrap();
        assert_eq!(val["ok"], true);
        assert_eq!(val["message"], "Registered repo");
    }

    #[test]
    fn test_json_error() {
        let output = ErrorOutput {
            error: "something went wrong".into(),
        };
        let val = serde_json::to_value(&output).unwrap();
        assert_eq!(val["error"], "something went wrong");
    }
}
