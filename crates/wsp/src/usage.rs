//! The usage line for a command whose clap signature does not match its real
//! grammar.
//!
//! `Command::override_usage` fixes `--help`, but clap keeps the value private,
//! so the SKILL.md generator cannot read it back and derives a signature from
//! the args instead. That produced `wsp recover [<workspace>]` in the file
//! AGENTS.md calls the source of truth for agents, while `--help` said
//! `<workspace>` — the exact ambiguity `recover` was rewritten to remove.
//!
//! [`UsageExt::usage`] sets both from one string, so they cannot drift.

use clap::Command;

/// A command's real usage line, readable back off the `Command`.
#[derive(Clone, Copy, Debug)]
pub struct Usage(
    // Read by the SKILL.md generator, which is behind the `codegen` feature,
    // and by the tests below. Neither counts for dead-code analysis in a plain
    // release build, where the value exists only to be carried.
    #[allow(dead_code)] pub &'static str,
);

impl clap::builder::CommandExt for Usage {}

pub trait UsageExt {
    /// Set the usage line for `--help` and for generated docs.
    ///
    /// Reach for this when clap's derived signature is misleading: a positional
    /// that is optional in clap but required in practice (`recover`), or one
    /// whose order differs from its declaration (`rename`).
    fn usage(self, line: &'static str) -> Self;
}

impl UsageExt for Command {
    fn usage(self, line: &'static str) -> Self {
        self.override_usage(line).add(Usage(line))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point: what `--help` shows is what a generator can read.
    #[test]
    fn usage_is_readable_back_off_the_command() {
        let cmd = Command::new("recover").usage("wsp recover <workspace>");
        assert_eq!(
            cmd.get::<Usage>().map(|u| u.0),
            Some("wsp recover <workspace>")
        );
    }

    /// Every `override_usage` should go through `usage()`, or the generator
    /// silently falls back to the derived signature it was meant to replace.
    #[test]
    fn every_command_setting_a_usage_line_uses_the_extension() {
        fn walk(cmd: &Command, path: String, found: &mut Vec<String>) {
            if cmd.get::<Usage>().is_some() {
                found.push(path.clone());
            }
            for sub in cmd.get_subcommands() {
                walk(sub, format!("{path} {}", sub.get_name()), found);
            }
        }
        let cli = crate::cli::build_cli();
        let mut found = Vec::new();
        for sub in cli.get_subcommands() {
            walk(sub, sub.get_name().to_string(), &mut found);
        }
        found.sort();

        // Grepping the source is the only way to catch a bare
        // `.override_usage()`: clap does not expose the value.
        let mut expected: Vec<String> =
            std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/src/cli"))
                .unwrap()
                .filter_map(|e| {
                    let path = e.ok()?.path();
                    let src = std::fs::read_to_string(&path).ok()?;
                    (src.contains(".usage(") || src.contains(".override_usage("))
                        .then(|| path.file_stem()?.to_str().map(String::from))?
                })
                .collect();
        expected.sort();

        let found_names: Vec<String> = found
            .iter()
            .map(|p| p.split(' ').next().unwrap().to_string())
            .collect();
        for module in &expected {
            assert!(
                found_names.iter().any(|n| n == module),
                "{module}.rs sets a usage line but it is not readable back; \
                 use `.usage(..)` from this module, not `.override_usage(..)`.\n  \
                 readable: {found_names:?}"
            );
        }
    }
}
