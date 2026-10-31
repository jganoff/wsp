//! Generate only the release being prepared. Finalized history is editorial
//! content and must survive byte-for-byte, even when old RC tags still exist.

use std::{fs, path::Path, process::Command};

use anyhow::{Context, Result, bail, ensure};
use semver::Version;

pub fn run(args: &[String]) -> Result<()> {
    ensure!(
        args.len() == 1,
        "usage: cargo xtask release-changelog <version>"
    );
    let version = Version::parse(&args[0]).context("invalid release version")?;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    prepare(&root, &version)
}

/// Keep the preamble and all sections older than the release being prepared.
/// An RC prep keeps earlier RC entries; a final prep consolidates its RCs.
fn retained<'a>(old: &'a str, version: &Version) -> Result<(&'a str, &'a str, Version)> {
    let mut offset = 0;
    let mut first = None;
    for line in old.split_inclusive('\n') {
        if let Some(heading) = line.strip_prefix("## [") {
            first.get_or_insert(offset);
            let name = heading
                .split_once(']')
                .context("invalid changelog heading")?
                .0;
            if name != "Unreleased" {
                let previous = Version::parse(name).context("invalid changelog version")?;
                ensure!(
                    previous <= *version,
                    "changelog contains newer version {previous}"
                );
                let same_line = (previous.major, previous.minor, previous.patch)
                    == (version.major, version.minor, version.patch);
                if previous < *version && !(version.pre.is_empty() && same_line) {
                    return Ok((&old[..first.unwrap()], &old[offset..], previous));
                }
            }
        }
        offset += line.len();
    }
    bail!("no previous release section found; refusing to replace changelog history")
}

fn prepare(root: &Path, version: &Version) -> Result<()> {
    let path = root.join("CHANGELOG.md");
    let old = fs::read_to_string(&path).context("reading CHANGELOG.md")?;
    let (preamble, history, previous) = retained(&old, version)?;
    let tag = format!("v{previous}");
    let ancestor = Command::new("git")
        .current_dir(root)
        .args(["merge-base", "--is-ancestor", &tag, "HEAD"])
        .output()
        .context("checking previous release tag")?;
    ensure!(
        ancestor.status.success(),
        "previous release tag {tag} is missing or not an ancestor of HEAD"
    );

    // Disable historical tag boundaries: RC tags inside the selected range
    // belong to the one section generated for this release.
    let output = Command::new("git")
        .current_dir(root)
        .args([
            "cliff",
            "--config",
            "cliff.toml",
            "--repository",
            ".",
            "--tag-pattern",
            "^$",
            "--strip",
            "all",
            "--tag",
            &format!("v{version}"),
            &format!("{tag}..HEAD"),
        ])
        .output()
        .context("running git-cliff (install git-cliff first)")?;
    ensure!(
        output.status.success(),
        "git-cliff failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let section = String::from_utf8(output.stdout).context("git-cliff output is not UTF-8")?;
    ensure!(
        section.starts_with(&format!("## [{version}]")),
        "git-cliff did not generate the expected release heading"
    );
    ensure!(
        section
            .lines()
            .filter(|line| line.starts_with("## ["))
            .count()
            == 1,
        "git-cliff generated more than one release section"
    );
    fs::write(
        path,
        format!("{preamble}{}\n\n{history}", section.trim_end()),
    )
    .context("writing CHANGELOG.md")
}

#[cfg(test)]
mod tests {
    use super::*;

    const OLD: &str = "# Changelog\n\n## [Unreleased]\n\npending\n\n## [1.2.0-rc.2] - date\n\nrc two\n\n## [1.2.0-rc.1] - date\n\nrc one\n\n## [1.1.0] - date\n\nHand-edited final notes.\n";

    #[test]
    fn final_consolidates_current_rcs_and_preserves_finalized_bytes() {
        let (header, history, base) = retained(OLD, &Version::parse("1.2.0").unwrap()).unwrap();
        assert_eq!(header, "# Changelog\n\n");
        assert_eq!(history, "## [1.1.0] - date\n\nHand-edited final notes.\n");
        assert_eq!(base.to_string(), "1.1.0");
    }

    #[test]
    fn prerelease_keeps_previous_rcs() {
        let (_, history, base) = retained(OLD, &Version::parse("1.2.0-rc.3").unwrap()).unwrap();
        assert!(history.starts_with("## [1.2.0-rc.2]"));
        assert!(history.ends_with("Hand-edited final notes.\n"));
        assert_eq!(base.to_string(), "1.2.0-rc.2");
    }

    #[test]
    fn rejects_a_downgrade_or_missing_history() {
        assert!(retained(OLD, &Version::parse("1.0.0").unwrap()).is_err());
        assert!(retained("# Changelog\n", &Version::parse("1.2.0").unwrap()).is_err());
    }

    #[test]
    fn a_prerelease_without_its_own_section_is_based_on_the_last_final() {
        let old = "# Changelog\n\n## [Unreleased]\n\n## [1.1.0] - date\n\nnotes\n";
        let (_, history, base) = retained(old, &Version::parse("1.2.0-rc.1").unwrap()).unwrap();
        assert_eq!(history, "## [1.1.0] - date\n\nnotes\n");
        assert_eq!(base.to_string(), "1.1.0");
    }

    #[test]
    fn release_history_with_old_rc_tags_is_preserved() {
        if !Command::new("git")
            .args(["cliff", "--version"])
            .output()
            .is_ok_and(|out| out.status.success())
        {
            assert!(
                std::env::var_os("WSP_RELEASE_TEST_REQUIRE_CLIFF").is_none(),
                "install git-cliff to run release history coverage"
            );
            eprintln!("skipping git-cliff integration fixture: git-cliff not installed");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let git = |args: &[&str]| {
            let out = Command::new("git")
                .current_dir(root)
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        git(&["init"]);
        git(&["config", "user.email", "test@example.com"]);
        git(&["config", "user.name", "Test"]);
        git(&["config", "commit.gpgsign", "false"]);
        fs::write(root.join("cliff.toml"), include_str!("../../../cliff.toml")).unwrap();
        git(&["add", "."]);
        git(&["commit", "-m", "feat: old release"]);
        git(&["tag", "v1.1.0-rc.1"]);
        git(&["commit", "--allow-empty", "-m", "fix: old final"]);
        git(&["tag", "v1.1.0"]);
        git(&[
            "commit",
            "--allow-empty",
            "-m",
            "feat: first current change",
        ]);
        git(&["tag", "v1.2.0-rc.1"]);
        git(&[
            "commit",
            "--allow-empty",
            "-m",
            "fix: second current change",
        ]);
        git(&["tag", "v1.2.0-rc.2"]);
        git(&["commit", "--allow-empty", "-m", "fix: final current change"]);
        fs::write(root.join("CHANGELOG.md"), OLD).unwrap();
        prepare(root, &Version::parse("1.2.0").unwrap()).unwrap();
        let actual = fs::read_to_string(root.join("CHANGELOG.md")).unwrap();
        assert!(actual.starts_with("# Changelog\n\n## [1.2.0]"));
        assert!(actual.ends_with("## [1.1.0] - date\n\nHand-edited final notes.\n"));
        for change in [
            "First current change",
            "Second current change",
            "Final current change",
        ] {
            assert!(actual.contains(change), "missing {change}: {actual}");
        }
        assert!(!actual.contains("rc."));
        assert!(!actual.contains("Old final"));
        assert!(!actual.contains("Unreleased"));

        // A repeated preparation replaces its section instead of duplicating it.
        prepare(root, &Version::parse("1.2.0").unwrap()).unwrap();
        assert_eq!(
            fs::read_to_string(root.join("CHANGELOG.md")).unwrap(),
            actual
        );

        fs::write(root.join("CHANGELOG.md"), OLD).unwrap();
        prepare(root, &Version::parse("1.2.0-rc.3").unwrap()).unwrap();
        let rc = fs::read_to_string(root.join("CHANGELOG.md")).unwrap();
        assert!(rc.starts_with("# Changelog\n\n## [1.2.0-rc.3]"));
        assert!(rc.contains("Final current change"));
        assert!(!rc.contains("First current change"));
        assert!(rc.ends_with(OLD.split_once("## [1.2.0-rc.2]").unwrap().1));

        fs::write(root.join("CHANGELOG.md"), OLD).unwrap();
        git(&["tag", "-d", "v1.1.0"]);
        assert!(prepare(root, &Version::parse("1.2.0").unwrap()).is_err());
        assert_eq!(fs::read_to_string(root.join("CHANGELOG.md")).unwrap(), OLD);
    }
}
