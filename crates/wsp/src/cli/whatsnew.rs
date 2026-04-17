use std::io::IsTerminal;

use anyhow::Result;
use clap::{Arg, ArgAction, ArgMatches, Command};
use owo_colors::OwoColorize;

use wsp_core::config::Paths;
use wsp_core::output::Output;

// Embedded at compile time so the command works for installed binaries.
// WHATSNEW.md has user-facing prose; CHANGELOG.md is the raw commit log fallback.
static WHATSNEW: &str = include_str!("../../../../WHATSNEW.md");
static CHANGELOG: &str = include_str!("../../../../CHANGELOG.md");

pub fn cmd() -> Command {
    Command::new("whatsnew")
        .about("Show what changed in this version of wsp [read-only]")
        .long_about(
            "Show what changed in this version of wsp.\n\n\
             Displays the changelog section for the currently installed version. \
             A one-time hint is shown after the first command following an upgrade; \
             this command lets you read the full details at any time. \
             Pass --all to show every version.",
        )
        .arg(
            Arg::new("all")
                .long("all")
                .short('a')
                .action(ArgAction::SetTrue)
                .help("Show every version, newest first"),
        )
}

pub fn run(matches: &ArgMatches, _paths: &Paths) -> Result<Output> {
    let show_all = matches.get_flag("all");

    let md = if show_all {
        render_all(WHATSNEW, CHANGELOG)
    } else {
        let version = env!("CARGO_PKG_VERSION");
        let section = extract_version_section(WHATSNEW, version);
        let section = if section.trim().is_empty() {
            extract_version_section(CHANGELOG, version)
        } else {
            section
        };
        if section.trim().is_empty() {
            println!(
                "No changelog entry found for v{}.\n\
                 See https://github.com/jganoff/wsp/releases for release notes.",
                version
            );
            return Ok(Output::None);
        }
        format!("## What's new in wsp v{}\n\n{}", version, section.trim())
    };

    if std::io::stdout().is_terminal() {
        print_styled(&md);
    } else {
        println!("{}", md);
    }
    Ok(Output::None)
}

/// Builds a markdown document with every version section concatenated,
/// newest first. Prefers prose from WHATSNEW.md; falls back to the raw
/// CHANGELOG.md section for versions that predate prose notes.
fn render_all(whatsnew: &str, changelog: &str) -> String {
    let mut out = String::new();
    for version in list_versions(changelog) {
        let section = extract_version_section(whatsnew, &version);
        let section = if section.trim().is_empty() {
            extract_version_section(changelog, &version)
        } else {
            section
        };
        if section.trim().is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!("## v{}\n\n{}\n", version, section.trim()));
    }
    out
}

/// Extracts version strings from `## [X.Y.Z] - ...` headers, in source order.
/// Skips non-release markers like `## [Unreleased]`.
fn list_versions(changelog: &str) -> Vec<String> {
    changelog
        .lines()
        .filter_map(|line| {
            let rest = line.strip_prefix("## [")?;
            let end = rest.find(']')?;
            let version = &rest[..end];
            version
                .starts_with(|c: char| c.is_ascii_digit())
                .then(|| version.to_string())
        })
        .collect()
}

/// Render markdown with minimal ANSI styling for terminal display.
/// Handles the subset of markdown used in WHATSNEW.md: headings, fenced
/// code blocks, bullet lists, and inline backtick code.
fn print_styled(md: &str) {
    let color = std::io::stdout().is_terminal();
    let mut in_code_block = false;
    for line in md.lines() {
        if line.starts_with("```") {
            in_code_block = !in_code_block;
            continue;
        }
        if in_code_block {
            if color {
                println!("{}", line.dimmed());
            } else {
                println!("{}", line);
            }
            continue;
        }
        if let Some(heading) = line.strip_prefix("## ") {
            let heading = render_inline_code(heading, false);
            if color {
                println!("{}", heading.bold().underline());
            } else {
                println!("{}", heading);
            }
        } else if let Some(heading) = line.strip_prefix("### ") {
            let heading = render_inline_code(heading, false);
            if color {
                println!("{}", heading.bold());
            } else {
                println!("{}", heading);
            }
        } else {
            println!("{}", render_inline_code(line, color));
        }
    }
}

/// Render inline backtick code spans. When `color` is true, code spans
/// get ANSI bold styling. When false, backtick markers are stripped.
fn render_inline_code(line: &str, color: bool) -> String {
    let mut result = String::with_capacity(line.len());
    let mut in_code = false;
    let mut code_buf = String::new();
    for ch in line.chars() {
        if ch == '`' {
            if in_code {
                if color {
                    result.push_str(&format!("{}", code_buf.bold()));
                } else {
                    result.push_str(&code_buf);
                }
                code_buf.clear();
            }
            in_code = !in_code;
        } else if in_code {
            code_buf.push(ch);
        } else {
            result.push(ch);
        }
    }
    // Unclosed backtick: emit as-is
    if !code_buf.is_empty() {
        result.push('`');
        result.push_str(&code_buf);
    }
    result
}

/// Extracts the changelog body for the given version.
/// Returns an empty string if the version is not found.
pub(crate) fn extract_version_section<'a>(changelog: &'a str, version: &str) -> &'a str {
    let header = format!("## [{}]", version);
    let Some(start_offset) = changelog.find(header.as_str()) else {
        return "";
    };
    // Skip past the header line (including any date suffix like " - 2026-03-30")
    let after_header = &changelog[start_offset + header.len()..];
    let body_start = after_header.find('\n').map(|i| i + 1).unwrap_or(0);
    let body = &after_header[body_start..];
    // Stop at the next `## [` section header
    let end = body.find("\n## [").unwrap_or(body.len());
    &body[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_CHANGELOG: &str = "\
# Changelog

## [1.2.0] - 2026-01-01

### Features

- Add foo

### Bug Fixes

- Fix bar

## [1.1.0] - 2025-12-01

### Features

- Add baz
";

    const SAMPLE_WHATSNEW: &str = "\
# What's New

## [1.2.0] - 2026-01-01

v1.2.0 brings foo and fixes bar. Try it out.

## [1.0.0] - 2025-06-01

The initial release of wsp.
";

    #[test]
    fn extracts_current_version_section() {
        let section = extract_version_section(SAMPLE_CHANGELOG, "1.2.0");
        assert!(section.contains("Add foo"), "should contain feature");
        assert!(section.contains("Fix bar"), "should contain bug fix");
        assert!(
            !section.contains("Add baz"),
            "should not bleed into next version"
        );
    }

    #[test]
    fn extracts_older_version_section() {
        let section = extract_version_section(SAMPLE_CHANGELOG, "1.1.0");
        assert!(section.contains("Add baz"));
        assert!(!section.contains("Add foo"));
    }

    #[test]
    fn returns_empty_for_unknown_version() {
        let section = extract_version_section(SAMPLE_CHANGELOG, "9.9.9");
        assert!(section.is_empty());
    }

    #[test]
    fn does_not_include_header_line_in_section() {
        let section = extract_version_section(SAMPLE_CHANGELOG, "1.2.0");
        assert!(
            !section.contains("2026-01-01"),
            "section body should not include the header line"
        );
    }

    #[test]
    fn whatsnew_hit_returns_prose() {
        let section = extract_version_section(SAMPLE_WHATSNEW, "1.2.0");
        assert!(
            section.contains("brings foo and fixes bar"),
            "should return prose from WHATSNEW"
        );
    }

    #[test]
    fn whatsnew_miss_falls_back_to_changelog() {
        // v1.1.0 exists in CHANGELOG but not in WHATSNEW
        let whatsnew_section = extract_version_section(SAMPLE_WHATSNEW, "1.1.0");
        assert!(
            whatsnew_section.trim().is_empty(),
            "should not find v1.1.0 in WHATSNEW"
        );
        let changelog_section = extract_version_section(SAMPLE_CHANGELOG, "1.1.0");
        assert!(
            changelog_section.contains("Add baz"),
            "should fall back to CHANGELOG"
        );
    }

    #[test]
    fn whatsnew_preferred_over_changelog_for_same_version() {
        // v1.2.0 exists in both; WHATSNEW should be preferred
        let whatsnew_section = extract_version_section(SAMPLE_WHATSNEW, "1.2.0");
        let changelog_section = extract_version_section(SAMPLE_CHANGELOG, "1.2.0");
        assert!(
            !whatsnew_section.trim().is_empty(),
            "WHATSNEW should have an entry"
        );
        assert!(
            !changelog_section.trim().is_empty(),
            "CHANGELOG should also have an entry"
        );
        // Prose content differs from commit bullets
        assert!(whatsnew_section.contains("brings foo"));
        assert!(changelog_section.contains("Add foo"));
    }

    #[test]
    fn both_miss_returns_empty() {
        let whatsnew_section = extract_version_section(SAMPLE_WHATSNEW, "9.9.9");
        let changelog_section = extract_version_section(SAMPLE_CHANGELOG, "9.9.9");
        assert!(whatsnew_section.is_empty());
        assert!(changelog_section.is_empty());
    }

    #[test]
    fn embedded_whatsnew_compiles() {
        // Validates that the include_str! paths resolve at compile time
        assert!(!WHATSNEW.is_empty(), "WHATSNEW.md should be embedded");
        assert!(!CHANGELOG.is_empty(), "CHANGELOG.md should be embedded");
    }

    #[test]
    fn list_versions_extracts_in_source_order() {
        let versions = list_versions(SAMPLE_CHANGELOG);
        assert_eq!(versions, vec!["1.2.0", "1.1.0"]);
    }

    #[test]
    fn list_versions_empty_for_unreleased_only() {
        let versions = list_versions("# Changelog\n\n## [Unreleased]\n\n- change\n");
        assert!(versions.is_empty());
    }

    #[test]
    fn render_all_prefers_whatsnew_prose_falls_back_to_changelog() {
        let out = render_all(SAMPLE_WHATSNEW, SAMPLE_CHANGELOG);
        // v1.2.0 is in both; should use prose from WHATSNEW
        assert!(
            out.contains("brings foo and fixes bar"),
            "should use WHATSNEW prose for v1.2.0"
        );
        // v1.1.0 is only in CHANGELOG; should fall back
        assert!(
            out.contains("Add baz"),
            "should fall back to CHANGELOG for v1.1.0"
        );
        // Per-version headings should be present
        assert!(out.contains("## v1.2.0"));
        assert!(out.contains("## v1.1.0"));
    }

    #[test]
    fn render_all_newest_first() {
        let out = render_all(SAMPLE_WHATSNEW, SAMPLE_CHANGELOG);
        let v120 = out.find("## v1.2.0").expect("v1.2.0 heading");
        let v110 = out.find("## v1.1.0").expect("v1.1.0 heading");
        assert!(v120 < v110, "newest version should appear first");
    }

    #[test]
    fn render_all_skips_versions_missing_from_both() {
        // A version mentioned only in WHATSNEW (not in CHANGELOG) should be
        // skipped, since list_versions walks CHANGELOG (the authoritative list).
        let changelog = "# Changelog\n\n## [1.2.0] - 2026-01-01\n\n- fix\n";
        let whatsnew = "# What's New\n\n## [9.9.9] - 2027-01-01\n\nghost\n\n## [1.2.0] - 2026-01-01\n\nprose\n";
        let out = render_all(whatsnew, changelog);
        assert!(out.contains("## v1.2.0"));
        assert!(!out.contains("9.9.9"), "orphan WHATSNEW version is skipped");
    }
}
