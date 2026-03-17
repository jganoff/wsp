use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};

use crate::workspace::Metadata;

pub const MARKER_BEGIN: &str = "<!-- wsp:begin -->";
pub const MARKER_END: &str = "<!-- wsp:end -->";
const SKILL_CONTENT: &str = include_str!("../../../skills/wsp-manage/SKILL.md");
const REPORT_SKILL_CONTENT: &str = include_str!("../../../skills/wsp-report/SKILL.md");
const NEW_FEATURE_SKILL_CONTENT: &str = include_str!("../../../skills/wsp-new-feature/SKILL.md");

/// Generate or update AGENTS.md, CLAUDE.md symlink, and workspace skill.
pub fn update(ws_dir: &Path, metadata: &Metadata) -> Result<()> {
    let agents_path = ws_dir.join("AGENTS.md");
    let section = build_marked_section(metadata);

    let content = if agents_path.exists() {
        let existing = fs::read_to_string(&agents_path).context("reading existing AGENTS.md")?;
        replace_marked_section(&existing, &section)
    } else {
        build_initial_file(metadata, &section)
    };

    // Atomic write via tempfile + rename
    let mut tmp =
        tempfile::NamedTempFile::new_in(ws_dir).context("creating temp file for AGENTS.md")?;
    tmp.write_all(content.as_bytes())
        .context("writing AGENTS.md content")?;
    tmp.persist(&agents_path)
        .context("renaming temp file to AGENTS.md")?;

    ensure_symlink(ws_dir)?;
    install_skill(ws_dir)?;

    Ok(())
}

fn build_marked_section(metadata: &Metadata) -> String {
    let mut s = String::new();

    s.push_str(MARKER_BEGIN);
    s.push('\n');
    s.push_str("## Workspace Context\n\n");
    s.push_str(
        "**Multi-repo workspace.** Do not assume the workspace root is a git repo. \
         Each directory listed below is its own independent repository with its own \
         history, build system, and dependencies. When spawning sub-agents, pass the \
         full absolute path to the specific repo directory they need.\n\n",
    );
    s.push_str("| Property | Value |\n");
    s.push_str("|----------|-------|\n");
    s.push_str(&format!("| Workspace | {} |\n", metadata.name));
    s.push_str(&format!("| Branch | {} |\n", metadata.branch));
    s.push('\n');

    s.push_str("## Repositories\n\n");
    s.push_str("| Repo | Directory |\n");
    s.push_str("|------|-----------|\n");

    for identity in metadata.repos.keys() {
        let dir = metadata
            .dir_name(identity)
            .unwrap_or_else(|_| identity.clone());
        s.push_str(&format!("| {} | {} |\n", identity, dir));
    }

    s.push_str("\n## Workspace Boundary\n\n");
    s.push_str(
        "**The workspace root is managed by wsp. Do not create, modify, or delete any files \
         here.** Only edit files inside the repo directories listed above.\n\
         Do not touch other copies of these repos elsewhere on disk.\n",
    );

    s.push_str("\n## Per-Repo Conventions\n\n");
    s.push_str(
        "Each repo may have its own `CLAUDE.md` or `AGENTS.md` with build commands, \
         architecture notes, and file conventions. Always check those before creating \
         new files or making assumptions about project structure.\n",
    );

    s.push_str("\n## Quick Reference\n\n");
    s.push_str("```bash\n");
    s.push_str("wsp st                  # status across all repos\n");
    s.push_str("wsp diff                # diff across all repos\n");
    s.push_str("wsp repo add <repo>     # add repo to workspace\n");
    s.push_str("wsp repo rm <repo>      # remove repo from workspace\n");
    s.push_str("wsp exec <name> -- cmd  # run command in each repo\n");
    s.push_str("```\n");
    s.push_str("\n## New Features\n\n");
    s.push_str(
        "To start a new feature, use the **/wsp-new-feature** skill to create a wsp workspace.\n",
    );

    s.push_str("\n## Troubleshooting\n\n");
    s.push_str(
        "If you encounter a wsp issue, use the **/wsp-report** skill to gather diagnostics and file a GitHub issue.\n\n",
    );
    s.push_str(
        "Or report manually at <https://github.com/jganoff/wsp/issues> with the output of `wsp --version`, `uname -srm`, and `wsp st --json`.\n",
    );
    s.push_str("\n## Feedback Loop\n\n");
    s.push_str(
        "When you encounter friction with any tool in this workspace (wsp, build scripts, \
         CI, tasks), suggest filing an issue on the relevant repo. Don't silently work \
         around problems — surface them.\n",
    );
    s.push_str(MARKER_END);
    s.push('\n');

    s
}

/// Extract user-written content from AGENTS.md (everything outside the wsp markers).
/// Returns None if there's no meaningful user content.
pub fn extract_user_content(agents_md: &str) -> Option<String> {
    let begin = agents_md.find(MARKER_BEGIN);
    let end = agents_md.find(MARKER_END);

    let user_part = match (begin, end) {
        (Some(b), Some(e)) if b < e => {
            let before = agents_md[..b].trim();
            let after_end = e + MARKER_END.len();
            let after = agents_md[after_end..].trim();
            let mut parts = Vec::new();
            if !before.is_empty() {
                parts.push(before);
            }
            if !after.is_empty() {
                parts.push(after);
            }
            if parts.is_empty() {
                return None;
            }
            parts.join("\n\n")
        }
        _ => {
            // No markers — the whole file is user content
            let trimmed = agents_md.trim();
            if trimmed.is_empty() {
                return None;
            }
            trimmed.to_string()
        }
    };

    // Filter out the default placeholder comment
    let filtered = user_part
        .replace(
            "<!-- Add your project-specific notes for AI agents here -->",
            "",
        )
        .trim()
        .to_string();

    if filtered.is_empty() {
        None
    } else {
        Some(filtered)
    }
}

fn build_initial_file(metadata: &Metadata, section: &str) -> String {
    let mut s = String::new();
    s.push_str(&format!("# Workspace: {}\n\n", metadata.name));
    s.push_str("<!-- Add your project-specific notes for AI agents here -->\n\n");
    s.push_str(section);
    s
}

fn replace_marked_section(existing: &str, new_section: &str) -> String {
    let begin_idx = existing.find(MARKER_BEGIN);
    let end_idx = existing.find(MARKER_END);

    match (begin_idx, end_idx) {
        (Some(b), Some(e)) if b < e => {
            let end_of_marker = e + MARKER_END.len();
            let mut result = String::new();
            result.push_str(&existing[..b]);
            result.push_str(new_section);
            // Skip any trailing newline after the end marker
            let rest_start = if existing[end_of_marker..].starts_with('\n') {
                end_of_marker + 1
            } else {
                end_of_marker
            };
            if rest_start < existing.len() {
                result.push_str(&existing[rest_start..]);
            }
            result
        }
        _ => {
            // Missing, malformed, or inverted markers — append
            let mut result = existing.to_string();
            if !result.is_empty() {
                if !result.ends_with('\n') {
                    result.push('\n');
                }
                result.push('\n');
            }
            result.push_str(new_section);
            result
        }
    }
}

fn ensure_symlink(ws_dir: &Path) -> Result<()> {
    let link_path = ws_dir.join("CLAUDE.md");

    match fs::symlink_metadata(&link_path) {
        Ok(meta) => {
            if meta.file_type().is_symlink() {
                // Skip if already pointing to AGENTS.md
                if fs::read_link(&link_path).ok().as_deref() == Some(Path::new("AGENTS.md")) {
                    return Ok(());
                }
                fs::remove_file(&link_path).context("removing stale CLAUDE.md symlink")?;
                symlink_file("AGENTS.md", &link_path).context("creating CLAUDE.md symlink")?;
            }
            // Regular file — leave it alone
        }
        Err(_) => {
            // Path doesn't exist. (Broken symlinks are handled above since
            // symlink_metadata succeeds for broken symlinks and reports is_symlink=true.)
            symlink_file("AGENTS.md", &link_path).context("creating CLAUDE.md symlink")?;
        }
    }

    Ok(())
}

#[cfg(unix)]
fn symlink_file<P: AsRef<Path>, Q: AsRef<Path>>(original: P, link: Q) -> std::io::Result<()> {
    std::os::unix::fs::symlink(original, link)
}

#[cfg(windows)]
fn symlink_file<P: AsRef<Path>, Q: AsRef<Path>>(original: P, link: Q) -> std::io::Result<()> {
    std::os::windows::fs::symlink_file(original, link)
}

fn install_skill(ws_dir: &Path) -> Result<()> {
    let manage_dir = ws_dir.join(".claude/skills/wsp-manage");
    fs::create_dir_all(&manage_dir).context("creating wsp-manage skill directory")?;
    fs::write(manage_dir.join("SKILL.md"), SKILL_CONTENT).context("writing wsp-manage SKILL.md")?;

    let report_dir = ws_dir.join(".claude/skills/wsp-report");
    fs::create_dir_all(&report_dir).context("creating wsp-report skill directory")?;
    fs::write(report_dir.join("SKILL.md"), REPORT_SKILL_CONTENT)
        .context("writing wsp-report SKILL.md")?;

    let new_feature_dir = ws_dir.join(".claude/skills/wsp-new-feature");
    fs::create_dir_all(&new_feature_dir).context("creating wsp-new-feature skill directory")?;
    fs::write(new_feature_dir.join("SKILL.md"), NEW_FEATURE_SKILL_CONTENT)
        .context("writing wsp-new-feature SKILL.md")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::symlink_file;
    use super::*;
    use std::collections::BTreeMap;

    use chrono::Utc;

    use crate::workspace::{Metadata, WorkspaceRepoRef};

    fn make_metadata(name: &str, branch: &str, repos: &[(&str, Option<&str>)]) -> Metadata {
        let mut map = BTreeMap::new();
        for (id, r) in repos {
            match r {
                Some(ref_str) => {
                    map.insert(
                        id.to_string(),
                        Some(WorkspaceRepoRef {
                            r#ref: ref_str.to_string(),
                            url: None,
                        }),
                    );
                }
                None => {
                    map.insert(id.to_string(), None);
                }
            }
        }
        Metadata {
            version: 0,
            name: name.into(),
            branch: branch.into(),
            repos: map,
            created: Utc::now(),
            description: None,
            last_used: None,
            created_from: None,
            dirs: BTreeMap::new(),
            config: None,
        }
    }

    fn make_metadata_with_dirs(
        name: &str,
        branch: &str,
        repos: &[(&str, Option<&str>)],
        dirs: &[(&str, &str)],
    ) -> Metadata {
        let mut meta = make_metadata(name, branch, repos);
        for (k, v) in dirs {
            meta.dirs.insert(k.to_string(), v.to_string());
        }
        meta
    }

    // --- Marker parsing tests (pure string, no FS) ---

    #[test]
    fn test_replace_marked_section() {
        struct Case {
            name: &'static str,
            existing: &'static str,
            new_section: &'static str,
            want_contains: Vec<&'static str>,
            want_not_contains: Vec<&'static str>,
        }

        let cases = vec![
            Case {
                name: "both markers present — content replaced",
                existing: "# Title\n\n<!-- wsp:begin -->\nold content\n<!-- wsp:end -->\n",
                new_section: "<!-- wsp:begin -->\nnew content\n<!-- wsp:end -->\n",
                want_contains: vec!["# Title\n\n", "new content"],
                want_not_contains: vec!["old content"],
            },
            Case {
                name: "user content before and after preserved",
                existing: "# My Notes\n\nCustom text\n\n<!-- wsp:begin -->\nold\n<!-- wsp:end -->\n\n## Footer\n",
                new_section: "<!-- wsp:begin -->\nnew\n<!-- wsp:end -->\n",
                want_contains: vec!["# My Notes\n\nCustom text\n\n", "new", "## Footer"],
                want_not_contains: vec!["old"],
            },
            Case {
                name: "only begin marker — appends",
                existing: "# Title\n<!-- wsp:begin -->\npartial\n",
                new_section: "<!-- wsp:begin -->\nnew\n<!-- wsp:end -->\n",
                want_contains: vec!["# Title\n<!-- wsp:begin -->\npartial\n", "new", MARKER_END],
                want_not_contains: vec![],
            },
            Case {
                name: "only end marker — appends",
                existing: "# Title\n<!-- wsp:end -->\n",
                new_section: "<!-- wsp:begin -->\nnew\n<!-- wsp:end -->\n",
                want_contains: vec!["# Title\n<!-- wsp:end -->\n", "new"],
                want_not_contains: vec![],
            },
            Case {
                name: "no markers — appends",
                existing: "# Title\n\nSome content\n",
                new_section: "<!-- wsp:begin -->\nnew\n<!-- wsp:end -->\n",
                want_contains: vec!["# Title\n\nSome content\n", "new"],
                want_not_contains: vec![],
            },
            Case {
                name: "inverted markers — appends",
                existing: "<!-- wsp:end -->\nstuff\n<!-- wsp:begin -->\n",
                new_section: "<!-- wsp:begin -->\nnew\n<!-- wsp:end -->\n",
                want_contains: vec!["<!-- wsp:end -->\nstuff\n<!-- wsp:begin -->\n", "new"],
                want_not_contains: vec![],
            },
            Case {
                name: "empty string — appends",
                existing: "",
                new_section: "<!-- wsp:begin -->\nnew\n<!-- wsp:end -->\n",
                want_contains: vec!["new"],
                want_not_contains: vec![],
            },
        ];

        for tc in &cases {
            let result = replace_marked_section(tc.existing, tc.new_section);
            for want in &tc.want_contains {
                assert!(
                    result.contains(want),
                    "case {:?}: expected to contain {:?}, got {:?}",
                    tc.name,
                    want,
                    result
                );
            }
            for not_want in &tc.want_not_contains {
                assert!(
                    !result.contains(not_want),
                    "case {:?}: expected NOT to contain {:?}, got {:?}",
                    tc.name,
                    not_want,
                    result
                );
            }
        }
    }

    // --- Content generation tests ---

    #[test]
    fn test_build_marked_section_content() {
        struct Case {
            name: &'static str,
            meta: Metadata,
            want_contains: Vec<&'static str>,
        }

        let cases = vec![
            Case {
                name: "single repo",
                meta: make_metadata("feat", "feat", &[("github.com/acme/api-gateway", None)]),
                want_contains: vec!["| github.com/acme/api-gateway | api-gateway |"],
            },
            Case {
                name: "multiple repos",
                meta: make_metadata(
                    "feat",
                    "jg/feat",
                    &[
                        ("github.com/acme/api-gateway", None),
                        ("github.com/acme/proto", None),
                    ],
                ),
                want_contains: vec![
                    "| Workspace | feat |",
                    "| Branch | jg/feat |",
                    "| github.com/acme/api-gateway | api-gateway |",
                    "| github.com/acme/proto | proto |",
                ],
            },
            Case {
                name: "custom dir names",
                meta: make_metadata_with_dirs(
                    "feat",
                    "feat",
                    &[("github.com/acme/api-gateway", None)],
                    &[("github.com/acme/api-gateway", "custom-dir")],
                ),
                want_contains: vec!["| github.com/acme/api-gateway | custom-dir |"],
            },
            Case {
                name: "empty repos — valid table with header only",
                meta: make_metadata("empty", "empty", &[]),
                want_contains: vec![
                    "**Multi-repo workspace.**",
                    "Do not assume the workspace root is a git repo",
                    "When spawning sub-agents, pass the full absolute path",
                    "| Repo | Directory |",
                    "## Workspace Boundary",
                    "Do not create, modify, or delete any files here",
                    "Only edit files inside the repo directories",
                    "## Per-Repo Conventions",
                    "## Quick Reference",
                    "## New Features",
                    "/wsp-new-feature",
                    "## Troubleshooting",
                    "/wsp-report",
                    "https://github.com/jganoff/wsp/issues",
                    "## Feedback Loop",
                ],
            },
        ];

        for tc in &cases {
            let result = build_marked_section(&tc.meta);
            assert!(result.starts_with(MARKER_BEGIN), "case {:?}", tc.name);
            assert!(result.contains(MARKER_END), "case {:?}", tc.name);
            for want in &tc.want_contains {
                assert!(
                    result.contains(want),
                    "case {:?}: expected to contain {:?}, got:\n{}",
                    tc.name,
                    want,
                    result
                );
            }
        }
    }

    #[test]
    fn test_build_initial_file() {
        let meta = make_metadata("my-feat", "jg/my-feat", &[("github.com/acme/api", None)]);
        let section = build_marked_section(&meta);
        let result = build_initial_file(&meta, &section);

        assert!(result.starts_with("# Workspace: my-feat\n"));
        assert!(result.contains("<!-- Add your project-specific notes"));
        assert!(result.contains(MARKER_BEGIN));
        assert!(result.contains(MARKER_END));
        assert!(result.contains("| github.com/acme/api | api |"));
    }

    // --- Filesystem integration tests ---

    #[test]
    fn test_update_creates_agents_md_and_symlink_and_skill() {
        let tmp = tempfile::tempdir().unwrap();
        let ws_dir = tmp.path();
        let meta = make_metadata("test-ws", "test-ws", &[("github.com/acme/api", None)]);

        update(ws_dir, &meta).unwrap();

        // AGENTS.md exists with expected content
        let content = fs::read_to_string(ws_dir.join("AGENTS.md")).unwrap();
        assert!(content.contains("# Workspace: test-ws"));
        assert!(content.contains(MARKER_BEGIN));
        assert!(content.contains("| github.com/acme/api | api |"));

        // CLAUDE.md is a symlink to AGENTS.md
        let link_meta = fs::symlink_metadata(ws_dir.join("CLAUDE.md")).unwrap();
        assert!(link_meta.file_type().is_symlink());
        let target = fs::read_link(ws_dir.join("CLAUDE.md")).unwrap();
        assert_eq!(target.to_str().unwrap(), "AGENTS.md");

        // Skills installed
        let manage_skill = ws_dir.join(".claude/skills/wsp-manage/SKILL.md");
        assert!(manage_skill.exists());
        let skill = fs::read_to_string(&manage_skill).unwrap();
        assert!(skill.contains("wsp"));

        let report_skill = ws_dir.join(".claude/skills/wsp-report/SKILL.md");
        assert!(report_skill.exists());
        let skill = fs::read_to_string(&report_skill).unwrap();
        assert!(skill.contains("wsp-report"));

        let new_feature_skill = ws_dir.join(".claude/skills/wsp-new-feature/SKILL.md");
        assert!(new_feature_skill.exists());
        let skill = fs::read_to_string(&new_feature_skill).unwrap();
        assert!(skill.contains("wsp-new-feature"));
    }

    #[test]
    fn test_update_preserves_user_content() {
        let tmp = tempfile::tempdir().unwrap();
        let ws_dir = tmp.path();
        let meta = make_metadata("ws", "ws", &[("github.com/acme/api", None)]);

        // First creation
        update(ws_dir, &meta).unwrap();

        // Simulate user editing: add content before markers
        let agents_path = ws_dir.join("AGENTS.md");
        let original = fs::read_to_string(&agents_path).unwrap();
        let modified = original.replace(
            "<!-- Add your project-specific notes for AI agents here -->",
            "## My Custom Notes\n\nThis is my important context.",
        );
        fs::write(&agents_path, &modified).unwrap();

        // Add a repo (simulated by updating metadata) and re-run
        let meta2 = make_metadata(
            "ws",
            "ws",
            &[("github.com/acme/api", None), ("github.com/acme/web", None)],
        );
        update(ws_dir, &meta2).unwrap();

        let result = fs::read_to_string(&agents_path).unwrap();
        assert!(result.contains("## My Custom Notes"));
        assert!(result.contains("This is my important context."));
        assert!(result.contains("| github.com/acme/web | web |"));
    }

    #[test]
    fn test_update_appends_when_markers_removed() {
        let tmp = tempfile::tempdir().unwrap();
        let ws_dir = tmp.path();
        let meta = make_metadata("ws", "ws", &[("github.com/acme/api", None)]);

        // Create initial file
        update(ws_dir, &meta).unwrap();

        // User removes markers entirely
        let agents_path = ws_dir.join("AGENTS.md");
        fs::write(&agents_path, "# My Custom File\n\nNo markers here.\n").unwrap();

        // Re-run update
        update(ws_dir, &meta).unwrap();

        let result = fs::read_to_string(&agents_path).unwrap();
        assert!(result.starts_with("# My Custom File\n\nNo markers here.\n"));
        assert!(result.contains(MARKER_BEGIN));
        assert!(result.contains(MARKER_END));
    }

    #[test]
    fn test_broken_symlink_recreated() {
        let tmp = tempfile::tempdir().unwrap();
        let ws_dir = tmp.path();

        // Create a broken symlink
        symlink_file("nonexistent-target", ws_dir.join("CLAUDE.md")).unwrap();

        let meta = make_metadata("ws", "ws", &[]);
        update(ws_dir, &meta).unwrap();

        // Symlink should now point to AGENTS.md
        let link_meta = fs::symlink_metadata(ws_dir.join("CLAUDE.md")).unwrap();
        assert!(link_meta.file_type().is_symlink());
        let target = fs::read_link(ws_dir.join("CLAUDE.md")).unwrap();
        assert_eq!(target.to_str().unwrap(), "AGENTS.md");
    }

    #[test]
    fn test_regular_claude_md_left_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let ws_dir = tmp.path();

        // Create a regular CLAUDE.md file
        fs::write(ws_dir.join("CLAUDE.md"), "# My custom CLAUDE.md\n").unwrap();

        let meta = make_metadata("ws", "ws", &[]);
        update(ws_dir, &meta).unwrap();

        // Should still be a regular file with original content
        let link_meta = fs::symlink_metadata(ws_dir.join("CLAUDE.md")).unwrap();
        assert!(!link_meta.file_type().is_symlink());
        let content = fs::read_to_string(ws_dir.join("CLAUDE.md")).unwrap();
        assert_eq!(content, "# My custom CLAUDE.md\n");
    }

    #[test]
    fn test_empty_repos_generates_valid_table() {
        let tmp = tempfile::tempdir().unwrap();
        let ws_dir = tmp.path();
        let meta = make_metadata("empty-ws", "empty-ws", &[]);

        update(ws_dir, &meta).unwrap();

        let content = fs::read_to_string(ws_dir.join("AGENTS.md")).unwrap();
        assert!(content.contains("| Repo | Directory |"));
        assert!(content.contains(MARKER_BEGIN));
        assert!(content.contains(MARKER_END));
    }
}
