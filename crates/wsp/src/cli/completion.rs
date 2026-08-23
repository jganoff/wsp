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
            pattern: "rm".to_string(),
            body: build_posix_cd_out("rm"),
        },
        ShellCase {
            pattern: "remove".to_string(),
            body: build_posix_cd_out("rm"),
        },
        ShellCase {
            pattern: "recover".to_string(),
            body: build_posix_recover(),
        },
    ]
}

fn build_posix_cd_into(cmd_name: &str) -> String {
    format!(
        "shift\n\
         \x20     command \"$wsp_bin\" {cmd_name} \"$@\" || return\n\
         \x20     local wsp_dir=\"$wsp_root/$1\"\n\
         \x20     cd \"$wsp_dir\"",
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

fn build_posix_cd_out(cmd_name: &str) -> String {
    format!(
        "shift\n\
         \x20     local _wsp_name\n\
         \x20     for _wsp_name in \"$@\"; do\n\
         \x20       [[ \"$_wsp_name\" != -* ]] && break\n\
         \x20       _wsp_name=\n\
         \x20     done\n\
         \x20     if [[ -n \"$_wsp_name\" ]]; then\n\
         \x20       local wsp_dir=\"$wsp_root/$_wsp_name\"\n\
         \x20       if [[ \"$PWD\" = \"$wsp_dir\" || \"$PWD\" = \"$wsp_dir\"/* ]]; then\n\
         \x20         cd \"$wsp_root\" || cd \"$HOME\"\n\
         \x20       fi\n\
         \x20     fi\n\
         \x20     command \"$wsp_bin\" {cmd_name} \"$@\"\n\
         \x20     if [[ ! -d \"$PWD\" ]]; then\n\
         \x20       cd \"$wsp_root\" || cd \"$HOME\"\n\
         \x20     fi",
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
            command $wsp_bin new $args; or return\n\
            set -l wsp_dir \"$wsp_root/$args[1]\"\n\
            cd $wsp_dir\n\
\n\
        case cd\n\
            set -l args $argv[2..]\n\
            set -l dir (WSP_SHELL=1 command $wsp_bin cd $args); or return\n\
            cd $dir\n\
\n\
        case rm remove\n\
            set -l args $argv[2..]\n\
            set -l _wsp_name\n\
            for _a in $args\n\
                if not string match -q -- '-*' $_a\n\
                    set _wsp_name $_a\n\
                    break\n\
                end\n\
            end\n\
            if test -n \"$_wsp_name\"\n\
                set -l wsp_dir \"$wsp_root/$_wsp_name\"\n\
                if string match -q -- \"$wsp_dir\" $PWD; or string match -q -- \"$wsp_dir/*\" $PWD\n\
                    cd \"$wsp_root\"; or cd $HOME\n\
                end\n\
            end\n\
            command $wsp_bin rm $args\n\
            if not test -d $PWD\n\
                cd \"$wsp_root\"; or cd $HOME\n\
            end\n\
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
    writeln!(w, "        {{ $_ -in 'new', 'create' }} {{")?;
    writeln!(
        w,
        "            $restArgs = @($args | Select-Object -Skip 1)"
    )?;
    writeln!(w, "            & $wspBin new @restArgs")?;
    writeln!(
        w,
        "            if ($LASTEXITCODE -eq 0 -and $restArgs.Count -gt 0) {{"
    )?;
    writeln!(w, "                $wspName = $null")?;
    writeln!(w, "                foreach ($a in $restArgs) {{")?;
    writeln!(
        w,
        "                    if (-not $a.StartsWith('-')) {{ $wspName = $a; break }}"
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
    writeln!(w, "            $wspName = $null")?;
    writeln!(w, "            foreach ($a in $restArgs) {{")?;
    writeln!(
        w,
        "                if (-not $a.StartsWith('-')) {{ $wspName = $a; break }}"
    )?;
    writeln!(w, "            }}")?;
    writeln!(w, "            if ($wspName) {{")?;
    writeln!(w, "                $wspDir = Join-Path $wspRoot $wspName")?;
    writeln!(
        w,
        "                if ($PWD.Path -eq $wspDir -or $PWD.Path -like \"$wspDir\\*\") {{"
    )?;
    writeln!(w, "                    Set-Location $wspRoot")?;
    writeln!(w, "                }}")?;
    writeln!(w, "            }}")?;
    writeln!(w, "            & $wspBin rm @restArgs")?;
    writeln!(
        w,
        "            if (-not (Test-Path -LiteralPath $PWD.Path -PathType Container)) {{"
    )?;
    writeln!(w, "                Set-Location $wspRoot")?;
    writeln!(w, "            }}")?;
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
        for pattern in &["new|create)", "cd)", "rm)", "remove)", "recover)", "*)"] {
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
    #[test]
    fn test_posix_rm_does_not_false_positive_on_prefix_match() {
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
            assert!(
                out.contains(r#"= "$wsp_dir" || "$PWD" = "$wsp_dir"/*"#),
                "{shell}: rm check must require separator, not bare prefix glob"
            );
        }
    }

    #[test]
    fn test_fish_rm_does_not_false_positive_on_prefix_match() {
        let out = output(|w| {
            write_fish(
                w,
                "/usr/bin/wsp",
                "/home/user/dev",
                ShellHookOpts::default(),
            )
        });
        assert!(
            out.contains(
                r#"string match -q -- "$wsp_dir" $PWD; or string match -q -- "$wsp_dir/*" $PWD"#
            ),
            "fish: rm check must require separator, not bare prefix glob"
        );
    }

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

    #[test]
    fn test_ps_new_skips_flag_args_when_cding() {
        let out = output(|w| write_powershell(w, r"C:\wsp.exe", r"C:\dev"));
        // Must iterate restArgs to find the workspace name, not blindly use restArgs[0],
        // so that `wsp new --template foo my-workspace` cds into my-workspace not --template.
        assert!(
            !out.contains("Set-Location (Join-Path $wspRoot $restArgs[0])"),
            "new must not use restArgs[0] directly as workspace name"
        );
        assert!(
            out.contains("$a.StartsWith('-')"),
            "new must skip flag arguments when looking for workspace name"
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
    fn test_ps_rm_cds_out_of_workspace() {
        let out = output(|w| write_powershell(w, r"C:\wsp.exe", r"C:\dev"));
        assert!(
            out.contains("'rm', 'remove'"),
            "rm/remove case must be present"
        );
        // Must cd out before deleting if currently inside the workspace
        assert!(
            out.contains("$wspDir = Join-Path $wspRoot $wspName"),
            "rm must compute workspace dir"
        );
        assert!(
            out.contains("$PWD.Path -eq $wspDir -or $PWD.Path -like \"$wspDir\\*\""),
            "rm must check if cwd is exactly or inside workspace (with separator)"
        );
        // Must always call `rm`, even when user typed `remove`
        assert!(
            out.contains("& $wspBin rm @restArgs"),
            "remove must normalize to rm"
        );
        // Must cd away if the directory disappeared (e.g. rm -f with no prompt)
        assert!(
            out.contains("Test-Path -LiteralPath $PWD.Path -PathType Container"),
            "rm must cd away if cwd no longer exists"
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
}
