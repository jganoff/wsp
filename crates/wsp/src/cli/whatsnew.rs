use anyhow::Result;
use clap::{ArgMatches, Command};

use wsp_core::config::Paths;
use wsp_core::output::Output;

// Embedded at compile time so the command works for installed binaries.
static CHANGELOG: &str = include_str!("../../../../CHANGELOG.md");

pub fn cmd() -> Command {
    Command::new("whatsnew")
        .about("Show what changed in this version of wsp [read-only]")
        .long_about(
            "Show what changed in this version of wsp.\n\n\
             Displays the changelog section for the currently installed version. \
             A one-time hint is shown after the first command following an upgrade; \
             this command lets you read the full details at any time.",
        )
}

pub fn run(_matches: &ArgMatches, _paths: &Paths) -> Result<Output> {
    let version = env!("CARGO_PKG_VERSION");
    let section = extract_version_section(CHANGELOG, version);
    if section.is_empty() {
        println!(
            "No changelog entry found for v{}.\n\
             See https://github.com/jganoff/wsp/releases for release notes.",
            version
        );
    } else {
        println!("## What's new in wsp v{}\n\n{}", version, section.trim());
    }
    Ok(Output::None)
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

    const SAMPLE: &str = "\
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

    #[test]
    fn extracts_current_version_section() {
        let section = extract_version_section(SAMPLE, "1.2.0");
        assert!(section.contains("Add foo"), "should contain feature");
        assert!(section.contains("Fix bar"), "should contain bug fix");
        assert!(
            !section.contains("Add baz"),
            "should not bleed into next version"
        );
    }

    #[test]
    fn extracts_older_version_section() {
        let section = extract_version_section(SAMPLE, "1.1.0");
        assert!(section.contains("Add baz"));
        assert!(!section.contains("Add foo"));
    }

    #[test]
    fn returns_empty_for_unknown_version() {
        let section = extract_version_section(SAMPLE, "9.9.9");
        assert!(section.is_empty());
    }

    #[test]
    fn does_not_include_header_line_in_section() {
        let section = extract_version_section(SAMPLE, "1.2.0");
        assert!(
            !section.contains("2026-01-01"),
            "section body should not include the header line"
        );
    }
}
