//! Draft a release note from the ```whatsnew blocks in merged commits.
//!
//! Every PR carries a block in its description, and squash-merging puts that
//! description verbatim into the commit body, so the notes are already in
//! `git log`. No network, no bot, no fragment files.
//!
//! The output is raw material. Ordering by impact, folding several symptoms of
//! one cause into one line, and deciding what deserves prose are judgements
//! that need every note in view at once, so they stay with whoever writes the
//! release.

use anyhow::{Context, Result, bail};

/// Opens a note block. Everything up to the next fence belongs to it.
const OPEN: &str = "```whatsnew";

/// What a PR with nothing user-facing to announce writes, so that CI can tell
/// "nothing to say" from "nobody wrote a note".
const NOTHING: &str = "NONE";

/// Print the draft for `since..HEAD`, or the most recent tag if `since` is None.
pub fn run(since: Option<&str>) -> Result<()> {
    let since = match since {
        Some(rev) => rev.to_string(),
        None => most_recent_tag()?,
    };

    let bodies = commit_bodies(&format!("{since}..HEAD"))?;
    eprintln!("# collected from {} commit(s) since {since}", bodies.len());

    let notes = draft(&bodies);
    if notes.is_empty() {
        eprintln!("no user-facing notes since {since}");
        return Ok(());
    }
    print!("{notes}");
    Ok(())
}

/// Every note in `bodies`, in the order given, with `NONE` markers dropped.
///
/// Separate from the git plumbing so it can be tested against literals rather
/// than a repository.
fn draft(bodies: &[String]) -> String {
    let mut out = String::new();
    for body in bodies {
        for block in blocks_in(body) {
            if block.trim() == NOTHING {
                continue;
            }
            out.push_str(block.trim_end());
            out.push('\n');
        }
    }
    out
}

/// The contents of each ```whatsnew block in one commit body.
///
/// Scoped to a single body, so an unterminated block ends with it and cannot
/// reach the next commit. A blank line inside a block is content like any other.
fn blocks_in(body: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut current: Option<String> = None;

    for line in body.lines() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        match &mut current {
            // Any fence closes the block: a note that opens a nested one has
            // bigger problems than this loop.
            Some(block) if line.trim_start().starts_with("```") => {
                blocks.push(std::mem::take(block));
                current = None;
            }
            Some(block) => {
                block.push_str(line);
                block.push('\n');
            }
            None if line.trim() == OPEN => current = Some(String::new()),
            None => {}
        }
    }
    if let Some(block) = current {
        blocks.push(block);
    }
    blocks
}

/// Commit bodies for a revision range, newest first.
fn commit_bodies(range: &str) -> Result<Vec<String>> {
    // NUL-separated, and split here rather than asking a text tool to interpret
    // the separator. A commit body can contain anything except NUL.
    let out = std::process::Command::new("git")
        .args(["log", range, "--format=%B%x00"])
        .output()
        .context("running git log")?;
    if !out.status.success() {
        bail!(
            "git log {range} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .split('\0')
        .map(str::to_string)
        .filter(|body| !body.trim().is_empty())
        .collect())
}

fn most_recent_tag() -> Result<String> {
    let out = std::process::Command::new("git")
        .args(["describe", "--tags", "--abbrev=0"])
        .output()
        .context("running git describe")?;
    if !out.status.success() {
        bail!("no tag found; pass one explicitly, e.g. `just whatsnew-draft v0.19.0`");
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(lines: &[&str]) -> String {
        lines.join("\n")
    }

    #[test]
    fn a_blank_line_does_not_end_a_block() {
        let b = body(&[
            "subject",
            "",
            "```whatsnew",
            "### Fixes",
            "",
            "- first bullet",
            "- second bullet",
            "```",
        ]);
        assert_eq!(
            blocks_in(&b),
            vec!["### Fixes\n\n- first bullet\n- second bullet\n"]
        );
    }

    #[test]
    fn only_whatsnew_fences_open_a_block() {
        let b = body(&[
            "```",
            "some output the author pasted",
            "```",
            "prose",
            "```whatsnew",
            "the note",
            "```",
            "```sh",
            "a trailing example",
            "```",
        ]);
        assert_eq!(blocks_in(&b), vec!["the note\n"]);
    }

    #[test]
    fn none_markers_are_dropped() {
        let nothing = body(&["```whatsnew", "NONE", "```"]);
        let real = body(&["```whatsnew", "- a real note", "```"]);
        assert_eq!(draft(std::slice::from_ref(&nothing)), "");
        assert_eq!(draft(&[real, nothing]), "- a real note\n");
    }

    #[test]
    fn an_unterminated_block_cannot_reach_the_next_commit() {
        let unterminated = body(&["```whatsnew", "- note from a truncated body"]);
        let next = body(&["subject", "", "```whatsnew", "- the next note", "```"]);
        assert_eq!(
            draft(&[unterminated, next]),
            "- note from a truncated body\n- the next note\n"
        );
    }

    #[test]
    fn a_body_with_no_block_contributes_nothing() {
        assert_eq!(draft(&[body(&["just a subject", "", "and a body"])]), "");
    }

    #[test]
    fn carriage_returns_are_stripped() {
        let b = "```whatsnew\r\n- a note\r\n```\r\n";
        assert_eq!(blocks_in(b), vec!["- a note\n"]);
    }

    /// `blocks_in` returns a `Vec` because a body can hold more than one, and
    /// keeping only the first would drop a note silently.
    #[test]
    fn two_blocks_in_one_body_both_survive() {
        let b = body(&[
            "subject",
            "",
            "```whatsnew",
            "- from the first block",
            "```",
            "prose in between",
            "```whatsnew",
            "- from the second block",
            "```",
        ]);
        assert_eq!(
            blocks_in(&b),
            vec!["- from the first block\n", "- from the second block\n"]
        );
    }

    /// The caller decides what ships, so nothing may be reordered or deduped.
    #[test]
    fn every_block_survives_in_order() {
        let first = body(&["```whatsnew", "- one", "```"]);
        let second = body(&["```whatsnew", "- two", "```"]);
        assert_eq!(draft(&[first, second]), "- one\n- two\n");
    }
}
