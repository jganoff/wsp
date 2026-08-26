//! Repo automation for wsp: things done *to* the repository rather than by the
//! product. Never shipped, so it stays out of `crates/wsp`, which is a
//! workspace manager and nothing else.
//!
//! Reach it through the `just` recipes rather than remembering the invocation.

use anyhow::{Result, bail};

mod release_notes;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (task, rest) = match args.split_first() {
        Some((task, rest)) => (task.as_str(), rest),
        None => {
            usage();
            bail!("no task given");
        }
    };

    match task {
        // An empty argument is what a `just` recipe with an unset default
        // passes through, and it means "no revision given".
        "release-notes" => {
            release_notes::run(rest.first().map(String::as_str).filter(|r| !r.is_empty()))
        }
        "-h" | "--help" | "help" => {
            usage();
            Ok(())
        }
        other => {
            usage();
            bail!("unknown task: {other}");
        }
    }
}

fn usage() {
    eprintln!("usage: cargo xtask <task>\n");
    eprintln!("tasks:");
    eprintln!("  release-notes [<rev>]   draft a release note from whatsnew blocks since <rev>");
}
