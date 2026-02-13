use std::io::Write;

use anyhow::{Result, bail};
use clap::ArgMatches;

use crate::config::Paths;
use crate::output::Output;

pub fn run(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let shell = matches.get_one::<String>("shell").unwrap();
    match shell.as_str() {
        "zsh" => {
            generate_posix(&mut std::io::stdout(), paths, "zsh")?;
            Ok(Output::None)
        }
        "bash" => {
            generate_posix(&mut std::io::stdout(), paths, "bash")?;
            Ok(Output::None)
        }
        "fish" => {
            generate_fish(&mut std::io::stdout(), paths)?;
            Ok(Output::None)
        }
        _ => bail!("unsupported shell: {} (supported: zsh, bash, fish)", shell),
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

// ---------- zsh / bash (POSIX-like) ----------

fn generate_posix(w: &mut dyn Write, paths: &Paths, shell: &str) -> Result<()> {
    let bin_str = bin_path()?;
    let wsp_root = paths.workspaces_dir.display().to_string();
    write_posix(w, &bin_str, &wsp_root, shell)
}

fn write_posix(w: &mut dyn Write, bin_str: &str, wsp_root: &str, shell: &str) -> Result<()> {
    let cases = build_posix_cases();
    let bin_esc = posix_escape(bin_str);
    let root_esc = posix_escape(wsp_root);

    write!(
        w,
        "# wsp shell integration \u{2014} source with: eval \"$(wsp setup completion {shell})\"\n\
         \n\
         wsp() {{\n\
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

    writeln!(w, "source <(COMPLETE={shell} '{bin_esc}')")?;

    Ok(())
}

struct ShellCase {
    pattern: String,
    body: String,
}

fn build_posix_cases() -> Vec<ShellCase> {
    vec![
        ShellCase {
            pattern: "new".to_string(),
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

fn build_posix_cd_out(cmd_name: &str) -> String {
    format!(
        "shift\n\
         \x20     if [[ -n \"$1\" ]]; then\n\
         \x20       local wsp_dir=\"$wsp_root/$1\"\n\
         \x20       if [[ \"$PWD\" = \"$wsp_dir\"* ]]; then\n\
         \x20         cd \"$wsp_root\" || cd \"$HOME\"\n\
         \x20       fi\n\
         \x20       command \"$wsp_bin\" {cmd_name} \"$@\"\n\
         \x20     else\n\
         \x20       command \"$wsp_bin\" {cmd_name} \"$@\" || return\n\
         \x20       if [[ ! -d \"$PWD\" ]]; then\n\
         \x20         cd \"$wsp_root\" || cd \"$HOME\"\n\
         \x20       fi\n\
         \x20     fi",
    )
}

// ---------- fish ----------

fn generate_fish(w: &mut dyn Write, paths: &Paths) -> Result<()> {
    let bin_str = bin_path()?;
    let wsp_root = paths.workspaces_dir.display().to_string();
    write_fish(w, &bin_str, &wsp_root)
}

fn write_fish(w: &mut dyn Write, bin_str: &str, wsp_root: &str) -> Result<()> {
    let bin_esc = fish_escape(bin_str);
    let root_esc = fish_escape(wsp_root);

    write!(
        w,
        "\
# wsp shell integration \u{2014} source with: wsp setup completion fish | source\n\
\n\
function wsp\n\
    set -l wsp_bin '{bin_esc}'\n\
    set -l wsp_root '{root_esc}'\n\
\n\
    switch $argv[1]\n\
        case new\n\
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
            if test -n \"$args[1]\"\n\
                set -l wsp_dir \"$wsp_root/$args[1]\"\n\
                if string match -q \"$wsp_dir*\" $PWD\n\
                    cd \"$wsp_root\"; or cd $HOME\n\
                end\n\
                command $wsp_bin rm $args\n\
            else\n\
                command $wsp_bin rm $args; or return\n\
                if not test -d $PWD\n\
                    cd \"$wsp_root\"; or cd $HOME\n\
                end\n\
            end\n\
\n\
        case '*'\n\
            command $wsp_bin $argv\n\
    end\n\
end\n\
\n\
COMPLETE=fish '{bin_esc}' | source\n"
    )?;

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
            let out = output(|w| write_posix(w, "/opt/my tools/ws", "/home/user/dev", tc.shell));
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
        let out = output(|w| write_posix(w, "/usr/bin/ws", "/home/user/dev", "zsh"));
        for pattern in &["new)", "cd)", "rm)", "remove)", "*)"] {
            assert!(out.contains(pattern), "missing case pattern: {}", pattern);
        }
    }

    #[test]
    fn test_posix_shell_name_in_header() {
        let bash = output(|w| write_posix(w, "/usr/bin/ws", "/home/user/dev", "bash"));
        assert!(bash.contains("eval \"$(wsp setup completion bash)\""));

        let zsh = output(|w| write_posix(w, "/usr/bin/ws", "/home/user/dev", "zsh"));
        assert!(zsh.contains("eval \"$(wsp setup completion zsh)\""));
    }

    #[test]
    fn test_fish_quotes_bin_path_and_wsp_root() {
        let out = output(|w| write_fish(w, "/opt/my tools/ws", "/home/user/dev"));
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
    fn test_fish_contains_all_cases() {
        let out = output(|w| write_fish(w, "/usr/bin/ws", "/home/user/dev"));
        for pattern in &["case new", "case cd", "case rm remove", "case '*'"] {
            assert!(out.contains(pattern), "missing case pattern: {}", pattern);
        }
    }

    #[test]
    fn test_fish_header() {
        let out = output(|w| write_fish(w, "/usr/bin/ws", "/home/user/dev"));
        assert!(out.contains("wsp setup completion fish | source"));
    }

    #[test]
    fn test_posix_path_with_dollar_sign() {
        let out = output(|w| write_posix(w, "/opt/$weird/ws", "/home/user/dev", "bash"));
        // Single quotes prevent $weird from being expanded
        assert!(out.contains("local wsp_bin='/opt/$weird/ws'"));
        assert!(out.contains("COMPLETE=bash '/opt/$weird/ws'"));
    }

    #[test]
    fn test_posix_path_with_single_quote() {
        let out = output(|w| write_posix(w, "/usr/bin/wsp", "/home/o'brien/dev", "bash"));
        // Single quote in wsp_root must be escaped as '\''
        assert!(
            out.contains(r"local wsp_root='/home/o'\''brien/dev'"),
            "wsp_root single quote must be escaped: {}",
            out
        );
    }

    #[test]
    fn test_posix_bin_with_single_quote() {
        let out = output(|w| write_posix(w, "/opt/it's here/wsp", "/home/user/dev", "bash"));
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
        let out = output(|w| write_fish(w, "/usr/bin/wsp", "/home/o'brien/dev"));
        assert!(
            out.contains(r"set -l wsp_root '/home/o\'brien/dev'"),
            "fish wsp_root single quote must be escaped: {}",
            out
        );
    }

    #[test]
    fn test_fish_bin_with_single_quote() {
        let out = output(|w| write_fish(w, "/opt/it's here/wsp", "/home/user/dev"));
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
}
