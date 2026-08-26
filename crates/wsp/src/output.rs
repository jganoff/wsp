use std::io::Write;

use anyhow::{Result, bail};
use owo_colors::{OwoColorize, Stream::Stderr};
use tabwriter::TabWriter;

use serde::Serialize;
// Wildcard re-export: this module is the binary rendering layer and deliberately
// has access to every output type from wsp-core. Types are not re-exported
// publicly from this module — the glob stays internal to crates/wsp.
use wsp_core::output::*;

// ---------------------------------------------------------------------------
// GC warning printer (owo-colors belongs in the binary crate)
// ---------------------------------------------------------------------------

/// Print a gc workspace warning to stderr with bold+yellow ANSI styling.
pub fn print_gc_warning(warning: &str) {
    eprintln!(
        "{}",
        warning
            .if_supports_color(Stderr, |t| t.bold())
            .if_supports_color(Stderr, |t| t.yellow())
    );
}

// ---------------------------------------------------------------------------
// Table helper
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

// ---------------------------------------------------------------------------
// Central render function + exit_code
// ---------------------------------------------------------------------------

pub fn render(output: Output, json: bool) -> Result<()> {
    if json {
        return match output {
            Output::None => Ok(()),
            Output::RepoList(v) => print_json(&v),
            Output::TemplateList(v) => print_json(&v),
            Output::TemplateShow(v) => print_json(&v),
            Output::WorkspaceList(v) => print_json(&v),
            Output::WorkspaceRepoList(v) => print_json(&v),
            Output::Status(v) => print_json(&v),
            Output::Diff(v) => print_json(&v),
            Output::Log(v) => print_json(&v),
            Output::Exec(v) => print_json(&v),
            Output::Fetch(v) => print_json(&v),
            Output::Sync(v) => print_json(&v),
            Output::SyncAbort(v) => print_json(&v),
            Output::ConfigList(v) => print_json(&v),
            Output::ConfigGet(v) => print_json(&v),
            Output::Mutation(v) => print_json(&v),
            Output::Import(v) => print_json(&v),
            Output::Path(v) => print_json(&v),
            Output::Doctor(v) => print_json(&v),
            Output::SetupCommands(v) => print_json(&v),
        };
    }
    match output {
        Output::None => Ok(()),
        Output::RepoList(v) => render_repo_list_table(v),
        Output::TemplateList(v) => render_template_list_table(v),
        Output::TemplateShow(v) => render_template_show_text(v),
        Output::WorkspaceList(v) => render_workspace_list_table(v),
        Output::WorkspaceRepoList(v) => render_workspace_repo_list_table(v),
        Output::Status(v) => render_status_table(v),
        Output::Diff(v) => render_diff_text(v),
        Output::Log(v) => render_log_text(v),
        Output::Exec(_) => Ok(()), // text output handled inline during execution
        Output::Fetch(v) => render_fetch_text(v),
        Output::Sync(v) => render_sync_text(v),
        Output::SyncAbort(v) => render_sync_abort_text(v),
        Output::ConfigList(v) => render_config_list_text(v),
        Output::ConfigGet(v) => render_config_get_text(v),
        Output::Mutation(v) => render_mutation_text(v),
        Output::Import(v) => render_import_text(v),
        Output::Path(v) => render_path_text(v),
        Output::Doctor(_) => Ok(()), // text output handled inline during run
        Output::SetupCommands(v) => render_setup_commands_text(v),
    }
}

/// Returns non-zero exit code for batch outputs with failures.
pub fn exit_code(output: &Output) -> i32 {
    match output {
        Output::Exec(v) if v.repos.iter().any(|r| !r.ok) => 1,
        Output::Fetch(v) if v.repos.iter().any(|r| !r.ok) => 1,
        Output::Sync(v) if v.repos.iter().any(|r| !r.ok) => 1,
        Output::SyncAbort(v) if v.repos.iter().any(|r| !r.ok) => 1,
        Output::Import(v) if !v.failed.is_empty() => 1,
        Output::Doctor(v) => crate::cli::doctor::exit_code(v),
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

fn render_template_list_table(v: TemplateListOutput) -> Result<()> {
    if v.templates.is_empty() {
        println!("No templates defined.");
        return Ok(());
    }
    let mut table = Table::new(
        Box::new(std::io::stdout()),
        vec!["Name".to_string(), "Repos".to_string()],
    );
    for t in &v.templates {
        table.add_row(vec![t.name.clone(), t.repo_count.to_string()])?;
    }
    table.render()
}

fn render_template_show_text(v: TemplateShowOutput) -> Result<()> {
    println!("Template {:?}:", v.name);
    for r in &v.repos {
        println!("  {} ({})", r.identity, r.url);
    }
    Ok(())
}

fn render_workspace_list_table(v: WorkspaceListOutput) -> Result<()> {
    let removed = match v.state {
        ListState::Removed => true,
        ListState::Active => false,
    };
    if v.workspaces.is_empty() {
        println!(
            "{}",
            if removed {
                "No removed workspaces."
            } else {
                "No workspaces."
            }
        );
        // Still print the footer: removing your only workspace is exactly when
        // you most need to hear that it is recoverable.
        if let Some(hint) = &v.hint {
            println!("\n{}", hint);
        }
        return Ok(());
    }
    // The last two columns differ by set: an active workspace has a creation
    // date and a description, a removed one has neither -- what matters is when
    // it went and how long is left.
    let mut headers = vec![
        "Name".to_string(),
        "Branch".to_string(),
        "Repos".to_string(),
    ];
    headers.extend(
        if removed {
            ["Removed", "Expires"]
        } else {
            ["Created", "Description"]
        }
        .map(String::from),
    );
    let mut table = Table::new(Box::new(std::io::stdout()), headers);

    let now = chrono::Utc::now().timestamp();
    for ws in &v.workspaces {
        let mut row = vec![
            ws.name.clone(),
            ws.branch.clone(),
            ws.repo_count.to_string(),
        ];
        if removed {
            // Both are present on every removed entry, except `expires_at` when
            // retention is disabled -- then nothing expires.
            row.push(
                rfc3339(ws.removed_at.as_deref())
                    .map_or_else(String::new, |t| format_relative_time(t.timestamp(), now)),
            );
            row.push(format_expiry(rfc3339(ws.expires_at.as_deref())));
        } else {
            row.push(
                rfc3339(Some(&ws.created))
                    .map_or_else(String::new, |t| format_relative_time(t.timestamp(), now)),
            );
            row.push(ws.description.as_deref().unwrap_or("").to_string());
        }
        table.add_row(row)?;
    }
    table.render()?;
    if let Some(hint) = &v.hint {
        println!("\n{}", hint);
    }
    Ok(())
}

fn render_workspace_repo_list_table(v: WorkspaceRepoListOutput) -> Result<()> {
    if v.repos.is_empty() {
        println!("No repos in workspace.");
        return Ok(());
    }
    let mut table = Table::new(
        Box::new(std::io::stdout()),
        vec![
            "Identity".to_string(),
            "Shortname".to_string(),
            "Dir".to_string(),
        ],
    );
    for r in &v.repos {
        table.add_row(vec![
            r.identity.clone(),
            r.shortname.clone(),
            r.dir_name.clone(),
        ])?;
    }
    table.render()
}

/// Replace control characters (ANSI escapes, carriage returns, etc.) with the
/// Unicode replacement character so user-controlled strings from external forges
/// cannot manipulate the terminal.
fn sanitize_for_terminal(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_control() && c != '\n' && c != '\t' {
                '\u{FFFD}'
            } else {
                c
            }
        })
        .collect()
}

fn pr_state_label(pr: &PrInfo) -> &str {
    match pr.state.as_str() {
        "OPEN" if pr.is_draft => "draft",
        "OPEN" => "open",
        "MERGED" => "merged",
        "CLOSED" => "closed",
        other => other,
    }
}

fn format_pr_cell(pr: &PrInfo) -> String {
    format!("#{} {}", pr.number, pr_state_label(pr))
}

fn render_status_table(v: StatusOutput) -> Result<()> {
    let now = chrono::Utc::now().timestamp();
    let created_age = format_relative_time(v.created.timestamp(), now);

    let desc_display = v.description.as_deref().unwrap_or("(none)").to_string();
    let fields: &[(&str, &str)] = &[
        ("Workspace", &v.workspace),
        ("Branch", &v.branch),
        (
            "Created",
            &format!("{} ({})", v.created.format("%Y-%m-%d %H:%M"), created_age),
        ),
        ("Description", &desc_display),
    ];
    let label_width = fields.iter().map(|(k, _)| k.len()).max().unwrap_or(0) + 1;
    for (label, value) in fields {
        println!(
            "{:<width$} {}",
            format!("{}:", label),
            value,
            width = label_width
        );
    }
    println!();

    let mut headers = vec![
        "Repository".to_string(),
        "Branch".to_string(),
        "Status".to_string(),
    ];
    if v.pr_enabled {
        headers.push("PR".to_string());
    }

    let mut table = Table::new(Box::new(std::io::stdout()), headers);
    for rs in &v.repos {
        let status = if let Some(ref e) = rs.error {
            format_error(e)
        } else {
            format_repo_status(
                rs.ahead,
                rs.behind,
                rs.changed,
                rs.has_upstream,
                &rs.expected_branch,
            )
        };
        let mut row = vec![rs.shortname.clone(), rs.branch.clone(), status];
        if v.pr_enabled {
            row.push(match rs.pr {
                Some(ref pr) => format_pr_cell(pr),
                None => "-".into(),
            });
        }
        table.add_row(row)?;
    }
    if !v.root.is_empty() {
        let root_status = format!("{} untracked", v.root.len());
        let mut row = vec!["(workspace root)".into(), "-".into(), root_status];
        if v.pr_enabled {
            row.push("-".into());
        }
        table.add_row(row)?;
    }
    table.render()?;

    let has_file_detail = v.repos.iter().any(|r| !r.files.is_empty()) || !v.root.is_empty();
    let has_pr_detail = v.repos.iter().any(|r| r.pr.is_some());

    if v.verbose {
        for rs in &v.repos {
            let has_files = rs.error.is_none() && !rs.files.is_empty();
            let has_pr = rs.pr.is_some();
            if !has_files && !has_pr {
                continue;
            }
            println!("\n==> [{}]", rs.shortname);
            if let Some(ref pr) = rs.pr {
                println!(
                    "  PR #{}: {} ({})",
                    pr.number,
                    sanitize_for_terminal(&pr.title),
                    pr_state_label(pr),
                );
                println!("  {}", sanitize_for_terminal(&pr.url));
            }
            for f in &rs.files {
                println!("  {}", f);
            }
        }
        if !v.root.is_empty() {
            println!("\n==> [workspace root]");
            for item in &v.root {
                println!("  {}", item);
            }
        }
    } else if has_file_detail || has_pr_detail {
        println!("\nUse `wsp st -v` to see details.");
    }

    if !v.root.is_empty() {
        eprintln!("\nhint: suppress with wspignore (see `wsp help wspignore`)");
    }

    Ok(())
}

fn render_diff_text(v: DiffOutput) -> Result<()> {
    let mut first = true;
    for entry in &v.repos {
        if let Some(ref e) = entry.error {
            eprintln!("[{}] error: {}", entry.shortname, e);
            continue;
        }
        if entry.diff.is_empty() {
            continue;
        }
        if !first {
            println!();
        }
        println!("==> [{}]", entry.shortname);
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

fn render_sync_text(v: SyncOutput) -> Result<()> {
    if v.dry_run {
        println!(
            "Workspace: {}  Branch: {}  (dry run)\n",
            v.workspace, v.branch
        );
    } else {
        println!("Workspace: {}  Branch: {}\n", v.workspace, v.branch);
    }

    let mut table = Table::new(
        Box::new(std::io::stdout()),
        vec![
            "Repository".to_string(),
            "Action".to_string(),
            "Result".to_string(),
        ],
    );
    for r in &v.repos {
        let result = if let Some(ref e) = r.error {
            format!("ERROR — {}", e)
        } else {
            r.detail.clone().unwrap_or_default()
        };
        table.add_row(vec![r.shortname.clone(), r.action.clone(), result])?;
    }
    table.render()?;

    // Show actionable footer only for repos where a rebase/merge was attempted and conflicted
    let conflicted: Vec<&SyncRepoResult> = v
        .repos
        .iter()
        .filter(|r| !r.ok && r.error.as_deref() == Some("aborted, repo unchanged"))
        .collect();
    if !conflicted.is_empty() {
        eprintln!(
            "\n{} repo(s) had conflicts. To resolve manually:",
            conflicted.len()
        );
        for r in &conflicted {
            eprintln!("  cd {}", r.repo_dir.display());
            match r.strategy.as_str() {
                "merge" => eprintln!("  git merge {}", r.target),
                _ => eprintln!("  git rebase {}", r.target),
            }
        }
    }

    Ok(())
}

fn render_sync_abort_text(v: SyncAbortOutput) -> Result<()> {
    let mut table = Table::new(
        Box::new(std::io::stdout()),
        vec![
            "Repository".to_string(),
            "Action".to_string(),
            "Result".to_string(),
        ],
    );
    for r in &v.repos {
        let result = if let Some(ref e) = r.error {
            format!("ERROR — {}", e)
        } else {
            "ok".into()
        };
        table.add_row(vec![r.shortname.clone(), r.action.clone(), result])?;
    }
    table.render()?;
    Ok(())
}

fn render_config_list_text(v: ConfigListOutput) -> Result<()> {
    if v.entries.is_empty() {
        println!("No config values set.");
        return Ok(());
    }
    let has_source = v.entries.iter().any(|e| e.source.is_some());
    if has_source {
        let mut table = Table::new(
            Box::new(std::io::stdout()),
            vec!["Key".to_string(), "Value".to_string(), "Source".to_string()],
        );
        for e in &v.entries {
            let source = e
                .source
                .as_ref()
                .map(|s| format!("({})", s))
                .unwrap_or_default();
            let value = if e.experimental {
                format!("{} [experimental]", e.value)
            } else {
                e.value.clone()
            };
            table.add_row(vec![e.key.clone(), value, source])?;
        }
        table.render()
    } else {
        let mut table = Table::new(
            Box::new(std::io::stdout()),
            vec!["Key".to_string(), "Value".to_string()],
        );
        for e in &v.entries {
            let value = if e.experimental {
                format!("{} [experimental]", e.value)
            } else {
                e.value.clone()
            };
            table.add_row(vec![e.key.clone(), value])?;
        }
        table.render()
    }
}

fn render_config_get_text(v: ConfigGetOutput) -> Result<()> {
    match &v.value {
        Some(val) => println!("{}", val),
        None => println!("(not set)"),
    }
    Ok(())
}

fn render_mutation_text(v: MutationOutput) -> Result<()> {
    match v.duration_ms {
        Some(ms) => println!("{} ({:.1}s)", v.message, ms as f64 / 1000.0),
        None => println!("{}", v.message),
    }
    if let Some(hint) = &v.hint {
        println!("  {}", hint);
    }
    Ok(())
}

fn render_setup_commands_text(v: SetupCommandsOutput) -> Result<()> {
    if v.commands.is_empty() {
        println!("No setup commands for {}.", v.repo);
        return Ok(());
    }
    let max_label = v.commands.iter().map(|e| e.source.len()).max().unwrap_or(0);
    println!("Setup commands for {}:", v.repo);
    for entry in &v.commands {
        println!(
            "  [{:<width$}]  {}",
            entry.source,
            entry.command,
            width = max_label
        );
    }
    Ok(())
}

fn render_import_text(v: ImportOutput) -> Result<()> {
    if !v.registered.is_empty() {
        println!("Registered {} repo(s):", v.registered.len());
        for id in &v.registered {
            println!("  {}", id);
        }
    }
    if !v.skipped.is_empty() {
        println!("Skipped {} (already registered):", v.skipped.len());
        for id in &v.skipped {
            println!("  {}", id);
        }
    }
    if !v.failed.is_empty() {
        eprintln!("Failed {}:", v.failed.len());
        for f in &v.failed {
            eprintln!("  {}: {}", f.name, f.error);
        }
    }
    if v.registered.is_empty() && v.failed.is_empty() {
        println!("No new repos to register.");
    }
    Ok(())
}

fn render_path_text(v: PathOutput) -> Result<()> {
    println!("{}", v.path);
    Ok(())
}

/// Parse a timestamp this crate serialized. Absent or unparseable both mean
/// "no timestamp to show", which the callers render as blank or "never".
fn rfc3339(s: Option<&str>) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(s?)
        .ok()
        .map(|t| t.with_timezone(&chrono::Utc))
}

/// How long is left before a removed workspace expires: "in 5d", "soon", or
/// "never" when retention is disabled. Takes `gc::expires_at`'s output.
pub fn format_expiry(deadline: Option<chrono::DateTime<chrono::Utc>>) -> String {
    let Some(deadline) = deadline else {
        return "never".into();
    };
    let remaining = deadline - chrono::Utc::now();
    if remaining.num_seconds() <= 0 {
        "soon".into()
    } else if remaining.num_hours() >= 24 {
        // Rounded, not floored. A workspace removed seconds ago has 6d23h59m
        // left under the 7-day default, and flooring reports "in 6d" while
        // `wsp rm` has just printed an absolute date 7 days out -- the two
        // views of one deadline contradict each other on the commonest case.
        // Sub-day remainders fall to the branches below, so rounding up can
        // never claim a day for something with hours left.
        format!(
            "in {}d",
            (remaining.num_hours() as f64 / 24.0).round() as i64
        )
    } else if remaining.num_hours() > 0 {
        format!("in {}h", remaining.num_hours())
    } else {
        format!("in {}m", remaining.num_minutes())
    }
}

fn render_log_text(v: LogOutput) -> Result<()> {
    if v.oneline {
        render_log_oneline(&v.repos)
    } else {
        render_log_grouped(&v.repos)
    }
}

fn render_log_grouped(repos: &[RepoLogEntry]) -> Result<()> {
    let now = chrono::Utc::now().timestamp();
    let mut first = true;
    for entry in repos {
        if !first {
            println!();
        }
        println!("==> [{}]", entry.shortname);

        if let Some(ref e) = entry.error {
            eprintln!("  error: {}", e);
            first = false;
            continue;
        }

        if let Some(ref raw) = entry.raw {
            if raw.is_empty() {
                println!("  (no output)");
            } else {
                println!("{}", raw);
            }
            first = false;
            continue;
        }

        if entry.commits.is_empty() {
            println!("  (no commits on workspace branch)");
        } else {
            for c in &entry.commits {
                println!(
                    "  {}  {}  ({})",
                    &c.hash[..7.min(c.hash.len())],
                    c.subject,
                    format_relative_time(c.timestamp, now)
                );
            }
        }
        first = false;
    }
    Ok(())
}

fn render_log_oneline(repos: &[RepoLogEntry]) -> Result<()> {
    let now = chrono::Utc::now().timestamp();
    let mut all: Vec<(&str, &LogCommit)> = Vec::new();
    for entry in repos {
        if entry.error.is_some() {
            eprintln!(
                "[{}] error: {}",
                entry.shortname,
                entry.error.as_deref().unwrap_or("")
            );
            continue;
        }
        if let Some(ref raw) = entry.raw {
            if !raw.is_empty() {
                println!("==> [{}]", entry.shortname);
                println!("{}", raw);
            }
            continue;
        }
        for c in &entry.commits {
            all.push((&entry.shortname, c));
        }
    }

    all.sort_by(|a, b| b.1.timestamp.cmp(&a.1.timestamp).then_with(|| a.0.cmp(b.0)));

    if all.is_empty() {
        return Ok(());
    }

    let mut tw = TabWriter::new(std::io::stdout()).minwidth(0).padding(2);
    for (repo, c) in &all {
        writeln!(
            tw,
            "{}\t{}\t{}\t{}",
            repo,
            &c.hash[..7.min(c.hash.len())],
            c.subject,
            format_relative_time(c.timestamp, now)
        )?;
    }
    tw.flush()?;
    Ok(())
}

pub fn format_relative_time(ts: i64, now: i64) -> String {
    let delta = now.saturating_sub(ts);
    if delta < 0 {
        return "in the future".to_string();
    }
    let delta = delta as u64;
    match delta {
        0..=59 => format!("{}s ago", delta),
        60..=3599 => format!("{}m ago", delta / 60),
        3600..=86399 => format!("{}h ago", delta / 3600),
        86400..=604799 => format!("{}d ago", delta / 86400),
        _ => format!("{}w ago", delta / 604800),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Utc};
    use std::path::PathBuf;

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

    /// (name, headers, rows, expected)
    type TableCase<'a> = (&'a str, Vec<&'a str>, Vec<Vec<&'a str>>, &'a str);

    /// (name, ahead, behind, modified, has_upstream, expected_branch, want)
    type RepoStatusCase<'a> = (&'a str, u32, u32, u32, bool, &'a Option<String>, &'a str);

    #[test]
    fn test_table() {
        let cases: Vec<TableCase<'_>> = vec![
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
        let none: Option<String> = None;
        let cases: Vec<RepoStatusCase<'_>> = vec![
            ("clean", 0, 0, 0, true, &none, "clean"),
            ("clean no upstream", 0, 0, 0, false, &none, "clean"),
            ("modified only", 0, 0, 5, true, &none, "5 modified"),
            ("ahead with upstream", 3, 0, 0, true, &none, "3 ahead"),
            (
                "ahead no upstream",
                3,
                0,
                0,
                false,
                &none,
                "3 ahead (no upstream)",
            ),
            ("behind only", 0, 5, 0, true, &none, "5 behind"),
            (
                "ahead and behind",
                2,
                3,
                0,
                true,
                &none,
                "2 ahead, 3 behind",
            ),
            (
                "all three",
                2,
                3,
                4,
                true,
                &none,
                "2 ahead, 3 behind, 4 modified",
            ),
            (
                "both with upstream",
                2,
                0,
                4,
                true,
                &none,
                "2 ahead, 4 modified",
            ),
            (
                "both no upstream",
                2,
                0,
                4,
                false,
                &none,
                "2 ahead (no upstream), 4 modified",
            ),
            ("one each", 1, 0, 1, true, &none, "1 ahead, 1 modified"),
        ];
        for (name, ahead, behind, modified, has_upstream, expected_branch, want) in cases {
            assert_eq!(
                format_repo_status(ahead, behind, modified, has_upstream, expected_branch),
                want,
                "{}",
                name
            );
        }
    }

    #[test]
    fn test_format_pr_cell() {
        let cases: Vec<(&str, PrInfo, &str)> = vec![
            (
                "open",
                PrInfo {
                    number: 42,
                    url: "https://github.com/acme/dash/pull/42".into(),
                    state: "OPEN".into(),
                    title: "add feature".into(),
                    is_draft: false,
                },
                "#42 open",
            ),
            (
                "draft",
                PrInfo {
                    number: 7,
                    url: "https://github.com/acme/dash/pull/7".into(),
                    state: "OPEN".into(),
                    title: "wip: draft thing".into(),
                    is_draft: true,
                },
                "#7 draft",
            ),
            (
                "merged",
                PrInfo {
                    number: 108,
                    url: "https://github.com/acme/dash/pull/108".into(),
                    state: "MERGED".into(),
                    title: "ship it".into(),
                    is_draft: false,
                },
                "#108 merged",
            ),
            (
                "closed",
                PrInfo {
                    number: 999,
                    url: "https://github.com/acme/dash/pull/999".into(),
                    state: "CLOSED".into(),
                    title: "nope".into(),
                    is_draft: false,
                },
                "#999 closed",
            ),
        ];
        for (name, pr, want) in cases {
            assert_eq!(format_pr_cell(&pr), want, "{}", name);
        }
    }

    #[test]
    fn test_sanitize_for_terminal() {
        let cases: Vec<(&str, &str, &str)> = vec![
            ("passthrough", "normal title", "normal title"),
            ("tabs preserved", "col1\tcol2", "col1\tcol2"),
            ("newlines preserved", "line1\nline2", "line1\nline2"),
            (
                "ansi escape stripped",
                "clean \x1b[2J\x1b[Hpwned",
                "clean \u{FFFD}[2J\u{FFFD}[Hpwned",
            ),
            (
                "carriage return stripped",
                "visible\rinvisible",
                "visible\u{FFFD}invisible",
            ),
            (
                "null byte stripped",
                "before\x00after",
                "before\u{FFFD}after",
            ),
        ];
        for (name, input, want) in cases {
            assert_eq!(sanitize_for_terminal(input), want, "{}", name);
        }
    }

    #[test]
    fn test_format_repo_status_expected_branch() {
        let wb = Some("jganoff/my-feature".to_string());
        assert_eq!(
            format_repo_status(0, 0, 0, true, &wb),
            "not on workspace branch (jganoff/my-feature)"
        );
        assert_eq!(
            format_repo_status(2, 0, 1, true, &wb),
            "not on workspace branch (jganoff/my-feature), 2 ahead, 1 modified"
        );
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
    fn test_json_workspace_list() {
        let output = WorkspaceListOutput {
            hint: None,
            state: ListState::Active,
            workspaces: vec![WorkspaceListEntry {
                name: "my-ws".into(),
                branch: "my-ws".into(),
                repo_count: 2,
                repos: vec!["github.com/acme/api".into()],
                path: "/home/user/dev/workspaces/my-ws".into(),
                removed_at: None,
                expires_at: None,
                description: Some("test workspace".into()),
                created: "2026-03-01T10:00:00+00:00".into(),
                last_used: None,
                created_from: None,
            }],
        };
        let val = serde_json::to_value(&output).unwrap();
        assert_eq!(val["state"], "active");
        assert_eq!(val["workspaces"][0]["name"], "my-ws");
        assert_eq!(val["workspaces"][0]["repo_count"], 2);
        assert_eq!(val["workspaces"][0]["description"], "test workspace");
        assert_eq!(val["workspaces"][0]["created"], "2026-03-01T10:00:00+00:00");
        assert!(val["workspaces"][0].get("last_used").is_none());
    }

    #[test]
    fn test_json_workspace_repo_list() {
        let cases: Vec<(&str, WorkspaceRepoListOutput, serde_json::Value)> = vec![
            (
                "two repos",
                WorkspaceRepoListOutput {
                    workspace: "ws".into(),
                    branch: "ws".into(),
                    workspace_dir: PathBuf::from("/tmp/ws"),
                    repos: vec![
                        WorkspaceRepoListEntry {
                            identity: "github.com/user/repo-a".into(),
                            shortname: "repo-a".into(),
                            dir_name: "repo-a".into(),
                        },
                        WorkspaceRepoListEntry {
                            identity: "github.com/user/repo-b".into(),
                            shortname: "repo-b".into(),
                            dir_name: "repo-b".into(),
                        },
                    ],
                },
                serde_json::json!({
                    "workspace": "ws",
                    "branch": "ws",
                    "workspace_dir": "/tmp/ws",
                    "repos": [
                        {
                            "identity": "github.com/user/repo-a",
                            "shortname": "repo-a",
                            "dir_name": "repo-a"
                        },
                        {
                            "identity": "github.com/user/repo-b",
                            "shortname": "repo-b",
                            "dir_name": "repo-b"
                        }
                    ]
                }),
            ),
            (
                "empty",
                WorkspaceRepoListOutput {
                    workspace: "ws".into(),
                    branch: "ws".into(),
                    workspace_dir: PathBuf::from("/tmp/ws"),
                    repos: vec![],
                },
                serde_json::json!({ "workspace": "ws", "branch": "ws", "workspace_dir": "/tmp/ws", "repos": [] }),
            ),
        ];
        for (name, output, want) in cases {
            let val = serde_json::to_value(&output).unwrap();
            assert_eq!(val, want, "{}", name);
        }
    }

    #[test]
    fn test_json_status() {
        let output = StatusOutput {
            workspace: "my-ws".into(),
            branch: "my-ws".into(),
            workspace_dir: PathBuf::from("/tmp/workspaces/my-ws"),
            description: None,
            created: "2026-01-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap(),
            repos: vec![
                RepoStatusEntry {
                    identity: "github.com/user/repo-a".into(),
                    shortname: "repo-a".into(),
                    path: "/tmp/workspaces/my-ws/repo-a".into(),
                    branch: "my-ws".into(),
                    ahead: 1,
                    behind: 3,
                    changed: 2,
                    has_upstream: true,
                    role: "active".into(),
                    files: vec![" M src/main.rs".into(), "?? new.txt".into()],
                    error: None,
                    expected_branch: None,
                    pr: None,
                },
                RepoStatusEntry {
                    identity: "github.com/user/repo-b".into(),
                    shortname: "repo-b".into(),
                    path: String::new(),
                    branch: String::new(),
                    ahead: 0,
                    behind: 0,
                    changed: 0,
                    has_upstream: false,
                    role: "active".into(),
                    files: vec![],
                    error: Some("parse error".into()),
                    expected_branch: None,
                    pr: None,
                },
            ],
            root: vec![],
            verbose: false,
            pr_enabled: false,
        };
        let val = serde_json::to_value(&output).unwrap();
        assert_eq!(val["workspace"], "my-ws");
        assert_eq!(val["workspace_dir"], "/tmp/workspaces/my-ws");
        assert_eq!(val["repos"][0]["ahead"], 1);
        assert_eq!(val["repos"][0]["behind"], 3);
        assert_eq!(val["repos"][0]["changed"], 2);
        assert_eq!(val["repos"][0]["has_upstream"], true);
        assert_eq!(val["repos"][0]["role"], "active");
        assert_eq!(val["repos"][0]["files"][0], " M src/main.rs");
        assert_eq!(val["repos"][0]["files"][1], "?? new.txt");
        assert!(val["repos"][0].get("error").is_none());
        assert!(val["repos"][0].get("expected_branch").is_none());
        // repo-b has empty files → omitted
        assert!(val["repos"][1].get("files").is_none());
        assert_eq!(val["repos"][1]["has_upstream"], false);
        assert_eq!(val["repos"][1]["role"], "active");
        assert_eq!(val["repos"][1]["error"], "parse error");
        // root is empty → omitted
        assert!(val.get("root").is_none());
    }

    #[test]
    fn test_json_status_with_root() {
        let output = StatusOutput {
            workspace: "my-ws".into(),
            branch: "my-ws".into(),
            workspace_dir: PathBuf::from("/tmp/workspaces/my-ws"),
            description: None,
            created: "2026-01-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap(),
            repos: vec![],
            root: vec!["?? notes.md".into(), "?? my-stuff/".into()],
            verbose: true,
            pr_enabled: false,
        };
        let val = serde_json::to_value(&output).unwrap();
        assert_eq!(val["root"][0], "?? notes.md");
        assert_eq!(val["root"][1], "?? my-stuff/");
        assert_eq!(val["root"].as_array().unwrap().len(), 2);
        // verbose and pr_enabled are #[serde(skip)] → not serialized
        assert!(val.get("verbose").is_none());
        assert!(val.get("pr_enabled").is_none());
    }

    #[test]
    fn test_json_diff() {
        let output = DiffOutput {
            workspace: "ws".into(),
            branch: "ws".into(),
            workspace_dir: PathBuf::from("/tmp/ws"),
            repos: vec![
                RepoDiffEntry {
                    identity: "github.com/user/repo-a".into(),
                    shortname: "repo-a".into(),
                    path: "/tmp/ws/repo-a".into(),
                    diff: "--- a/file\n+++ b/file".into(),
                    error: None,
                },
                RepoDiffEntry {
                    identity: "github.com/user/repo-b".into(),
                    shortname: "repo-b".into(),
                    path: String::new(),
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
        let output = MutationOutput::new("Registered repo");
        let val = serde_json::to_value(&output).unwrap();
        assert_eq!(val["ok"], true);
        assert_eq!(val["message"], "Registered repo");
        assert!(val.get("hint").is_none()); // omitted when None
    }

    #[test]
    fn test_json_mutation_with_hint() {
        let output = MutationOutput::new("experimental.shell-prompt = true")
            .with_hint("re-source your shell to activate");
        let val = serde_json::to_value(&output).unwrap();
        assert_eq!(val["ok"], true);
        assert_eq!(val["hint"], "re-source your shell to activate");
    }

    #[test]
    fn test_json_error() {
        let output = ErrorOutput {
            error: "something went wrong".into(),
        };
        let val = serde_json::to_value(&output).unwrap();
        assert_eq!(val["error"], "something went wrong");
    }

    #[test]
    fn test_json_log() {
        let cases: Vec<(&str, LogOutput, serde_json::Value)> = vec![
            (
                "structured commits",
                LogOutput {
                    workspace: "ws".into(),
                    branch: "ws".into(),
                    workspace_dir: PathBuf::from("/tmp/ws"),
                    oneline: false,
                    repos: vec![RepoLogEntry {
                        identity: "github.com/acme/api-gateway".into(),
                        shortname: "api-gateway".into(),
                        path: "/tmp/ws/api-gateway".into(),
                        commits: vec![LogCommit {
                            hash: "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2".into(),
                            authored_at: "2023-11-14T22:13:20+00:00".into(),
                            timestamp: 1700000000,
                            subject: "feat: add billing".into(),
                        }],
                        raw: None,
                        error: None,
                    }],
                },
                serde_json::json!({
                    "workspace": "ws",
                    "branch": "ws",
                    "workspace_dir": "/tmp/ws",
                    "repos": [{
                        "identity": "github.com/acme/api-gateway",
                        "shortname": "api-gateway",
                        "path": "/tmp/ws/api-gateway",
                        "commits": [{
                            "hash": "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2",
                            "authored_at": "2023-11-14T22:13:20+00:00",
                            "subject": "feat: add billing"
                        }]
                    }]
                }),
            ),
            (
                "raw passthrough",
                LogOutput {
                    workspace: "ws".into(),
                    branch: "ws".into(),
                    workspace_dir: PathBuf::from("/tmp/ws"),
                    oneline: false,
                    repos: vec![RepoLogEntry {
                        identity: "github.com/acme/api-gateway".into(),
                        shortname: "api-gateway".into(),
                        path: "/tmp/ws/api-gateway".into(),
                        commits: vec![],
                        raw: Some("a1b2c3d feat: add billing\n".into()),
                        error: None,
                    }],
                },
                serde_json::json!({
                    "workspace": "ws",
                    "branch": "ws",
                    "workspace_dir": "/tmp/ws",
                    "repos": [{
                        "identity": "github.com/acme/api-gateway",
                        "shortname": "api-gateway",
                        "path": "/tmp/ws/api-gateway",
                        "commits": [],
                        "raw": "a1b2c3d feat: add billing\n"
                    }]
                }),
            ),
            (
                "error entry",
                LogOutput {
                    workspace: "ws".into(),
                    branch: "ws".into(),
                    workspace_dir: PathBuf::from("/tmp/ws"),
                    oneline: false,
                    repos: vec![RepoLogEntry {
                        identity: "github.com/acme/broken".into(),
                        shortname: "broken".into(),
                        path: String::new(),
                        commits: vec![],
                        raw: None,
                        error: Some("repo not found".into()),
                    }],
                },
                serde_json::json!({
                    "workspace": "ws",
                    "branch": "ws",
                    "workspace_dir": "/tmp/ws",
                    "repos": [{
                        "identity": "github.com/acme/broken",
                        "shortname": "broken",
                        "path": "",
                        "commits": [],
                        "error": "repo not found"
                    }]
                }),
            ),
            (
                "empty repos",
                LogOutput {
                    workspace: "ws".into(),
                    branch: "ws".into(),
                    workspace_dir: PathBuf::from("/tmp/ws"),
                    oneline: true,
                    repos: vec![],
                },
                serde_json::json!({ "workspace": "ws", "branch": "ws", "workspace_dir": "/tmp/ws", "repos": [] }),
            ),
        ];
        for (name, output, want) in cases {
            let val = serde_json::to_value(&output).unwrap();
            assert_eq!(val, want, "{}", name);
        }
    }

    #[test]
    fn test_json_sync() {
        let cases: Vec<(&str, SyncOutput, serde_json::Value)> = vec![
            (
                "basic sync",
                SyncOutput {
                    workspace: "my-ws".into(),
                    branch: "my-ws".into(),
                    dry_run: false,
                    repos: vec![SyncRepoResult {
                        identity: "github.com/acme/api-gateway".into(),
                        shortname: "api-gateway".into(),
                        path: "/tmp/ws/api-gateway".into(),
                        action: "rebase onto origin/main".into(),
                        ok: true,
                        detail: Some("2 commit(s) rebased".into()),
                        error: None,
                        repo_dir: PathBuf::from("/tmp/ws/api-gateway"),
                        target: "origin/main".into(),
                        strategy: "rebase".into(),
                    }],
                },
                serde_json::json!({
                    "workspace": "my-ws",
                    "branch": "my-ws",
                    "dry_run": false,
                    "repos": [{
                        "identity": "github.com/acme/api-gateway",
                        "shortname": "api-gateway",
                        "path": "/tmp/ws/api-gateway",
                        "action": "rebase onto origin/main",
                        "ok": true,
                        "detail": "2 commit(s) rebased"
                    }]
                }),
            ),
            (
                "dry run",
                SyncOutput {
                    workspace: "my-ws".into(),
                    branch: "my-ws".into(),
                    dry_run: true,
                    repos: vec![SyncRepoResult {
                        identity: "github.com/acme/api-gateway".into(),
                        shortname: "api-gateway".into(),
                        path: "/tmp/ws/api-gateway".into(),
                        action: "rebase onto origin/main".into(),
                        ok: true,
                        detail: Some("1 behind, 2 ahead".into()),
                        error: None,
                        repo_dir: PathBuf::from("/tmp/ws/api-gateway"),
                        target: "origin/main".into(),
                        strategy: "rebase".into(),
                    }],
                },
                serde_json::json!({
                    "workspace": "my-ws",
                    "branch": "my-ws",
                    "dry_run": true,
                    "repos": [{
                        "identity": "github.com/acme/api-gateway",
                        "shortname": "api-gateway",
                        "path": "/tmp/ws/api-gateway",
                        "action": "rebase onto origin/main",
                        "ok": true,
                        "detail": "1 behind, 2 ahead"
                    }]
                }),
            ),
            (
                "error entry",
                SyncOutput {
                    workspace: "my-ws".into(),
                    branch: "my-ws".into(),
                    dry_run: false,
                    repos: vec![SyncRepoResult {
                        identity: "github.com/acme/shared-lib".into(),
                        shortname: "shared-lib".into(),
                        path: "/tmp/ws/shared-lib".into(),
                        action: "rebase onto origin/main".into(),
                        ok: false,
                        detail: None,
                        error: Some("aborted, repo unchanged".into()),
                        repo_dir: PathBuf::from("/tmp/ws/shared-lib"),
                        target: "origin/main".into(),
                        strategy: "rebase".into(),
                    }],
                },
                serde_json::json!({
                    "workspace": "my-ws",
                    "branch": "my-ws",
                    "dry_run": false,
                    "repos": [{
                        "identity": "github.com/acme/shared-lib",
                        "shortname": "shared-lib",
                        "path": "/tmp/ws/shared-lib",
                        "action": "rebase onto origin/main",
                        "ok": false,
                        "error": "aborted, repo unchanged"
                    }]
                }),
            ),
        ];
        for (name, output, want) in cases {
            let val = serde_json::to_value(&output).unwrap();
            assert_eq!(val, want, "{}", name);
        }
    }

    #[test]
    fn test_json_exec() {
        let cases: Vec<(&str, ExecOutput, serde_json::Value)> = vec![
            (
                "success with captured output",
                ExecOutput {
                    workspace: "ws".into(),
                    repos: vec![ExecRepoResult {
                        identity: "github.com/acme/api-gateway".into(),
                        shortname: "api-gateway".into(),
                        path: "/tmp/ws/api-gateway".into(),
                        directory: "api-gateway".into(),
                        exit_code: 0,
                        signal: None,
                        ok: true,
                        stdout: Some("hello\n".into()),
                        stderr: Some(String::new()),
                        error: None,
                    }],
                },
                serde_json::json!({
                    "workspace": "ws",
                    "repos": [{
                        "identity": "github.com/acme/api-gateway",
                        "shortname": "api-gateway",
                        "path": "/tmp/ws/api-gateway",
                        "directory": "api-gateway",
                        "exit_code": 0,
                        "ok": true,
                        "stdout": "hello\n",
                        "stderr": ""
                    }]
                }),
            ),
            (
                "failure without capture",
                ExecOutput {
                    workspace: "ws".into(),
                    repos: vec![ExecRepoResult {
                        identity: "github.com/acme/api-gateway".into(),
                        shortname: "api-gateway".into(),
                        path: "/tmp/ws/api-gateway".into(),
                        directory: "api-gateway".into(),
                        exit_code: 1,
                        signal: None,
                        ok: false,
                        stdout: None,
                        stderr: None,
                        error: None,
                    }],
                },
                serde_json::json!({
                    "workspace": "ws",
                    "repos": [{
                        "identity": "github.com/acme/api-gateway",
                        "shortname": "api-gateway",
                        "path": "/tmp/ws/api-gateway",
                        "directory": "api-gateway",
                        "exit_code": 1,
                        "ok": false
                    }]
                }),
            ),
            (
                "spawn error",
                ExecOutput {
                    workspace: "ws".into(),
                    repos: vec![ExecRepoResult {
                        identity: "github.com/acme/api-gateway".into(),
                        shortname: "api-gateway".into(),
                        path: String::new(),
                        directory: String::new(),
                        exit_code: -1,
                        signal: None,
                        ok: false,
                        stdout: None,
                        stderr: None,
                        error: Some("No such file or directory".into()),
                    }],
                },
                serde_json::json!({
                    "workspace": "ws",
                    "repos": [{
                        "identity": "github.com/acme/api-gateway",
                        "shortname": "api-gateway",
                        "path": "",
                        "directory": "",
                        "exit_code": -1,
                        "ok": false,
                        "error": "No such file or directory"
                    }]
                }),
            ),
        ];
        for (name, output, want) in cases {
            let val = serde_json::to_value(&output).unwrap();
            assert_eq!(val, want, "{}", name);
        }
    }

    #[test]
    fn test_exit_code_exec() {
        let cases: Vec<(&str, ExecOutput, i32)> = vec![
            (
                "all ok",
                ExecOutput {
                    workspace: "ws".into(),
                    repos: vec![ExecRepoResult {
                        identity: "r".into(),
                        shortname: "r".into(),
                        path: String::new(),
                        directory: "r".into(),
                        exit_code: 0,
                        signal: None,
                        ok: true,
                        stdout: None,
                        stderr: None,
                        error: None,
                    }],
                },
                0,
            ),
            (
                "one failure",
                ExecOutput {
                    workspace: "ws".into(),
                    repos: vec![
                        ExecRepoResult {
                            identity: "a".into(),
                            shortname: "a".into(),
                            path: String::new(),
                            directory: "a".into(),
                            exit_code: 0,
                            signal: None,
                            ok: true,
                            stdout: None,
                            stderr: None,
                            error: None,
                        },
                        ExecRepoResult {
                            identity: "b".into(),
                            shortname: "b".into(),
                            path: String::new(),
                            directory: "b".into(),
                            exit_code: 1,
                            signal: None,
                            ok: false,
                            stdout: None,
                            stderr: None,
                            error: None,
                        },
                    ],
                },
                1,
            ),
            (
                "empty repos",
                ExecOutput {
                    workspace: "ws".into(),
                    repos: vec![],
                },
                0,
            ),
        ];
        for (name, output, want) in cases {
            assert_eq!(exit_code(&Output::Exec(output)), want, "{}", name);
        }
    }

    #[test]
    fn test_exit_code_sync_abort() {
        let cases: Vec<(&str, SyncAbortOutput, i32)> = vec![
            (
                "all ok",
                SyncAbortOutput {
                    workspace: "ws".into(),
                    repos: vec![SyncAbortRepoResult {
                        identity: "r".into(),
                        shortname: "r".into(),
                        path: String::new(),
                        action: "skip".into(),
                        ok: true,
                        error: None,
                    }],
                },
                0,
            ),
            (
                "one failure",
                SyncAbortOutput {
                    workspace: "ws".into(),
                    repos: vec![
                        SyncAbortRepoResult {
                            identity: "a".into(),
                            shortname: "a".into(),
                            path: String::new(),
                            action: "rebase aborted".into(),
                            ok: true,
                            error: None,
                        },
                        SyncAbortRepoResult {
                            identity: "b".into(),
                            shortname: "b".into(),
                            path: String::new(),
                            action: "rebase aborted".into(),
                            ok: false,
                            error: Some("abort failed".into()),
                        },
                    ],
                },
                1,
            ),
            (
                "empty repos",
                SyncAbortOutput {
                    workspace: "ws".into(),
                    repos: vec![],
                },
                0,
            ),
        ];
        for (name, output, want) in cases {
            assert_eq!(exit_code(&Output::SyncAbort(output)), want, "{}", name);
        }
    }

    #[test]
    fn test_format_relative_time() {
        let now = 1700000000i64;
        let cases = vec![
            ("just now", now, "0s ago"),
            ("30 seconds", now - 30, "30s ago"),
            ("1 minute", now - 60, "1m ago"),
            ("5 minutes", now - 300, "5m ago"),
            ("1 hour", now - 3600, "1h ago"),
            ("3 hours", now - 10800, "3h ago"),
            ("1 day", now - 86400, "1d ago"),
            ("6 days", now - 518400, "6d ago"),
            ("1 week", now - 604800, "1w ago"),
            ("3 weeks", now - 1814400, "3w ago"),
            ("future", now + 100, "in the future"),
        ];
        for (name, ts, want) in cases {
            assert_eq!(format_relative_time(ts, now), want, "{}", name);
        }
    }

    /// A freshly removed workspace must report the full retention window, not
    /// one day less, so it agrees with the absolute date `wsp rm` prints.
    #[test]
    fn test_format_expiry_agrees_with_the_removal_hint() {
        let trashed_at = chrono::Utc::now();
        for days in [2, 7, 30, 90] {
            let deadline = wsp_core::gc::expires_at(&trashed_at, days).unwrap();
            assert_eq!(
                format_expiry(Some(deadline)),
                format!("in {}d", days),
                "retention of {} day(s) must not lose a day to rounding",
                days
            );
        }
        // A one-day window is reported in hours, which is more use than "in 1d"
        // when there are 23 of them left.
        let deadline = wsp_core::gc::expires_at(&trashed_at, 1).unwrap();
        assert_eq!(format_expiry(Some(deadline)), "in 23h");
    }

    #[test]
    fn test_format_expiry() {
        let now = chrono::Utc::now();
        assert_eq!(format_expiry(None), "never");
        assert_eq!(
            format_expiry(Some(now - chrono::Duration::hours(1))),
            "soon"
        );
        assert_eq!(
            format_expiry(Some(
                now + chrono::Duration::days(5) + chrono::Duration::minutes(1)
            )),
            "in 5d"
        );
        assert_eq!(
            format_expiry(Some(
                now + chrono::Duration::hours(3) + chrono::Duration::minutes(1)
            )),
            "in 3h"
        );
        assert_eq!(
            format_expiry(Some(
                now + chrono::Duration::minutes(30) + chrono::Duration::seconds(1)
            )),
            "in 30m"
        );
    }
}
