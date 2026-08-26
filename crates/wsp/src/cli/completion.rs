//! Generates the shell integration: tab completions plus a `wsp` wrapper
//! function for zsh, bash, fish, and PowerShell.
//!
//! # Why a wrapper exists at all
//!
//! A child process cannot change its parent's working directory. That, and
//! only that, is why `wsp cd` needs a shell function rather than being a
//! command like any other. Everything in this file exists to serve that one
//! limitation.
//!
//! # The wrapper knows nothing
//!
//! The wrapper carries no knowledge in either direction. It does not parse
//! argv, hardcode which commands or aliases exist, or infer what a command
//! meant going in; it does not interpret the binary's output coming back.
//! Anything it needs, the binary tells it out-of-band.
//!
//! This is not a style preference. Every bug this file has produced came from
//! breaking it:
//!
//! - The wrapper listed `new` but not its alias `create`, so `wsp create` ran
//!   the command and never cd'd. Knowing command names is knowledge.
//! - posix read the workspace name as `$1` and PowerShell scanned for the
//!   first non-flag token. Both are wrong: `new` has five value-taking flags,
//!   so `wsp new -w src new-ws` cd'd into `src`. Parsing argv is knowledge.
//! - `rm` vacated the workspace directory before invoking the binary, which
//!   silently removed the cwd that its optional-positional fallback reads, so
//!   the documented bare form stopped working. Changing the binary's inputs is
//!   knowledge about what those inputs mean.
//! - `cd` captured stdout and treated it as a path, so `wsp cd <ws> --json`
//!   tried to change directory into a JSON document. Interpreting output is
//!   knowledge.
//!
//! # Channels
//!
//! | Variable | Direction | Meaning |
//! |----------|-----------|---------|
//! | `WSP_SHELL` | wrapper → binary | the wrapper is active |
//! | `WSP_CD_FILE` | wrapper → binary | scratch file for the binary to report a destination |
//! | `WSP_PWD` | wrapper → binary | where the user actually stood, when the wrapper had to vacate first |
//!
//! `cd` is the exception that proves the rule: its entire output *is* the
//! destination, so it reads stdout rather than a file — and pays for that with
//! a `-d` test, because with `--json` the output is a document instead. It gets
//! that exception because it is the hot path and a temp file per invocation is
//! not worth the symmetry.
//!
//! # Four dialects, one rule each
//!
//! Every case is written out per dialect. A fix applied to one body does not
//! reach the others, and that has bitten more than once — most recently a
//! `cd -` fix that landed in fish's `rm` case and not its `rename` case, caught
//! only by CI. `crate::shellnav::ShellNav` exists to make these render from a
//! single declaration on each command; until that lands, changing one case
//! means checking all four.
//!
//! Behavior is covered end-to-end in `tests/shell_cd.rs`, which spawns real
//! shells. String assertions on the generated text are not enough — several
//! passed for years while the behavior they described was broken.

use std::io::Write;

use anyhow::{Result, bail};
use clap::{Arg, ArgMatches, Command};

use wsp_core::config::{Config, Paths};
use wsp_core::output::Output;

/// Tmux integration mode for shell hooks.
///
/// When enabled, the shell hook applies a three-layer guard before renaming:
/// 1. **tmux available** — `$TMUX` set and `tmux` on PATH
/// 2. **active pane** — only the focused pane renames (prevents multi-pane fights)
/// 3. **title ownership** — skip if user explicitly set the title (`automatic-rename`
///    off without our `@wsp-title` marker); only restore `automatic-rename` when
///    leaving a workspace if wsp was the one who set the title
#[derive(Debug, Clone, Copy, Default, PartialEq)]
enum TmuxMode {
    #[default]
    Off,
    /// Sets the tmux window name to `wsp:<workspace>` via `tmux rename-window`.
    WindowTitle,
}

/// Shell hook options baked in at generation time from config.
#[derive(Debug, Clone, Copy, Default)]
struct ShellHookOpts {
    tmux: TmuxMode,
    prompt: bool,
}

impl ShellHookOpts {
    fn any_enabled(&self) -> bool {
        self.tmux != TmuxMode::Off || self.prompt
    }
}

pub fn cmd() -> Command {
    Command::new("completion")
        .add(crate::shellnav::ShellNav::none())
        .about("Output shell integration (completions + wrapper function) [read-only]")
        .long_about(
            "Output shell integration (completions + wrapper function) [read-only].\n\n\
             Prints a shell script that provides tab completion and the `wsp cd` wrapper \
             function.\n\n\
             zsh:        eval \"$(wsp completion zsh)\"\n\
             bash:       eval \"$(wsp completion bash)\"\n\
             fish:       wsp completion fish | source\n\
             powershell: Invoke-Expression (wsp completion powershell | Out-String)\n\n\
             To load automatically in every new shell, add the appropriate line above to \
             your shell's startup file:\n\n\
             zsh/bash:   echo 'eval \"$(wsp completion <shell>)\"' >> ~/.zshrc  (or ~/.bashrc)\n\
             fish:       echo 'wsp completion fish | source' >> ~/.config/fish/config.fish\n\
             powershell: Add-Content -Path $PROFILE -Value \
             \"`nInvoke-Expression (wsp completion powershell | Out-String)\"\n\n\
             PowerShell: if $PROFILE does not exist yet, run first:\n\
             \x20\x20New-Item -ItemType File -Force $PROFILE",
        )
        .arg(
            Arg::new("shell")
                .required(true)
                .value_parser(["zsh", "bash", "fish", "powershell"]),
        )
}

pub fn run(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let shell = matches.get_one::<String>("shell").unwrap();
    // Config load must not break shell startup — fall back to defaults on any error.
    // This handles version skew (e.g. newer config format with older binary), corrupt
    // config, or missing files gracefully.
    let hooks = match Config::load_from(&paths.config_path) {
        Ok(cfg) => {
            // SECURITY: closed match — only literal "window-title" produces shell
            // code. Arbitrary strings from hand-edited config fall to Off.
            let tmux = match cfg.shell_tmux_mode() {
                Some("window-title") => TmuxMode::WindowTitle,
                _ => TmuxMode::Off,
            };
            ShellHookOpts {
                tmux,
                prompt: cfg.shell_prompt_enabled(),
            }
        }
        Err(e) => {
            eprintln!("wsp: warning: failed to load config, shell hooks disabled: {e}");
            ShellHookOpts::default()
        }
    };
    match shell.as_str() {
        "zsh" => {
            generate_posix(&mut std::io::stdout(), paths, "zsh", hooks)?;
            Ok(Output::None)
        }
        "bash" => {
            generate_posix(&mut std::io::stdout(), paths, "bash", hooks)?;
            Ok(Output::None)
        }
        "fish" => {
            generate_fish(&mut std::io::stdout(), paths, hooks)?;
            Ok(Output::None)
        }
        "powershell" => {
            generate_powershell(&mut std::io::stdout(), paths, hooks)?;
            Ok(Output::None)
        }
        _ => bail!(
            "unsupported shell: {} (supported: zsh, bash, fish, powershell)",
            shell
        ),
    }
}

// ---------- shared helpers ----------

fn bin_path() -> Result<String> {
    let bin = std::env::current_exe()
        .map_err(|e| anyhow::anyhow!("cannot determine executable path: {}", e))?;
    Ok(bin.display().to_string())
}

/// Escape a string for embedding inside POSIX single quotes.
/// Single quotes have no escape mechanism, so we close the quote, add an
/// escaped literal single quote, and re-open: `'` → `'\''`
fn posix_escape(s: &str) -> String {
    s.replace('\'', "'\\''")
}

/// Escape a string for embedding inside fish single quotes.
/// Fish supports `\'` inside single-quoted strings.
fn fish_escape(s: &str) -> String {
    s.replace('\'', "\\'")
}

/// Escape a string for embedding inside PowerShell single quotes.
/// Single quotes are escaped by doubling them: `'` → `''`
fn ps_escape(s: &str) -> String {
    s.replace('\'', "''")
}

// ---------- zsh / bash (POSIX-like) ----------

fn generate_posix(
    w: &mut dyn Write,
    paths: &Paths,
    shell: &str,
    hooks: ShellHookOpts,
) -> Result<()> {
    let bin_str = bin_path()?;
    let wsp_root = paths.workspaces_dir.display().to_string();
    write_posix(w, &bin_str, &wsp_root, shell, hooks)
}

fn write_posix(
    w: &mut dyn Write,
    bin_str: &str,
    wsp_root: &str,
    shell: &str,
    hooks: ShellHookOpts,
) -> Result<()> {
    let cases = build_posix_cases();
    let bin_esc = posix_escape(bin_str);
    let root_esc = posix_escape(wsp_root);

    // IMPORTANT: do NOT add a leading comment here.
    // `eval $(wsp completion zsh)` (without quotes) is a common mistake; a `#`
    // at the start triggers word-splitting re-evaluation of anything on that
    // line (e.g. the embedded `$(wsp completion zsh)` in the comment text),
    // causing an infinite loop or confusing errors. Start with executable code.
    write!(
        w,
        "wsp() {{\n\
         \x20 local wsp_bin='{bin_esc}'\n\
         \x20 local wsp_root='{root_esc}'\n\
         \n\
         \x20 case \"$1\" in\n",
    )?;

    for case in &cases {
        write!(
            w,
            "    {})\n\
             \x20     {}\n\
             \x20     ;;\n",
            case.pattern, case.body
        )?;
    }

    write!(
        w,
        "    *)\n\
         \x20     command \"$wsp_bin\" \"$@\"\n\
         \x20     ;;\n\
         \x20 esac\n\
         }}\n\
         \n"
    )?;

    if shell == "zsh" {
        // Guard against compdef not being available yet (compinit not loaded).
        // Clap's generated completions call `compdef` at the end, which fails
        // if compinit hasn't run. Define a temporary no-op stub, source the
        // completions, then remove the stub so compinit can define the real one.
        writeln!(w, "if ! (( $+functions[compdef] )); then")?;
        writeln!(w, "  compdef() {{ :; }}")?;
        writeln!(w, "  source <(COMPLETE={shell} '{bin_esc}')")?;
        writeln!(w, "  unfunction compdef")?;
        writeln!(
            w,
            "  echo >&2 'wsp: compinit not loaded — tab completions disabled. Add \"autoload -Uz compinit && compinit\" before eval \"$(wsp completion zsh)\" in your .zshrc'"
        )?;
        writeln!(w, "else")?;
        writeln!(w, "  source <(COMPLETE={shell} '{bin_esc}')")?;
        writeln!(w, "fi")?;
    } else {
        writeln!(w, "source <(COMPLETE={shell} '{bin_esc}')")?;
    }

    // Experimental: shell hooks for workspace detection, tmux title, prompt variable
    if hooks.any_enabled() {
        write_posix_hooks(w, &root_esc, shell, hooks)?;
    }

    Ok(())
}

struct ShellCase {
    pattern: String,
    body: String,
}

fn build_posix_cases() -> Vec<ShellCase> {
    vec![
        ShellCase {
            // `create` is a visible alias for `new` (see cli/new.rs) — both must
            // cd, or `wsp create` silently falls through to the catch-all.
            pattern: "new|create".to_string(),
            body: build_posix_cd_into("new"),
        },
        ShellCase {
            pattern: "cd".to_string(),
            // Pass the output through when it is not a directory. `cd`'s
            // stdout is the structured-output channel, so with --json it
            // carries a document rather than a path; capturing it and cd'ing
            // blind turned `wsp cd <ws> --json` into an error. See #125.
            body: "shift\n\
                 \x20     local _wsp_out\n\
                 \x20     _wsp_out=$(WSP_SHELL=1 command \"$wsp_bin\" cd \"$@\") || return\n\
                 \x20     if [[ -d \"$_wsp_out\" ]]; then\n\
                 \x20       cd -- \"$_wsp_out\"\n\
                 \x20     else\n\
                 \x20       printf '%s\\n' \"$_wsp_out\"\n\
                 \x20     fi"
                .to_string(),
        },
        ShellCase {
            pattern: "rename".to_string(),
            body: build_posix_rename(),
        },
        ShellCase {
            // Both spellings share one body, which always invokes `rm`.
            pattern: "rm|remove".to_string(),
            body: build_posix_cd_out("rm"),
        },
        ShellCase {
            pattern: "recover".to_string(),
            body: build_posix_recover(),
        },
    ]
}

/// Shell body for `wsp new`: run the command, then cd to wherever it says it
/// landed.
///
/// The destination comes from the binary via `WSP_CD_FILE` rather than from
/// argv. `new` has five value-taking flags and can derive the workspace name
/// from `-b`, so no positional scan here can be correct — see `shellcd`.
fn build_posix_cd_into(cmd_name: &str) -> String {
    format!(
        "shift\n\
         \x20     local _wsp_cd _wsp_rc\n\
         \x20     _wsp_cd=$(mktemp) || return\n\
         \x20     WSP_CD_FILE=\"$_wsp_cd\" command \"$wsp_bin\" {cmd_name} \"$@\"\n\
         \x20     _wsp_rc=$?\n\
         \x20     if [[ $_wsp_rc -eq 0 && -s \"$_wsp_cd\" ]]; then\n\
         \x20       cd -- \"$(<\"$_wsp_cd\")\"\n\
         \x20     fi\n\
         \x20     rm -f \"$_wsp_cd\"\n\
         \x20     return $_wsp_rc",
    )
}

/// Shell body for `wsp recover`: cd to whatever the binary reports.
///
/// This used to derive the workspace name from `$1` and hardcode a skip list
/// for the `ls`/`list`/`show` subcommands. It no longer needs either: only the
/// restore path calls `cd_request`, so the read-only subcommands report nothing
/// and the wrapper moves nowhere. That deletes the skip list, the argv scan, and
/// the test that guarded both.
fn build_posix_recover() -> String {
    build_posix_cd_into("recover")
}

/// Shell body for `wsp rename`: vacate, rename, then follow the workspace to
/// its new location — but only if we were standing in it.
///
/// Composes both mechanisms already in use here. The vacate step is `rm`'s: on
/// Windows a directory that is a live process's cwd cannot be renamed, so the
/// shell must step out before the binary runs, and only the shell can move
/// itself. The destination comes from the binary like `new`'s, because `rename`
/// takes the workspace name as an *optional* positional (falling back to CWD
/// detection), so there is no reliable way to derive the new path from argv.
///
/// `$_wsp_prev` surviving means the rename did not affect us — either it failed,
/// or we were somewhere else entirely — so return there and nothing appears to
/// move. Landing at the new workspace root loses a subdirectory position
/// (`<ws>/sub` becomes `<ws-new>`); reconstructing the relative subpath is
/// possible but not worth the shell complexity.
fn build_posix_rename() -> String {
    "shift\n\
     \x20     local _wsp_prev=\"$PWD\" _wsp_oldpwd=\"$OLDPWD\" _wsp_cd _wsp_rc\n\
     \x20     _wsp_cd=$(mktemp) || return\n\
     \x20     cd \"$wsp_root\" 2>/dev/null || cd \"$HOME\" || return\n\
     \x20     WSP_PWD=\"$_wsp_prev\" WSP_CD_FILE=\"$_wsp_cd\" command \"$wsp_bin\" rename \"$@\"\n\
     \x20     _wsp_rc=$?\n\
     \x20     if [[ -d \"$_wsp_prev\" ]]; then\n\
     \x20       [[ -n \"$_wsp_oldpwd\" && -d \"$_wsp_oldpwd\" ]] && cd \"$_wsp_oldpwd\"\n\
     \x20       cd \"$_wsp_prev\"\n\
     \x20     elif [[ $_wsp_rc -eq 0 && -s \"$_wsp_cd\" ]]; then\n\
     \x20       cd -- \"$(<\"$_wsp_cd\")\"\n\
     \x20     fi\n\
     \x20     rm -f \"$_wsp_cd\"\n\
     \x20     return $_wsp_rc"
        .to_string()
}

/// Shell body for `wsp rm`: step out of the way, remove, then step back if the
/// directory survived.
///
/// The shell must vacate before the binary runs — on Windows a directory that
/// is a live process's cwd cannot be renamed, so removal fails outright — and
/// only the shell can move itself, so this cannot be delegated to the binary
/// the way `new` delegates its destination.
///
/// It deliberately does *not* work out whether `$PWD` is inside the target.
/// That comparison was a string match against `$wsp_root/<name>`, which yields
/// a false negative on 8.3 short paths, trailing separators, slash direction,
/// and junctions. On unix a false negative is harmless — the rename succeeds
/// regardless — but on Windows it means `wsp rm` fails for no visible reason.
/// Leaving unconditionally and returning only if the old directory still exists
/// is correct in every case and has nothing to spell wrong:
///
/// - removed while inside  -> old dir is gone   -> stay at the root
/// - removal blocked       -> old dir survives  -> return, as if nothing moved
/// - never inside          -> old dir survives  -> return, no visible change
///
/// Returning goes *via* the shell's previous directory when one exists, so the
/// detour does not clobber `cd -`. Two cds would leave the previous directory
/// set to `$wsp_root` — the wrapper's scratch stop — instead of wherever the
/// user actually came from. Assigning `$OLDPWD` fixes that in bash but not in
/// zsh, which keeps its own record and ignores the assignment; stepping through
/// the old directory works in both because it is the real thing rather than a
/// variable standing in for it.
///
/// Costs one extra `cd`, and under zsh's AUTO_PUSHD it adds one more entry to
/// the directory stack. Both seemed better than silently breaking `cd -`.
fn build_posix_cd_out(cmd_name: &str) -> String {
    format!(
        "shift\n\
         \x20     local _wsp_prev=\"$PWD\" _wsp_oldpwd=\"$OLDPWD\"\n\
         \x20     cd \"$wsp_root\" 2>/dev/null || cd \"$HOME\" || return\n\
         \x20     local _wsp_rc\n\
         \x20     WSP_PWD=\"$_wsp_prev\" command \"$wsp_bin\" {cmd_name} \"$@\"\n\
         \x20     _wsp_rc=$?\n\
         \x20     if [[ -d \"$_wsp_prev\" ]]; then\n\
         \x20       [[ -n \"$_wsp_oldpwd\" && -d \"$_wsp_oldpwd\" ]] && cd \"$_wsp_oldpwd\"\n\
         \x20       cd \"$_wsp_prev\"\n\
         \x20     fi\n\
         \x20     return $_wsp_rc",
    )
}

fn write_posix_hooks(
    w: &mut dyn Write,
    root_esc: &str,
    shell: &str,
    hooks: ShellHookOpts,
) -> Result<()> {
    writeln!(w)?;
    writeln!(
        w,
        "# wsp shell hooks (experimental) — workspace detection + integrations"
    )?;
    writeln!(w, "_wsp_hook() {{")?;
    writeln!(w, "  local wsp_root='{root_esc}'")?;
    writeln!(w, "  if [[ \"$PWD\" = \"$wsp_root\"/* ]]; then")?;
    writeln!(w, "    local _wsp_ws=\"${{PWD#$wsp_root/}}\"")?;
    writeln!(w, "    _wsp_ws=\"${{_wsp_ws%%/*}}\"")?;
    writeln!(w, "    export WSP_WORKSPACE=\"$_wsp_ws\"")?;
    writeln!(w, "  else")?;
    writeln!(w, "    unset WSP_WORKSPACE")?;
    writeln!(w, "  fi")?;

    if hooks.tmux == TmuxMode::WindowTitle {
        writeln!(w)?;
        writeln!(
            w,
            "  if [ -n \"$TMUX\" ] && command -v tmux >/dev/null 2>&1; then"
        )?;
        writeln!(
            w,
            "    if [ \"$(tmux display-message -p '#{{pane_id}}')\" = \"$TMUX_PANE\" ]; then"
        )?;
        writeln!(w, "      if [ -n \"$WSP_WORKSPACE\" ]; then")?;
        // Skip if user explicitly set the window title (automatic-rename off without our marker)
        writeln!(
            w,
            "        if [ \"$(tmux show-window-option -v automatic-rename 2>/dev/null)\" = \"off\" ] \\"
        )?;
        writeln!(
            w,
            "           && [ -z \"$(tmux show-window-option -v @wsp-title 2>/dev/null)\" ]; then"
        )?;
        writeln!(w, "          : # user owns this title, skip")?;
        writeln!(w, "        else")?;
        writeln!(w, "          tmux rename-window \"wsp:$WSP_WORKSPACE\"")?;
        writeln!(
            w,
            "          tmux set-window-option @wsp-title on >/dev/null 2>&1"
        )?;
        writeln!(w, "        fi")?;
        writeln!(w, "      else")?;
        // Only restore automatic-rename if wsp was the one who set the title
        writeln!(
            w,
            "        if [ -n \"$(tmux show-window-option -v @wsp-title 2>/dev/null)\" ]; then"
        )?;
        writeln!(
            w,
            "          tmux set-window-option automatic-rename on >/dev/null 2>&1"
        )?;
        writeln!(
            w,
            "          tmux set-window-option -u @wsp-title >/dev/null 2>&1"
        )?;
        writeln!(w, "        fi")?;
        writeln!(w, "      fi")?;
        writeln!(w, "    fi")?;
        writeln!(w, "  fi")?;
    }

    writeln!(w, "}}")?;
    writeln!(w)?;

    // Hook registration differs by shell
    if shell == "zsh" {
        writeln!(w, "autoload -Uz add-zsh-hook")?;
        writeln!(w, "add-zsh-hook precmd _wsp_hook")?;
    } else {
        // bash
        writeln!(w, "if [[ ! \"$PROMPT_COMMAND\" == *_wsp_hook* ]]; then")?;
        writeln!(
            w,
            "  PROMPT_COMMAND=\"_wsp_hook${{PROMPT_COMMAND:+;$PROMPT_COMMAND}}\""
        )?;
        writeln!(w, "fi")?;
    }

    // Trigger on initial load
    writeln!(w, "_wsp_hook")?;

    Ok(())
}

// ---------- fish ----------

fn generate_fish(w: &mut dyn Write, paths: &Paths, hooks: ShellHookOpts) -> Result<()> {
    let bin_str = bin_path()?;
    let wsp_root = paths.workspaces_dir.display().to_string();
    write_fish(w, &bin_str, &wsp_root, hooks)
}

fn write_fish(
    w: &mut dyn Write,
    bin_str: &str,
    wsp_root: &str,
    hooks: ShellHookOpts,
) -> Result<()> {
    let bin_esc = fish_escape(bin_str);
    let root_esc = fish_escape(wsp_root);

    write!(
        w,
        "\
# wsp shell integration \u{2014} source with: wsp completion fish | source\n\
\n\
function wsp\n\
    set -l wsp_bin '{bin_esc}'\n\
    set -l wsp_root '{root_esc}'\n\
\n\
    switch $argv[1]\n\
        case new create\n\
            set -l args $argv[2..]\n\
            set -l cdfile (mktemp)\n\
            WSP_CD_FILE=$cdfile command $wsp_bin new $args\n\
            set -l rc $status\n\
            if test $rc -eq 0 -a -s $cdfile\n\
                cd -- (cat $cdfile)\n\
            end\n\
            rm -f $cdfile\n\
            return $rc\n\
\n\
        case cd\n\
            set -l args $argv[2..]\n\
            set -l out (WSP_SHELL=1 command $wsp_bin cd $args); or return\n\
            if test -d \"$out\"\n\
                cd -- \"$out\"\n\
            else\n\
                printf '%s\\n' $out\n\
            end\n\
\n\
        case rename\n\
            set -l args $argv[2..]\n\
            set -l prev $PWD\n\
            set -l oldpwd \"\"\n\
            if test (count $dirprev) -gt 0\n\
                set oldpwd $dirprev[-1]\n\
            end\n\
            set -l cdfile (mktemp)\n\
            cd \"$wsp_root\" 2>/dev/null; or cd $HOME; or return 1\n\
            WSP_PWD=$prev WSP_CD_FILE=$cdfile command $wsp_bin rename $args\n\
            set -l rc $status\n\
            if test -d \"$prev\"\n\
                if test -n \"$oldpwd\" -a -d \"$oldpwd\"\n\
                    cd \"$oldpwd\"\n\
                end\n\
                cd \"$prev\"\n\
            else if test $rc -eq 0 -a -s $cdfile\n\
                cd -- (cat $cdfile)\n\
            end\n\
            rm -f $cdfile\n\
            return $rc\n\
\n\
        case rm remove\n\
            set -l args $argv[2..]\n\
            set -l prev $PWD\n\
            set -l oldpwd \"\"\n\
            if test (count $dirprev) -gt 0\n\
                set oldpwd $dirprev[-1]\n\
            end\n\
            cd \"$wsp_root\" 2>/dev/null; or cd $HOME; or return 1\n\
            WSP_PWD=$prev command $wsp_bin rm $args\n\
            set -l rc $status\n\
            if test -d \"$prev\"\n\
                if test -n \"$oldpwd\" -a -d \"$oldpwd\"\n\
                    cd \"$oldpwd\"\n\
                end\n\
                cd \"$prev\"\n\
            end\n\
            return $rc\n\
\n\
        case recover\n\
            set -l args $argv[2..]\n\
            set -l cdfile (mktemp)\n\
            WSP_CD_FILE=$cdfile command $wsp_bin recover $args\n\
            set -l rc $status\n\
            if test $rc -eq 0 -a -s $cdfile\n\
                cd -- (cat $cdfile)\n\
            end\n\
            rm -f $cdfile\n\
            return $rc\n\
\n\
        case '*'\n\
            command $wsp_bin $argv\n\
    end\n\
end\n\
\n\
COMPLETE=fish '{bin_esc}' | source\n"
    )?;

    if hooks.any_enabled() {
        write_fish_hooks(w, &root_esc, hooks)?;
    }

    Ok(())
}

fn write_fish_hooks(w: &mut dyn Write, root_esc: &str, hooks: ShellHookOpts) -> Result<()> {
    writeln!(w)?;
    writeln!(
        w,
        "# wsp shell hooks (experimental) — workspace detection + integrations"
    )?;
    writeln!(w, "function _wsp_hook --on-variable PWD")?;
    writeln!(w, "    set -l wsp_root '{root_esc}'")?;
    writeln!(w, "    if string match -q \"$wsp_root/*\" $PWD")?;
    writeln!(
        w,
        "        set -gx WSP_WORKSPACE (string split / (string replace \"$wsp_root/\" '' $PWD))[1]"
    )?;
    writeln!(w, "    else")?;
    writeln!(w, "        set -ge WSP_WORKSPACE")?;
    writeln!(w, "    end")?;

    if hooks.tmux == TmuxMode::WindowTitle {
        writeln!(w)?;
        writeln!(w, "    if set -q TMUX; and command -q tmux")?;
        writeln!(
            w,
            "        if test (tmux display-message -p '#{{pane_id}}') = $TMUX_PANE"
        )?;
        writeln!(w, "            if set -q WSP_WORKSPACE")?;
        // Skip if user explicitly set the window title (automatic-rename off without our marker)
        writeln!(
            w,
            "                set -l _ar (tmux show-window-option -v automatic-rename 2>/dev/null)"
        )?;
        writeln!(
            w,
            "                set -l _wt (tmux show-window-option -v @wsp-title 2>/dev/null)"
        )?;
        writeln!(
            w,
            "                if test \"$_ar\" = off; and test -z \"$_wt\""
        )?;
        writeln!(w, "                    : # user owns this title, skip")?;
        writeln!(w, "                else")?;
        writeln!(
            w,
            "                    tmux rename-window \"wsp:$WSP_WORKSPACE\""
        )?;
        writeln!(
            w,
            "                    tmux set-window-option @wsp-title on >/dev/null 2>&1"
        )?;
        writeln!(w, "                end")?;
        writeln!(w, "            else")?;
        // Only restore automatic-rename if wsp was the one who set the title
        writeln!(
            w,
            "                set -l _wt (tmux show-window-option -v @wsp-title 2>/dev/null)"
        )?;
        writeln!(w, "                if test -n \"$_wt\"")?;
        writeln!(
            w,
            "                    tmux set-window-option automatic-rename on >/dev/null 2>&1"
        )?;
        writeln!(
            w,
            "                    tmux set-window-option -u @wsp-title >/dev/null 2>&1"
        )?;
        writeln!(w, "                end")?;
        writeln!(w, "            end")?;
        writeln!(w, "        end")?;
        writeln!(w, "    end")?;
    }

    writeln!(w, "end")?;
    writeln!(w)?;
    writeln!(w, "# Trigger on initial load")?;
    writeln!(w, "_wsp_hook")?;

    Ok(())
}

// ---------- powershell ----------

fn generate_powershell(w: &mut dyn Write, paths: &Paths, _hooks: ShellHookOpts) -> Result<()> {
    // TODO: shell hooks (tmux window title, prompt) not yet implemented for PowerShell
    let bin_str = bin_path()?;
    let wsp_root = paths.workspaces_dir.display().to_string();
    write_powershell(w, &bin_str, &wsp_root)
}

fn write_powershell(w: &mut dyn Write, bin_str: &str, wsp_root: &str) -> Result<()> {
    let bin_esc = ps_escape(bin_str);
    let root_esc = ps_escape(wsp_root);

    writeln!(w, "function wsp {{")?;
    writeln!(w, "    $wspBin = '{bin_esc}'")?;
    writeln!(w, "    $wspRoot = '{root_esc}'")?;
    writeln!(w)?;
    writeln!(w, "    switch ($args[0]) {{")?;
    // The destination comes from the binary via WSP_CD_FILE, not from argv.
    // `new` has five value-taking flags and can derive the name from -b, so no
    // scan here can be correct — the previous "first non-flag arg" version sent
    // `wsp new -w existing new-ws` into `existing`. See the shellcd module.
    writeln!(w, "        {{ $_ -in 'new', 'create' }} {{")?;
    writeln!(
        w,
        "            $restArgs = @($args | Select-Object -Skip 1)"
    )?;
    writeln!(
        w,
        "            $cdFile = [System.IO.Path]::GetTempFileName()"
    )?;
    writeln!(w, "            $env:WSP_CD_FILE = $cdFile")?;
    writeln!(w, "            & $wspBin new @restArgs")?;
    writeln!(w, "            $rc = $LASTEXITCODE")?;
    writeln!(
        w,
        "            Remove-Item Env:\\WSP_CD_FILE -ErrorAction SilentlyContinue"
    )?;
    writeln!(
        w,
        "            $dest = if (Test-Path -LiteralPath $cdFile) {{ (Get-Content -LiteralPath $cdFile -Raw) }} else {{ '' }}"
    )?;
    writeln!(
        w,
        "            Remove-Item -LiteralPath $cdFile -ErrorAction SilentlyContinue"
    )?;
    // Get-Content -Raw yields $null for an empty file, and $null.Trim() throws,
    // so test with IsNullOrWhiteSpace rather than calling a method on $dest.
    writeln!(
        w,
        "            if ($rc -eq 0 -and -not [string]::IsNullOrWhiteSpace($dest)) {{"
    )?;
    // -LiteralPath: without it Set-Location treats [ and ] as wildcards, so a
    // workspace path containing them would fail to resolve.
    writeln!(w, "                Set-Location -LiteralPath $dest.Trim()")?;
    writeln!(w, "            }}")?;
    // Cmdlets do not touch $LASTEXITCODE, so it would survive on its own —
    // restore it explicitly anyway, to match the rm case and to keep the exit
    // status a stated contract rather than an inherited side effect.
    writeln!(
        w,
        "            if ($rc -ne 0) {{ $global:LASTEXITCODE = $rc }}"
    )?;
    writeln!(w, "        }}")?;
    writeln!(w, "        'cd' {{")?;
    writeln!(
        w,
        "            $restArgs = @($args | Select-Object -Skip 1)"
    )?;
    writeln!(w, "            $env:WSP_SHELL = '1'")?;
    writeln!(w, "            $out = & $wspBin cd @restArgs")?;
    writeln!(
        w,
        "            Remove-Item Env:\\WSP_SHELL -ErrorAction SilentlyContinue"
    )?;
    // -Raw so a multi-line document (--json) survives as one string rather
    // than an array of lines.
    writeln!(w, "            $rc = $LASTEXITCODE")?;
    writeln!(w, "            $text = ($out -join \"`n\")")?;
    writeln!(w, "            if ($rc -eq 0 -and $text) {{")?;
    writeln!(
        w,
        "                if (Test-Path -LiteralPath $text -PathType Container) {{"
    )?;
    writeln!(w, "                    Set-Location -LiteralPath $text")?;
    writeln!(w, "                }} else {{")?;
    writeln!(w, "                    Write-Output $text")?;
    writeln!(w, "                }}")?;
    writeln!(w, "            }}")?;
    writeln!(w, "        }}")?;
    writeln!(w, "        {{ $_ -in 'rm', 'remove' }} {{")?;
    writeln!(
        w,
        "            $restArgs = @($args | Select-Object -Skip 1)"
    )?;
    // Vacate unconditionally rather than testing whether $PWD is inside the
    // target. The old `-eq`/`-like` comparison against "$wspRoot\<name>" gave a
    // false negative on 8.3 short paths, trailing separators, slash direction,
    // and junctions — and on Windows a false negative is fatal, because a
    // directory that is a live process's cwd cannot be renamed, so `wsp rm`
    // failed outright. See build_posix_cd_out for the case analysis.
    writeln!(w, "            $prev = $PWD.Path")?;
    // try/catch expresses `cd "$wsp_root" || cd "$HOME"` directly. Inferring
    // failure from "$PWD did not change" would misfire when the shell is
    // already at the root, sending it to $HOME for no reason.
    writeln!(
        w,
        "            try {{ Set-Location -LiteralPath $wspRoot -ErrorAction Stop }}"
    )?;
    writeln!(
        w,
        "            catch {{ Set-Location -LiteralPath $HOME -ErrorAction SilentlyContinue }}"
    )?;
    writeln!(w, "            $env:WSP_PWD = $prev")?;
    writeln!(w, "            & $wspBin rm @restArgs")?;
    writeln!(w, "            $rc = $LASTEXITCODE")?;
    writeln!(
        w,
        "            Remove-Item Env:\\WSP_PWD -ErrorAction SilentlyContinue"
    )?;
    writeln!(
        w,
        "            if (Test-Path -LiteralPath $prev -PathType Container) {{"
    )?;
    writeln!(w, "                Set-Location -LiteralPath $prev")?;
    writeln!(w, "            }}")?;
    writeln!(
        w,
        "            if ($rc -ne 0) {{ $global:LASTEXITCODE = $rc }}"
    )?;
    writeln!(w, "        }}")?;
    // See build_posix_rename: vacate so Windows can rename at all, then follow
    // the workspace to wherever the binary says it went, but only if we were
    // standing in it.
    writeln!(w, "        'rename' {{")?;
    writeln!(
        w,
        "            $restArgs = @($args | Select-Object -Skip 1)"
    )?;
    writeln!(w, "            $prev = $PWD.Path")?;
    writeln!(
        w,
        "            $cdFile = [System.IO.Path]::GetTempFileName()"
    )?;
    writeln!(w, "            $env:WSP_CD_FILE = $cdFile")?;
    writeln!(
        w,
        "            try {{ Set-Location -LiteralPath $wspRoot -ErrorAction Stop }}"
    )?;
    writeln!(
        w,
        "            catch {{ Set-Location -LiteralPath $HOME -ErrorAction SilentlyContinue }}"
    )?;
    writeln!(w, "            $env:WSP_PWD = $prev")?;
    writeln!(w, "            & $wspBin rename @restArgs")?;
    writeln!(w, "            $rc = $LASTEXITCODE")?;
    writeln!(
        w,
        "            Remove-Item Env:\\WSP_CD_FILE -ErrorAction SilentlyContinue"
    )?;
    writeln!(
        w,
        "            $dest = if (Test-Path -LiteralPath $cdFile) {{ (Get-Content -LiteralPath $cdFile -Raw) }} else {{ '' }}"
    )?;
    writeln!(
        w,
        "            Remove-Item -LiteralPath $cdFile -ErrorAction SilentlyContinue"
    )?;
    writeln!(
        w,
        "            if (Test-Path -LiteralPath $prev -PathType Container) {{"
    )?;
    writeln!(w, "                Set-Location -LiteralPath $prev")?;
    writeln!(
        w,
        "            }} elseif ($rc -eq 0 -and -not [string]::IsNullOrWhiteSpace($dest)) {{"
    )?;
    writeln!(w, "                Set-Location -LiteralPath $dest.Trim()")?;
    writeln!(w, "            }}")?;
    writeln!(
        w,
        "            if ($rc -ne 0) {{ $global:LASTEXITCODE = $rc }}"
    )?;
    writeln!(w, "        }}")?;
    // Same WSP_CD_FILE form as `new`. Every successful `recover` restores a
    // workspace and reports its destination -- listing moved to `wsp ls
    // --removed` precisely so this stays true -- so the wrapper needs no skip
    // list.
    writeln!(w, "        'recover' {{")?;
    writeln!(
        w,
        "            $restArgs = @($args | Select-Object -Skip 1)"
    )?;
    writeln!(
        w,
        "            $cdFile = [System.IO.Path]::GetTempFileName()"
    )?;
    writeln!(w, "            $env:WSP_CD_FILE = $cdFile")?;
    writeln!(w, "            & $wspBin recover @restArgs")?;
    writeln!(w, "            $rc = $LASTEXITCODE")?;
    writeln!(
        w,
        "            Remove-Item Env:\\WSP_CD_FILE -ErrorAction SilentlyContinue"
    )?;
    writeln!(
        w,
        "            $dest = if (Test-Path -LiteralPath $cdFile) {{ (Get-Content -LiteralPath $cdFile -Raw) }} else {{ '' }}"
    )?;
    writeln!(
        w,
        "            Remove-Item -LiteralPath $cdFile -ErrorAction SilentlyContinue"
    )?;
    writeln!(
        w,
        "            if ($rc -eq 0 -and -not [string]::IsNullOrWhiteSpace($dest)) {{"
    )?;
    writeln!(w, "                Set-Location -LiteralPath $dest.Trim()")?;
    writeln!(w, "            }}")?;
    writeln!(
        w,
        "            if ($rc -ne 0) {{ $global:LASTEXITCODE = $rc }}"
    )?;
    writeln!(w, "        }}")?;
    writeln!(w, "        default {{")?;
    writeln!(w, "            & $wspBin @args")?;
    writeln!(w, "        }}")?;
    writeln!(w, "    }}")?;
    writeln!(w, "}}")?;
    writeln!(w)?;
    // Register-ArgumentCompleter without -Native fires for functions, not just
    // external executables. -Native would be ignored because `wsp` is a function.
    writeln!(
        w,
        "Register-ArgumentCompleter -CommandName wsp -ScriptBlock {{"
    )?;
    writeln!(
        w,
        "    param($wordToComplete, $commandAst, $cursorPosition)"
    )?;
    writeln!(w, "    $prev = $env:COMPLETE")?;
    writeln!(w, "    $env:COMPLETE = 'powershell'")?;
    writeln!(w, "    if ($wordToComplete -eq '') {{")?;
    // PS 5.1 silently drops empty-string args to native exes when using &.
    // --% (stop-parsing) passes the trailing `""` as a raw command-line token;
    // CommandLineToArgvW then gives clap_complete the empty argv slot it needs.
    // $argStr is expanded in the here-string before Invoke-Expression sees --%.
    writeln!(w, "        $argStr = $commandAst.Extent.Text")?;
    writeln!(
        w,
        "        $argStr = $argStr.Substring(0, [math]::Min($cursorPosition, $argStr.Length))"
    )?;
    writeln!(w, "        $results = Invoke-Expression @\"")?;
    writeln!(w, "& '{bin_esc}' --% -- $argStr `\"`\"")?;
    writeln!(w, "\"@")?;
    writeln!(w, "    }} else {{")?;
    // Use CommandElements directly to avoid re-evaluating user-typed text
    // through Invoke-Expression (prevents injection via $() in argument values).
    writeln!(
        w,
        "        $tokens = @($commandAst.CommandElements | ForEach-Object {{ $_.Extent.Text }})"
    )?;
    writeln!(w, "        $results = & '{bin_esc}' -- @tokens")?;
    writeln!(w, "    }}")?;
    writeln!(
        w,
        "    if ($null -eq $prev) {{ Remove-Item Env:\\COMPLETE }} else {{ $env:COMPLETE = $prev }}"
    )?;
    writeln!(w, "    $results | ForEach-Object {{")?;
    writeln!(w, "        $split = $_ -split \"`t\"")?;
    writeln!(w, "        $cmd = $split[0]")?;
    writeln!(
        w,
        "        $help = if ($split.Length -ge 2) {{ $split[1] }} else {{ $split[0] }}"
    )?;
    writeln!(
        w,
        "        [System.Management.Automation.CompletionResult]::new($cmd, $cmd, 'ParameterValue', $help)"
    )?;
    writeln!(w, "    }}")?;
    writeln!(w, "}}")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(f: impl Fn(&mut Vec<u8>) -> Result<()>) -> String {
        let mut buf = Vec::new();
        f(&mut buf).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn test_posix_quotes_bin_path_and_wsp_root() {
        struct Case {
            name: &'static str,
            shell: &'static str,
        }

        let cases = vec![
            Case {
                name: "zsh",
                shell: "zsh",
            },
            Case {
                name: "bash",
                shell: "bash",
            },
        ];

        for tc in cases {
            let out = output(|w| {
                write_posix(
                    w,
                    "/opt/my tools/ws",
                    "/home/user/dev",
                    tc.shell,
                    ShellHookOpts::default(),
                )
            });
            assert!(
                out.contains("local wsp_bin='/opt/my tools/ws'"),
                "case {}: wsp_bin should be single-quoted",
                tc.name
            );
            assert!(
                out.contains("local wsp_root='/home/user/dev'"),
                "case {}: wsp_root should be single-quoted",
                tc.name
            );
            // wsp_root should be referenced as $wsp_root, not interpolated.
            // No trailing slash: nothing builds "$wsp_root/<name>" any more, the
            // wrapper only cds to the root itself.
            assert!(
                out.contains("$wsp_root"),
                "case {}: wsp_root should be referenced as variable",
                tc.name
            );
            assert!(
                !out.contains("\"/home/user/dev/"),
                "case {}: wsp_root should not be interpolated directly into commands",
                tc.name
            );
            assert!(
                out.contains(&format!(
                    "source <(COMPLETE={} '/opt/my tools/ws')",
                    tc.shell
                )),
                "case {}: COMPLETE line should be single-quoted",
                tc.name
            );
        }
    }

    #[test]
    fn test_posix_contains_all_cases() {
        let out = output(|w| {
            write_posix(
                w,
                "/usr/bin/ws",
                "/home/user/dev",
                "zsh",
                ShellHookOpts::default(),
            )
        });
        for pattern in &["new|create)", "cd)", "rm|remove)", "recover)", "*)"] {
            assert!(out.contains(pattern), "missing case pattern: {}", pattern);
        }
    }

    /// Guards against the wrapper and clap drifting apart: if `new` gains or
    /// loses a visible alias, the hardcoded shell patterns must be updated.
    #[test]
    fn test_new_visible_aliases_match_wrapper() {
        let aliases: Vec<String> = crate::cli::new::cmd()
            .get_visible_aliases()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            aliases,
            vec!["create".to_string()],
            "`new` aliases changed — update the wrapper patterns in write_posix/write_fish/write_powershell"
        );
    }

    #[test]
    fn test_fish_contains_all_cases() {
        let out = output(|w| {
            write_fish(
                w,
                "/usr/bin/wsp",
                "/home/user/dev",
                ShellHookOpts::default(),
            )
        });
        for pattern in &[
            "case new create",
            "case cd",
            "case rm remove",
            "case recover",
            "case '*'",
        ] {
            assert!(out.contains(pattern), "missing case pattern: {}", pattern);
        }
    }

    #[test]
    fn test_posix_shell_name_in_source_line() {
        // Shell name must appear in the COMPLETE=<shell> source line.
        let bash = output(|w| {
            write_posix(
                w,
                "/usr/bin/ws",
                "/home/user/dev",
                "bash",
                ShellHookOpts::default(),
            )
        });
        assert!(
            bash.contains("COMPLETE=bash"),
            "bash source line must contain shell name"
        );

        let zsh = output(|w| {
            write_posix(
                w,
                "/usr/bin/ws",
                "/home/user/dev",
                "zsh",
                ShellHookOpts::default(),
            )
        });
        assert!(
            zsh.contains("COMPLETE=zsh"),
            "zsh source line must contain shell name"
        );
    }

    // Regression test for https://github.com/jganoff/wsp/issues/51:
    // `eval $(wsp completion zsh)` (without quotes) word-splits the output and
    // re-evaluates tokens; a leading `#` comment containing `$(wsp ...)` causes
    // an infinite loop. The script must start with executable code, never a comment.
    #[test]
    fn test_posix_output_does_not_start_with_comment() {
        for shell in ["zsh", "bash"] {
            let out = output(|w| {
                write_posix(
                    w,
                    "/usr/bin/wsp",
                    "/home/user/dev",
                    shell,
                    ShellHookOpts::default(),
                )
            });
            assert!(
                !out.trim_start().starts_with('#'),
                "{shell} completion output must not start with a '#' comment (breaks eval $(...) without quotes)"
            );
        }
    }

    #[test]
    fn test_fish_quotes_bin_path_and_wsp_root() {
        let out = output(|w| {
            write_fish(
                w,
                "/opt/my tools/ws",
                "/home/user/dev",
                ShellHookOpts::default(),
            )
        });
        assert!(
            out.contains("set -l wsp_bin '/opt/my tools/ws'"),
            "wsp_bin should be single-quoted"
        );
        assert!(
            out.contains("set -l wsp_root '/home/user/dev'"),
            "wsp_root should be single-quoted"
        );
        assert!(
            out.contains("$wsp_root"),
            "wsp_root should be referenced as variable"
        );
        assert!(
            !out.contains("\"/home/user/dev/"),
            "wsp_root should not be interpolated directly"
        );
        assert!(
            out.contains("COMPLETE=fish '/opt/my tools/ws' | source"),
            "COMPLETE line should be single-quoted"
        );
    }

    #[test]
    fn test_fish_header() {
        let out =
            output(|w| write_fish(w, "/usr/bin/ws", "/home/user/dev", ShellHookOpts::default()));
        assert!(out.contains("wsp completion fish | source"));
    }

    #[test]
    fn test_posix_path_with_dollar_sign() {
        let out = output(|w| {
            write_posix(
                w,
                "/opt/$weird/ws",
                "/home/user/dev",
                "bash",
                ShellHookOpts::default(),
            )
        });
        // Single quotes prevent $weird from being expanded
        assert!(out.contains("local wsp_bin='/opt/$weird/ws'"));
        assert!(out.contains("COMPLETE=bash '/opt/$weird/ws'"));
    }

    #[test]
    fn test_posix_path_with_single_quote() {
        let out = output(|w| {
            write_posix(
                w,
                "/usr/bin/wsp",
                "/home/o'brien/dev",
                "bash",
                ShellHookOpts::default(),
            )
        });
        // Single quote in wsp_root must be escaped as '\''
        assert!(
            out.contains(r"local wsp_root='/home/o'\''brien/dev'"),
            "wsp_root single quote must be escaped: {}",
            out
        );
    }

    #[test]
    fn test_posix_bin_with_single_quote() {
        let out = output(|w| {
            write_posix(
                w,
                "/opt/it's here/wsp",
                "/home/user/dev",
                "bash",
                ShellHookOpts::default(),
            )
        });
        assert!(
            out.contains(r"local wsp_bin='/opt/it'\''s here/wsp'"),
            "wsp_bin single quote must be escaped: {}",
            out
        );
        assert!(
            out.contains(r"COMPLETE=bash '/opt/it'\''s here/wsp'"),
            "COMPLETE single quote must be escaped: {}",
            out
        );
    }

    #[test]
    fn test_fish_path_with_single_quote() {
        let out = output(|w| {
            write_fish(
                w,
                "/usr/bin/wsp",
                "/home/o'brien/dev",
                ShellHookOpts::default(),
            )
        });
        assert!(
            out.contains(r"set -l wsp_root '/home/o\'brien/dev'"),
            "fish wsp_root single quote must be escaped: {}",
            out
        );
    }

    #[test]
    fn test_fish_bin_with_single_quote() {
        let out = output(|w| {
            write_fish(
                w,
                "/opt/it's here/wsp",
                "/home/user/dev",
                ShellHookOpts::default(),
            )
        });
        assert!(
            out.contains(r"set -l wsp_bin '/opt/it\'s here/wsp'"),
            "fish wsp_bin single quote must be escaped: {}",
            out
        );
        assert!(
            out.contains(r"COMPLETE=fish '/opt/it\'s here/wsp' | source"),
            "fish COMPLETE single quote must be escaped: {}",
            out
        );
    }

    #[test]
    fn test_zsh_compdef_guard() {
        let out = output(|w| {
            write_posix(
                w,
                "/usr/bin/wsp",
                "/home/user/dev",
                "zsh",
                ShellHookOpts::default(),
            )
        });
        assert!(
            out.contains("if ! (( $+functions[compdef] ))"),
            "zsh output should guard against missing compdef"
        );
        assert!(
            out.contains("unfunction compdef"),
            "zsh output should clean up stub compdef"
        );
        assert!(
            out.contains("compinit not loaded"),
            "zsh output should warn when compinit is missing"
        );
    }

    #[test]
    fn test_bash_no_compdef_guard() {
        let out = output(|w| {
            write_posix(
                w,
                "/usr/bin/wsp",
                "/home/user/dev",
                "bash",
                ShellHookOpts::default(),
            )
        });
        assert!(
            !out.contains("compdef"),
            "bash output should not have compdef guard"
        );
    }

    // --- Shell hook tests ---

    #[test]
    fn test_no_hooks_by_default() {
        let opts = ShellHookOpts::default();
        for shell in &["zsh", "bash"] {
            let out = output(|w| write_posix(w, "/usr/bin/wsp", "/home/user/dev", shell, opts));
            assert!(
                !out.contains("_wsp_hook"),
                "{}: should not emit hooks when disabled",
                shell
            );
            assert!(
                !out.contains("WSP_WORKSPACE"),
                "{}: should not emit WSP_WORKSPACE when disabled",
                shell
            );
        }
        let out = output(|w| write_fish(w, "/usr/bin/wsp", "/home/user/dev", opts));
        assert!(!out.contains("_wsp_hook"), "fish: no hooks when disabled");
    }

    #[test]
    fn test_prompt_only_hooks() {
        let opts = ShellHookOpts {
            prompt: true,
            tmux: TmuxMode::Off,
        };
        // zsh
        let out = output(|w| write_posix(w, "/usr/bin/wsp", "/home/user/dev", "zsh", opts));
        assert!(out.contains("_wsp_hook"), "zsh: hook function emitted");
        assert!(out.contains("WSP_WORKSPACE"), "zsh: sets WSP_WORKSPACE");
        assert!(out.contains("add-zsh-hook precmd"), "zsh: registers precmd");
        assert!(!out.contains("tmux rename-window"), "zsh: no tmux commands");

        // bash
        let out = output(|w| write_posix(w, "/usr/bin/wsp", "/home/user/dev", "bash", opts));
        assert!(
            out.contains("PROMPT_COMMAND"),
            "bash: registers PROMPT_COMMAND"
        );
        assert!(
            !out.contains("tmux rename-window"),
            "bash: no tmux commands"
        );

        // fish
        let out = output(|w| write_fish(w, "/usr/bin/wsp", "/home/user/dev", opts));
        assert!(out.contains("--on-variable PWD"), "fish: PWD hook");
        assert!(out.contains("WSP_WORKSPACE"), "fish: sets WSP_WORKSPACE");
        assert!(
            !out.contains("tmux rename-window"),
            "fish: no tmux commands"
        );
    }

    #[test]
    fn test_tmux_window_title_hooks() {
        let opts = ShellHookOpts {
            prompt: false,
            tmux: TmuxMode::WindowTitle,
        };
        let out = output(|w| write_posix(w, "/usr/bin/wsp", "/home/user/dev", "zsh", opts));
        assert!(out.contains("_wsp_hook"), "hook function emitted");
        assert!(out.contains("WSP_WORKSPACE"), "sets WSP_WORKSPACE");
        assert!(
            out.contains("tmux rename-window"),
            "tmux rename-window present"
        );
        assert!(
            out.contains("automatic-rename on"),
            "restores automatic-rename when leaving workspace"
        );
        assert!(out.contains("$TMUX"), "guards on TMUX env var");
        assert!(
            out.contains("command -v tmux"),
            "guards on tmux availability"
        );
        assert!(
            out.contains("display-message -p") && out.contains("TMUX_PANE"),
            "guards on active pane"
        );
        assert!(
            out.contains("@wsp-title"),
            "uses @wsp-title marker to track ownership"
        );

        let out = output(|w| write_fish(w, "/usr/bin/wsp", "/home/user/dev", opts));
        assert!(
            out.contains("tmux rename-window"),
            "fish: tmux rename-window present"
        );
        assert!(
            out.contains("command -q tmux"),
            "fish: guards on tmux availability"
        );
        assert!(
            out.contains("display-message -p") && out.contains("TMUX_PANE"),
            "fish: guards on active pane"
        );
        assert!(
            out.contains("@wsp-title"),
            "fish: uses @wsp-title marker to track ownership"
        );
    }

    #[test]
    fn test_both_hooks() {
        let opts = ShellHookOpts {
            prompt: true,
            tmux: TmuxMode::WindowTitle,
        };
        let out = output(|w| write_posix(w, "/usr/bin/wsp", "/home/user/dev", "zsh", opts));
        assert!(out.contains("WSP_WORKSPACE"));
        assert!(out.contains("tmux rename-window"));
        assert!(out.contains("add-zsh-hook precmd"));
    }

    #[test]
    fn test_hook_path_escaping() {
        let opts = ShellHookOpts {
            prompt: true,
            tmux: TmuxMode::Off,
        };
        let out = output(|w| write_posix(w, "/usr/bin/wsp", "/home/o'brien/dev", "zsh", opts));
        assert!(
            out.contains(r"local wsp_root='/home/o'\''brien/dev'"),
            "hook wsp_root must escape single quotes: {}",
            out
        );
    }

    // Regression: rm cd-out check must require a path separator after the workspace
    // name, otherwise `wsp rm foo` incorrectly cds you out when you're in `foobar`.

    // --- PowerShell tests ---

    #[test]
    fn test_ps_quotes_bin_path_and_wsp_root() {
        let out = output(|w| write_powershell(w, r"C:\path\to\wsp.exe", r"C:\Users\user\dev"));
        assert!(
            out.contains(r"$wspBin = 'C:\path\to\wsp.exe'"),
            "wsp_bin should be single-quoted"
        );
        assert!(
            out.contains(r"$wspRoot = 'C:\Users\user\dev'"),
            "wsp_root should be single-quoted"
        );
    }

    #[test]
    fn test_ps_contains_all_cases() {
        let out = output(|w| write_powershell(w, r"C:\wsp.exe", r"C:\dev"));
        assert!(out.contains("'new', 'create'"), "missing new/create case");
        assert!(out.contains("'cd'"), "missing cd case");
        assert!(out.contains("'rm', 'remove'"), "missing rm/remove case");
        assert!(out.contains("'recover'"), "missing recover case");
        assert!(out.contains("default"), "missing default case");
    }

    /// Slice the generated script from `start` up to `end`, so an assertion
    /// about one `case` branch cannot accidentally be satisfied by another.
    ///
    /// Without this, `!out.contains(...)` reads as a guard on the branch you
    /// have in mind while actually matching a sibling branch — which is how
    /// `test_ps_new_skips_flag_args_when_cding` kept passing after the code it
    /// described had been deleted.
    fn case_body<'a>(out: &'a str, start: &str, end: &str) -> &'a str {
        let i = out
            .find(start)
            .unwrap_or_else(|| panic!("case marker not found: {start}"));
        let rest = &out[i..];
        let j = rest
            .find(end)
            .unwrap_or_else(|| panic!("end marker not found after {start}: {end}"));
        &rest[..j]
    }

    /// `new` must take its destination from the binary, never from argv.
    ///
    /// Both argv strategies are wrong: `$1` mistakes a leading flag for the
    /// name, and scanning for the first non-flag token mistakes a flag's value
    /// for it, sending `wsp new -w existing new-ws` into `existing`. This
    /// asserts the argv derivation is gone from every dialect, not merely that
    /// one spelling of it is absent.
    #[test]
    fn test_destination_comes_from_binary() {
        let posix = output(|w| {
            write_posix(
                w,
                "/usr/bin/wsp",
                "/home/user/dev",
                "zsh",
                ShellHookOpts::default(),
            )
        });
        let posix_new = case_body(&posix, "new|create)", "    cd)");
        assert!(
            posix_new.contains("WSP_CD_FILE"),
            "posix new must pass WSP_CD_FILE"
        );
        assert!(
            !posix_new.contains("$wsp_root/$"),
            "posix new must not build the destination from a positional arg"
        );

        let fish = output(|w| {
            write_fish(
                w,
                "/usr/bin/wsp",
                "/home/user/dev",
                ShellHookOpts::default(),
            )
        });
        let fish_new = case_body(&fish, "case new create", "case cd");
        assert!(
            fish_new.contains("WSP_CD_FILE"),
            "fish new must pass WSP_CD_FILE"
        );
        assert!(
            !fish_new.contains("$wsp_root/$"),
            "fish new must not build the destination from a positional arg"
        );

        let pwsh = output(|w| write_powershell(w, r"C:\wsp.exe", r"C:\dev"));
        // recover now uses the same mechanism, so its old argv scan and
        // ls/list/show skip list must be gone from every dialect.
        let posix_recover = case_body(&posix, "    recover)", "    *)");
        assert!(
            posix_recover.contains("WSP_CD_FILE"),
            "posix recover must pass WSP_CD_FILE"
        );
        assert!(
            !posix_recover.contains("!= ls"),
            "posix recover must not keep a subcommand skip list"
        );

        let fish_recover = case_body(&fish, "case recover", "case '*'");
        assert!(
            fish_recover.contains("WSP_CD_FILE"),
            "fish recover must pass WSP_CD_FILE"
        );
        assert!(
            !fish_recover.contains("ls|list|show"),
            "fish recover must not keep a subcommand skip list"
        );

        let pwsh_recover = case_body(&pwsh, "'recover' {", "default {");
        assert!(
            pwsh_recover.contains("$env:WSP_CD_FILE"),
            "powershell recover must pass WSP_CD_FILE"
        );
        assert!(
            !pwsh_recover.contains("$wspName"),
            "powershell recover must not scan argv"
        );

        let pwsh_new = case_body(&pwsh, "$_ -in 'new', 'create'", "'cd' {");
        assert!(
            pwsh_new.contains("$env:WSP_CD_FILE"),
            "powershell new must pass WSP_CD_FILE"
        );
        assert!(
            !pwsh_new.contains("$wspName"),
            "powershell new must not scan argv for the workspace name"
        );
        assert!(
            !pwsh_new.contains("Join-Path $wspRoot"),
            "powershell new must not build the destination from $wspRoot"
        );
    }

    #[test]
    fn test_ps_complete_registration() {
        let out = output(|w| write_powershell(w, r"C:\wsp.exe", r"C:\dev"));
        // Must use non-Native so the completer fires for the wsp function, not
        // just for external executables.
        assert!(
            out.contains("Register-ArgumentCompleter -CommandName wsp -ScriptBlock"),
            "must register non-native completer for the wsp function"
        );
        assert!(
            !out.contains("Register-ArgumentCompleter -Native"),
            "-Native would only fire for external executables, not the wsp function"
        );
        assert!(
            out.contains("$env:COMPLETE = 'powershell'"),
            "scriptblock must set COMPLETE env var"
        );
        assert!(
            out.contains(r"& 'C:\wsp.exe' -- @tokens"),
            "non-empty word branch must use CommandElements tokens (avoids Invoke-Expression injection)"
        );
        assert!(
            out.contains(r"& 'C:\wsp.exe' --% -- $argStr"),
            "empty word branch must use --% to pass empty string (PS 5.1 drops bare '' args)"
        );
        assert!(
            !out.contains("$argStr += \" ''\""),
            "should not use += '' workaround (PS 5.1 silently drops empty args to native exes)"
        );
        assert!(
            out.contains("CompletionResult"),
            "scriptblock must return CompletionResult objects"
        );
    }

    #[test]
    fn test_ps_bin_with_single_quote() {
        let out = output(|w| write_powershell(w, r"C:\it's\wsp.exe", r"C:\dev"));
        assert!(
            out.contains(r"$wspBin = 'C:\it''s\wsp.exe'"),
            "wsp_bin single quote must be doubled: {}",
            out
        );
        assert!(
            out.contains(r"& 'C:\it''s\wsp.exe' -- @tokens"),
            "completion scriptblock single quote must be doubled: {}",
            out
        );
    }

    #[test]
    fn test_ps_root_with_single_quote() {
        let out = output(|w| write_powershell(w, r"C:\wsp.exe", r"C:\o'brien\dev"));
        assert!(
            out.contains(r"$wspRoot = 'C:\o''brien\dev'"),
            "wsp_root single quote must be doubled: {}",
            out
        );
    }

    /// `rm` must vacate unconditionally and come back only if the old directory
    /// survived — with no path comparison anywhere.
    ///
    /// The comparison this replaces gave false negatives on 8.3 short paths,
    /// trailing separators, slash direction and junctions. On unix that was
    /// harmless; on Windows it made `wsp rm` fail outright, because a directory
    /// that is a live process's cwd cannot be renamed.
    ///
    /// The prefix-match hazard the previous tests guarded (`rm w` while sitting
    /// in `w-extra`) is now covered behaviorally in tests/shell_cd.rs, which is
    /// a stronger check than asserting on the shape of the comparison.
    #[test]
    fn test_rm_vacates_without_comparing_paths() {
        for shell in &["zsh", "bash"] {
            let out = output(|w| {
                write_posix(
                    w,
                    "/usr/bin/wsp",
                    "/home/user/dev",
                    shell,
                    ShellHookOpts::default(),
                )
            });
            let body = case_body(&out, "    rm|remove)", "    recover)");
            assert!(
                body.contains("_wsp_prev=\"$PWD\""),
                "{shell}: rm must remember where it started"
            );
            assert!(
                body.contains("-d \"$_wsp_prev\""),
                "{shell}: rm must return only if the old directory survived"
            );
            assert!(
                !body.contains("wsp_dir"),
                "{shell}: rm must not compare against a computed workspace dir"
            );
        }

        let fish = output(|w| {
            write_fish(
                w,
                "/usr/bin/wsp",
                "/home/user/dev",
                ShellHookOpts::default(),
            )
        });
        let fish_rm = case_body(&fish, "case rm remove", "case recover");
        assert!(
            fish_rm.contains("set -l prev $PWD"),
            "fish: rm must remember where it started"
        );
        assert!(
            !fish_rm.contains("wsp_dir"),
            "fish: rm must not compare against a computed workspace dir"
        );

        let pwsh = output(|w| write_powershell(w, r"C:\wsp.exe", r"C:\dev"));
        let ps_rm = case_body(&pwsh, "$_ -in 'rm', 'remove'", "'recover' {");
        assert!(
            ps_rm.contains("$prev = $PWD.Path"),
            "powershell: rm must remember where it started"
        );
        assert!(
            !ps_rm.contains("$wspDir"),
            "powershell: rm must not compare against a computed workspace dir"
        );
        // `remove` must still normalize to the real subcommand name.
        assert!(
            ps_rm.contains("& $wspBin rm @restArgs"),
            "powershell: remove must normalize to rm"
        );
    }

    /// Case labels present in the generated posix wrapper.
    fn posix_case_names(out: &str) -> Vec<String> {
        let mut v = Vec::new();
        for line in out.lines() {
            let t = line.trim();
            if let Some(pat) = t.strip_suffix(')')
                && !pat.is_empty()
                && pat != "*"
                && !pat.contains(' ')
            {
                v.extend(pat.split('|').map(String::from));
            }
        }
        v.sort();
        v
    }

    /// Case labels present in the generated fish wrapper.
    fn fish_case_names(out: &str) -> Vec<String> {
        let mut v = Vec::new();
        for line in out.lines() {
            if let Some(rest) = line.trim().strip_prefix("case ") {
                v.extend(
                    rest.split_whitespace()
                        .filter(|t| *t != "'*'")
                        .map(String::from),
                );
            }
        }
        v.sort();
        v
    }

    /// Case labels present in the generated PowerShell wrapper. Handles both
    /// `'cd' {` and `{ $_ -in 'rm', 'remove' } {`.
    fn ps_case_names(out: &str) -> Vec<String> {
        let mut v = Vec::new();
        for line in out.lines() {
            let t = line.trim();
            if !t.ends_with('{') {
                continue;
            }
            if !(t.starts_with('\'') || t.starts_with("{ $_ -in")) {
                continue;
            }
            let mut rest = t;
            while let Some(i) = rest.find('\'') {
                rest = &rest[i + 1..];
                match rest.find('\'') {
                    Some(j) => {
                        v.push(rest[..j].to_string());
                        rest = &rest[j + 1..];
                    }
                    None => break,
                }
            }
        }
        v.sort();
        v
    }

    fn generated_posix() -> String {
        output(|w| {
            write_posix(
                w,
                "/usr/bin/wsp",
                "/home/user/dev",
                "zsh",
                ShellHookOpts::default(),
            )
        })
    }

    /// The dialects must handle exactly the same set of commands.
    ///
    /// This is the #103 shape as a test: `create` was added to the posix wrapper
    /// but the alias was missing everywhere, and nothing compared the dialects
    /// to each other. A case added to one generator and forgotten in another is
    /// now a failure rather than a platform-specific bug found by a user.
    #[test]
    fn test_dialects_cover_the_same_commands() {
        let posix = posix_case_names(&generated_posix());
        let fish = fish_case_names(&output(|w| {
            write_fish(
                w,
                "/usr/bin/wsp",
                "/home/user/dev",
                ShellHookOpts::default(),
            )
        }));
        let ps = ps_case_names(&output(|w| write_powershell(w, r"C:\wsp.exe", r"C:\dev")));

        assert_eq!(
            posix, fish,
            "posix and fish wrappers handle different commands"
        );
        assert_eq!(
            posix, ps,
            "posix and powershell wrappers handle different commands"
        );
    }

    /// The wrapper's cases and the commands' own declarations must agree.
    ///
    /// This is the payoff of declaring `ShellNav` on the command: there is no
    /// list to maintain here. A command that says it moves the shell must have a
    /// wrapper case, and a wrapper case must correspond to a command that says
    /// so. Adding a command with a `ShellNav` and forgetting the generator fails
    /// here, as does the reverse.
    #[test]
    fn test_shellnav_matches_wrapper_cases() {
        // posix_case_names already sorts.
        let mut cases = posix_case_names(&generated_posix());
        cases.dedup();

        let mut declared: Vec<String> = Vec::new();
        for sub in crate::cli::build_cli().get_subcommands() {
            // Declared gaps move the shell but have no wrapper case on
            // purpose; they are excluded here rather than silently passing as
            // safe.
            if sub
                .get::<crate::shellnav::ShellNav>()
                .is_some_and(|n| n.moves_shell() && !n.is_unhandled_gap())
            {
                declared.push(sub.get_name().to_string());
                declared.extend(sub.get_visible_aliases().map(String::from));
            }
        }
        declared.sort();
        declared.dedup();

        assert_eq!(
            declared, cases,
            "commands declaring ShellNav and wrapper cases disagree.\n  \
             declared on commands: {declared:?}\n  \
             cases in the wrapper:  {cases:?}"
        );
    }

    /// Every top-level command must declare how it affects the shell's cwd.
    ///
    /// Requiring an explicit declaration — including `ShellNav::none()` — is
    /// what removes the hand-maintained exemption list this replaced. Absence
    /// would be indistinguishable from "nobody thought about it", which is how
    /// `wsp rename` shipped for five releases moving the workspace directory
    /// with no wrapper case at all.
    ///
    /// Hidden commands are exempt: they are codegen helpers, not user surface.
    #[test]
    fn test_every_command_declares_shell_nav() {
        let cli = crate::cli::build_cli();
        let missing: Vec<&str> = cli
            .get_subcommands()
            .filter(|s| !s.is_hide_set())
            .filter(|s| s.get::<crate::shellnav::ShellNav>().is_none())
            .map(|s| s.get_name())
            .collect();

        assert!(
            missing.is_empty(),
            "these commands do not declare a ShellNav: {missing:?}\n  \
             Add `.add(ShellNav::none())` if the command cannot move the shell's \
             directory, or the matching constructor if it can — and then give it a \
             wrapper case and a STRAND_CASES row in tests/shell_cd.rs."
        );
    }

    /// A command with subcommands must not also take a positional.
    ///
    /// clap resolves the first token as a subcommand when it can, so any
    /// positional value that happens to match a subcommand name is silently
    /// shadowed -- the user asked for one thing and got the other, with no
    /// error. `wsp recover` hit this: `recover ls` listed instead of restoring
    /// a workspace named `ls`, which reserved every subcommand name out of the
    /// workspace namespace and made the shell wrapper and the auto-gc gate
    /// wrong for one of the two forms. The listing moved to `wsp ls --removed`.
    ///
    /// Commands that are pure namespaces (`wsp repo`, `wsp config`) are fine:
    /// with no positional of their own there is nothing to shadow. Flags are
    /// fine too. It is the overlap that breaks.
    #[test]
    fn test_commands_with_subcommands_take_no_positionals() {
        fn walk(cmd: &clap::Command, path: &str, offenders: &mut Vec<String>) {
            let positionals: Vec<&str> =
                cmd.get_positionals().map(|a| a.get_id().as_str()).collect();
            let subs: Vec<&str> = cmd.get_subcommands().map(|c| c.get_name()).collect();
            if !positionals.is_empty() && !subs.is_empty() {
                offenders.push(format!(
                    "{path}: positionals {positionals:?} are shadowed by subcommands {subs:?}"
                ));
            }
            for sub in cmd.get_subcommands() {
                walk(sub, &format!("{path} {}", sub.get_name()), offenders);
            }
        }

        let cli = crate::cli::build_cli();
        let mut offenders = Vec::new();
        for sub in cli.get_subcommands() {
            walk(sub, sub.get_name(), &mut offenders);
        }

        assert!(
            offenders.is_empty(),
            "a positional value matching a subcommand name is silently \
             swallowed:\n  {}\n  \
             Make the subcommand a flag, or give it its own top-level command.",
            offenders.join("\n  ")
        );
    }

    /// Every user-facing top-level command is either exercised by the smoke
    /// scripts or named here as a known gap.
    ///
    /// Top-level only: `wsp repo add` marks the whole `repo` tree covered, so
    /// `repo rm` and friends are not tracked. Widening this to every leaf is
    /// worth doing; it needs the register to grow first.
    ///
    /// The smoke scripts are the only check that runs a real binary against a
    /// real filesystem on all three platforms, so anything they miss is only
    /// ever covered by unit tests that share the binary's assumptions. `wsp ls`
    /// printing "No workspaces." while swallowing the footer that says a
    /// workspace is recoverable passed every unit test and failed the smoke
    /// script on its first run.
    ///
    /// `UNSMOKED` is a debt register, not an exemption list: the assertion is an
    /// equality, so adding a command without smoking it fails, and smoking one
    /// without deleting its line here fails too. The list only shrinks.
    ///
    /// A command counts as covered if *either* script invokes it, since a few
    /// checks are legitimately platform-specific. Keeping the two scripts in
    /// step is convention, not enforced here. "Invokes" means the command runs,
    /// not that it succeeds -- a check asserting a command fails still counts.
    #[test]
    fn test_every_command_is_smoked_or_listed_as_a_gap() {
        /// Commands with no smoke coverage. Empty, and worth keeping that way:
        /// a new top-level command fails this test until it is smoked.
        ///
        /// If you must add an entry, say why here. Forcing stdin to a non-TTY
        /// (`< /dev/null`, or an empty-string pipe in PowerShell) is usually
        /// enough to make an interactive command assertable -- and it is what
        /// makes a check unable to hang, not what makes it able to.
        const UNSMOKED: &[&str] = &[];

        let sh = include_str!("../../../../scripts/smoke.sh");
        let ps = include_str!("../../../../scripts/smoke.ps1");

        // Match an invocation on a line that is not a comment. A plain
        // whole-file substring search would count a command named in a comment
        // and the equality below would then tell the next person to delete its
        // UNSMOKED line, baking in coverage that does not exist.
        //
        // Deliberately not anchored to the start of a statement: a check that
        // grows a `if out=$(... )` wrapper is still a check, and a guard that
        // reports lost coverage every time one is refactored trains people to
        // edit the register instead of reading it.
        let invoked = |name: &str| {
            let mentions = |src: &str, prefix: &str| {
                src.lines()
                    .filter(|l| !l.trim_start().starts_with('#'))
                    .any(|l| {
                        l.match_indices(prefix).any(|(i, _)| {
                            l[i + prefix.len()..]
                                .chars()
                                .next()
                                .is_none_or(|c| !c.is_alphanumeric() && c != '-' && c != '_')
                        })
                    })
            };
            mentions(sh, &format!("\"$WSP\" {name}")) || mentions(ps, &format!("Wsp {name}"))
        };

        let cli = crate::cli::build_cli();
        let mut gaps: Vec<&str> = cli
            .get_subcommands()
            .filter(|s| !s.is_hide_set())
            .map(|s| s.get_name())
            .filter(|name| !invoked(name))
            .collect();
        gaps.sort();

        let mut expected: Vec<&str> = UNSMOKED.to_vec();
        expected.sort();

        assert_eq!(
            gaps, expected,
            "smoke coverage and the UNSMOKED register disagree.\n  \
             unsmoked now:   {gaps:?}\n  \
             UNSMOKED says:  {expected:?}\n  \
             A command that gained coverage must lose its UNSMOKED line; a new \
             command needs a check in both scripts/smoke.sh and scripts/smoke.ps1."
        );
    }

    /// The two smoke scripts must assert the same things.
    ///
    /// The test above checks that every *command* is smoked somewhere. This
    /// checks the *checks*: adding one to `smoke.sh` and forgetting
    /// `smoke.ps1` is the obvious failure, and nothing else notices, because
    /// the PowerShell twin only ever runs in CI. Comparing the `ok`/`Ok`
    /// labels catches it at `cargo test` instead.
    ///
    /// It also means the labels are an interface: keep them identical across
    /// the two scripts, and put anything genuinely dialect-specific in
    /// `DIALECT_ONLY` with a reason.
    #[test]
    fn test_the_smoke_scripts_check_the_same_things() {
        /// Checks that exist in one dialect only, and why.
        const DIALECT_ONLY: &[&str] = &[
            // smoke.sh loops over bash and zsh; smoke.ps1 has one shell to test
            // and additionally parses the wrapper with the PowerShell parser.
            "completion $sh (parses)",
            "completion powershell",
            "completion output parses as PowerShell",
        ];

        /// Labels passed to `ok` (sh) or `Ok` (ps1).
        fn labels(src: &str, func: &str) -> Vec<String> {
            let mut found = Vec::new();
            for line in src.lines() {
                let mut rest = line;
                while let Some(at) = rest.find(func) {
                    // `ok "` must be a call, not the tail of another word.
                    let preceded_by_word = rest[..at]
                        .chars()
                        .next_back()
                        .is_some_and(|c| c.is_alphanumeric() || c == '_');
                    rest = &rest[at + func.len()..];
                    if preceded_by_word {
                        continue;
                    }
                    if let Some(end) = rest.find('"') {
                        found.push(rest[..end].to_string());
                    }
                }
            }
            found
        }

        let mut sh = labels(include_str!("../../../../scripts/smoke.sh"), "ok \"");
        let mut ps = labels(include_str!("../../../../scripts/smoke.ps1"), "Ok \"");
        for set in [&mut sh, &mut ps] {
            set.retain(|l| !DIALECT_ONLY.contains(&l.as_str()));
            set.sort();
            set.dedup();
        }

        let only_sh: Vec<&String> = sh.iter().filter(|l| !ps.contains(l)).collect();
        let only_ps: Vec<&String> = ps.iter().filter(|l| !sh.contains(l)).collect();
        assert!(
            only_sh.is_empty() && only_ps.is_empty(),
            "the smoke scripts have drifted apart.\n  \
             only in smoke.sh:  {only_sh:?}\n  \
             only in smoke.ps1: {only_ps:?}\n  \
             Add the missing check to the other script, or if it is genuinely \
             dialect-specific, add its label to DIALECT_ONLY with a reason."
        );
        // A guard against the extractor silently matching nothing.
        assert!(
            sh.len() > 20,
            "expected to find the checks, found {}",
            sh.len()
        );
    }

    /// `rename` must check the previous directory before the reported
    /// destination.
    ///
    /// Both flags are set for `rename`, and the order is load-bearing: rendering
    /// follow-first would teleport someone who ran `wsp rename w new` from
    /// `$HOME` into a workspace they were never in, because `$HOME` survives and
    /// a destination was reported. Nothing else pins this down, since `rename` is
    /// the only command with both flags.
    #[test]
    fn test_rename_prefers_previous_over_destination() {
        let generated = generated_posix();
        let body = case_body(&generated, "    rename)", "    rm|remove)");
        let prev_at = body
            .find("-d \"$_wsp_prev\"")
            .expect("rename must test the previous directory");
        let dest_at = body
            .find("-s \"$_wsp_cd\"")
            .expect("rename must test the reported destination");
        assert!(
            prev_at < dest_at,
            "rename must prefer the previous directory over the reported \
             destination; found the destination check first:\n{body}"
        );
    }
}
