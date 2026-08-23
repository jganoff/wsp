//! What the shell wrapper must do around a command, declared on the command.
//!
//! This is attached to the clap `Command` itself via clap's typed-extension
//! mechanism — the same one `clap_complete` uses for `ArgValueCandidates`, and
//! which this crate already uses for arg completers. So a command's shell
//! behavior lives next to its definition rather than in a table somewhere else.
//!
//! Read it back with `cmd.get::<ShellNav>()`. The declaration is *required* —
//! `test_every_command_declares_shell_nav` fails without it — because absence
//! would be indistinguishable from nobody having considered the question, which
//! is how `wsp rename` shipped for five releases moving the workspace directory
//! with no wrapper case at all.

/// The four primitives the dialects actually differ over. Every wrapper case is
/// some combination of these, which is why the generators can render from the
/// flags rather than hand-writing a body per command per dialect.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ShellNav {
    /// Leave the workspaces tree before the binary runs.
    ///
    /// Required whenever the command may relocate or remove the directory the
    /// shell is standing in: Windows cannot rename or delete a live process's
    /// cwd, and only the shell can move itself, so this can never be delegated
    /// to the binary.
    pub vacate: bool,
    /// On success, cd to the path the binary wrote to `WSP_CD_FILE`.
    pub follow_destination: bool,
    /// Afterwards, prefer the starting directory if it still exists.
    ///
    /// Named "prefer" because it **takes precedence over `follow_destination`**,
    /// and that precedence is load-bearing rather than incidental. `rename` sets
    /// both: if the old directory survives we were never inside the renamed
    /// workspace, so we must go back. Rendering follow-first instead would
    /// teleport someone who ran `wsp rename w new` from `$HOME` into a workspace
    /// they were never in — `$HOME` survives *and* a destination was reported.
    ///
    /// Pinned by `test_rename_prefers_previous_over_destination`.
    pub prefer_previous: bool,
    /// The destination arrives on stdout instead — `wsp cd`'s existing contract.
    pub destination_on_stdout: bool,
    /// The command *can* strand the shell, but the wrapper does not handle it
    /// yet.
    ///
    /// This is the third state the old `KNOWN_GAPS` list carried, and it has to
    /// exist: without it a known-broken command has to be declared `none()`,
    /// whose contract says the command cannot relocate the shell. That would put
    /// a false safety claim in the machine-readable declaration and leave a code
    /// comment as the only record — exactly the failure mode this type exists to
    /// remove.
    pub unhandled_gap: bool,
}

impl clap::builder::CommandExt for ShellNav {}

impl ShellNav {
    /// The command cannot relocate or remove the directory the shell is in, so
    /// the wrapper needs no case for it.
    ///
    /// Declared explicitly rather than left absent. Absence would be
    /// indistinguishable from "nobody thought about it", which is how every bug
    /// in this area happened; requiring a declaration turns that into a test
    /// failure and removes the need for any hand-maintained exemption list.
    pub const fn none() -> Self {
        Self {
            vacate: false,
            follow_destination: false,
            prefer_previous: false,
            destination_on_stdout: false,
            unhandled_gap: false,
        }
    }

    /// `new`: the binary reports where it landed.
    pub const fn follows() -> Self {
        Self {
            vacate: false,
            follow_destination: true,
            prefer_previous: false,
            destination_on_stdout: false,
            unhandled_gap: false,
        }
    }

    /// `rm`: step aside, remove, come back only if it survived.
    pub const fn vacates() -> Self {
        Self {
            vacate: true,
            follow_destination: false,
            prefer_previous: true,
            destination_on_stdout: false,
            unhandled_gap: false,
        }
    }

    /// `rename`: step aside so the move can happen, then follow it — unless we
    /// were never inside, in which case come back.
    pub const fn vacates_and_follows() -> Self {
        Self {
            vacate: true,
            follow_destination: true,
            prefer_previous: true,
            destination_on_stdout: false,
            unhandled_gap: false,
        }
    }

    /// Can strand the shell; wrapper support not written yet. Requires an
    /// issue reference at the call site.
    pub const fn unhandled_gap() -> Self {
        Self {
            vacate: false,
            follow_destination: false,
            prefer_previous: false,
            destination_on_stdout: false,
            unhandled_gap: true,
        }
    }

    /// `cd`: destination on stdout.
    pub const fn prints_path() -> Self {
        Self {
            vacate: false,
            follow_destination: false,
            prefer_previous: false,
            destination_on_stdout: true,
            unhandled_gap: false,
        }
    }

    /// Does this command need the shell to move at all?
    ///
    /// Currently read only by tests. In the full design the generators render
    /// wrapper bodies from these flags, at which point this becomes production
    /// code and the allow can go.
    #[allow(dead_code)]
    pub const fn moves_shell(&self) -> bool {
        self.vacate || self.follow_destination || self.destination_on_stdout
    }

    /// Declared as able to strand the shell with no wrapper support.
    #[allow(dead_code)]
    pub const fn is_unhandled_gap(&self) -> bool {
        self.unhandled_gap
    }
}
