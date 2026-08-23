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
            body: "shift\n\
                 \x20     local dir\n\
                 \x20     dir=$(WSP_SHELL=1 command \"$wsp_bin\" cd \"$@\") || return\n\
                 \x20     cd \"$dir\""
                .to_string(),
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

/// Shell body for `wsp recover <name>`: run the command, then cd into the
/// restored workspace. Only cds when the first argument is a plain workspace
/// name (not a subcommand like `ls`/`show` or a flag).
fn build_posix_recover() -> String {
    "shift\n\
     \x20     command \"$wsp_bin\" recover \"$@\" || return\n\
     \x20     local _wsp_name=\"$1\"\n\
     \x20     if [[ -n \"$_wsp_name\" && \"$_wsp_name\" != ls && \"$_wsp_name\" != list && \"$_wsp_name\" != show && \"$_wsp_name\" != -* ]]; then\n\
     \x20       cd \"$wsp_root/$_wsp_name\"\n\
     \x20     fi"
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
fn build_posix_cd_out(cmd_name: &str) -> String {
    format!(
        "shift\n\
         \x20     local _wsp_prev=\"$PWD\"\n\
         \x20     cd \"$wsp_root\" 2>/dev/null || cd \"$HOME\" || return\n\
         \x20     local _wsp_rc\n\
         \x20     command \"$wsp_bin\" {cmd_name} \"$@\"\n\
         \x20     _wsp_rc=$?\n\
         \x20     if [[ -d \"$_wsp_prev\" ]]; then\n\
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
            set -l dir (WSP_SHELL=1 command $wsp_bin cd $args); or return\n\
            cd $dir\n\
\n\
        case rm remove\n\
            set -l args $argv[2..]\n\
            set -l prev $PWD\n\
            cd \"$wsp_root\" 2>/dev/null; or cd $HOME; or return 1\n\
            command $wsp_bin rm $args\n\
            set -l rc $status\n\
            if test -d \"$prev\"\n\
                cd \"$prev\"\n\
            end\n\
            return $rc\n\
\n\
        case recover\n\
            set -l args $argv[2..]\n\
            command $wsp_bin recover $args; or return\n\
            set -l _wsp_name $args[1]\n\
            if test -n \"$_wsp_name\"; and not string match -qr '^(ls|list|show|-.*)$' -- \"$_wsp_name\"\n\
                cd \"$wsp_root/$_wsp_name\"\n\
            end\n\
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
    writeln!(w, "            $dir = & $wspBin cd @restArgs")?;
    writeln!(
        w,
        "            Remove-Item Env:\\WSP_SHELL -ErrorAction SilentlyContinue"
    )?;
    writeln!(w, "            if ($LASTEXITCODE -eq 0 -and $dir) {{")?;
    writeln!(w, "                Set-Location $dir")?;
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
    writeln!(w, "            & $wspBin rm @restArgs")?;
    writeln!(w, "            $rc = $LASTEXITCODE")?;
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
    writeln!(w, "        'recover' {{")?;
    writeln!(
        w,
        "            $restArgs = @($args | Select-Object -Skip 1)"
    )?;
    writeln!(w, "            & $wspBin recover @restArgs")?;
    writeln!(
        w,
        "            if ($LASTEXITCODE -eq 0 -and $restArgs.Count -gt 0) {{"
    )?;
    writeln!(w, "                $wspName = $null")?;
    writeln!(w, "                foreach ($a in $restArgs) {{")?;
    writeln!(
        w,
        "                    if (-not $a.StartsWith('-') -and $a -notin 'ls', 'list', 'show') {{ $wspName = $a; break }}"
    )?;
    writeln!(w, "                }}")?;
    writeln!(w, "                if ($wspName) {{")?;
    writeln!(
        w,
        "                    Set-Location (Join-Path $wspRoot $wspName)"
    )?;
    writeln!(w, "                }}")?;
    writeln!(w, "            }}")?;
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
            // wsp_root should be referenced as $wsp_root, not interpolated
            assert!(
                out.contains("$wsp_root/"),
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
    fn test_posix_recover_cds_into_workspace() {
        let out = output(|w| {
            write_posix(
                w,
                "/usr/bin/wsp",
                "/home/user/dev",
                "zsh",
                ShellHookOpts::default(),
            )
        });
        assert!(out.contains("recover)"), "recover case must be present");
        assert!(
            out.contains("cd \"$wsp_root/$_wsp_name\""),
            "recover must cd into restored workspace"
        );
        // Must guard against subcommands
        assert!(
            out.contains("ls") && out.contains("show"),
            "recover must skip cd for ls/show subcommands"
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
    fn test_fish_recover_cds_into_workspace() {
        let out = output(|w| {
            write_fish(
                w,
                "/usr/bin/wsp",
                "/home/user/dev",
                ShellHookOpts::default(),
            )
        });
        assert!(out.contains("case recover"), "recover case must be present");
        assert!(
            out.contains("cd \"$wsp_root/$_wsp_name\""),
            "recover must cd into restored workspace"
        );
        assert!(
            out.contains("ls|list|show"),
            "recover must skip cd for ls/list/show subcommands"
        );
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
            out.contains("$wsp_root/"),
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
    fn test_new_takes_destination_from_binary() {
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
    fn test_ps_recover_cds_into_workspace() {
        let out = output(|w| write_powershell(w, r"C:\wsp.exe", r"C:\dev"));
        assert!(out.contains("'recover'"), "recover case must be present");
        assert!(
            out.contains("Set-Location (Join-Path $wspRoot $wspName)"),
            "recover must cd into restored workspace"
        );
        assert!(
            out.contains("ls") && out.contains("list") && out.contains("show"),
            "recover must skip cd for ls/list/show subcommands"
        );
        assert!(
            out.contains("$a.StartsWith('-')"),
            "recover must skip cd for flag arguments"
        );
        assert!(
            !out.contains("$wspName = $restArgs[0]"),
            "recover must not use restArgs[0] directly (flags before workspace name would be used as workspace name)"
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
}
