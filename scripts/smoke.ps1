#!/usr/bin/env pwsh
# Smoke-test a wsp build end to end.
#
# Runs against a real binary, in a sandboxed data directory, so it never
# touches your registry, mirrors, or workspaces.
#
#   ./scripts/smoke.ps1 -Wsp C:\path\to\wsp.exe -ExpectVersion 0.19.0-rc.1
#   ./scripts/smoke.ps1 -Wsp ./wsp.exe -Offline      # skip network steps
#
# Exits non-zero if any check fails.

param(
    [Parameter(Mandatory = $true)][string]$Wsp,
    [string]$ExpectVersion = "",
    [switch]$Offline
)

$ErrorActionPreference = "Continue"
# -match/-notmatch take regexes. Fixed literals below are written plain; any
# pattern built from a variable goes through [regex]::Escape, because generated
# names and paths carry characters a regex would read as syntax.
$failures = New-Object System.Collections.ArrayList

function Ok($msg) { Write-Host "  ok    $msg" }
function Bad($msg) { Write-Host "  FAIL  $msg" -ForegroundColor Red; [void]$failures.Add($msg) }

# Run wsp, capturing stdout+stderr. Returns the output; sets $global:LastRc.
function Wsp {
    $out = & $Wsp @args 2>&1 | Out-String
    $global:LastRc = $LASTEXITCODE
    return $out
}

# Resolve to an absolute path before any Push-Location changes the CWD.
$Wsp = (Resolve-Path $Wsp -ErrorAction Stop).Path

# --- sandbox -------------------------------------------------------------
# XDG_DATA_HOME is honoured on every platform, so this isolates config,
# mirrors, gc and templates. workspaces-dir is separate and set below.
$sandbox = Join-Path ([System.IO.Path]::GetTempPath()) ("wsp-smoke-" + [guid]::NewGuid().ToString("N").Substring(0, 8))
New-Item -ItemType Directory -Force -Path $sandbox | Out-Null
$oldXdg = $env:XDG_DATA_HOME
$env:XDG_DATA_HOME = Join-Path $sandbox "data"
$workspaces = Join-Path $sandbox "workspaces"
New-Item -ItemType Directory -Force -Path $workspaces | Out-Null

# Point git at a minimal config so that user-level url.insteadOf rewrites
# (e.g. https://github.com/ -> git@github.com:) don't redirect the test
# clones to SSH, where the agent may not be available.
#
# gpgsign is off because the network section commits: a signing key the runner
# does not have would fail the commit rather than the check under test.
$gitConfig = Join-Path $sandbox "gitconfig"
@"
[user]
    email = smoke@test.local
    name = Smoke Test
[commit]
    gpgsign = false
"@ | Set-Content -Path $gitConfig -Encoding utf8
$oldGitConfigGlobal = $env:GIT_CONFIG_GLOBAL
$env:GIT_CONFIG_GLOBAL = $gitConfig
# GIT_CONFIG_GLOBAL does not shadow the system config, so a system-level
# core.hooksPath or url.insteadOf would still reach the fixture commits.
$oldGitConfigNoSystem = $env:GIT_CONFIG_NOSYSTEM
$env:GIT_CONFIG_NOSYSTEM = '1'

# Commands detect the workspace from the working directory, which
# XDG_DATA_HOME does not isolate. Run from inside a real workspace and doctor
# will happily inspect *that* one. Move somewhere neutral.
Push-Location $sandbox

Write-Host "sandbox: $sandbox"
Write-Host ""

try {
    # --- offline checks --------------------------------------------------
    Write-Host "offline"

    $v = (Wsp --version).Trim()
    if ($global:LastRc -ne 0) { Bad "--version exited $($global:LastRc)" }
    elseif ($ExpectVersion -and ($v -notmatch [regex]::Escape($ExpectVersion))) {
        Bad "--version says '$v', expected to contain '$ExpectVersion'"
    } else { Ok "--version: $v" }

    $h = Wsp --help
    if ($global:LastRc -ne 0) { Bad "--help exited $($global:LastRc)" } else { Ok "--help" }

    # The headline feature of this release: PowerShell integration must emit
    # a usable wrapper, not an empty file or an error.
    $c = Wsp completion powershell
    if ($global:LastRc -ne 0) { Bad "completion powershell exited $($global:LastRc)" }
    elseif ($c -notmatch "function wsp") { Bad "completion powershell has no 'function wsp'" }
    else { Ok "completion powershell" }

    # It must also parse as PowerShell, which is the part a string check misses.
    $errs = $null
    [void][System.Management.Automation.Language.Parser]::ParseInput($c, [ref]$null, [ref]$errs)
    if ($errs -and $errs.Count -gt 0) { Bad "completion output does not parse: $($errs[0].Message)" }
    else { Ok "completion output parses as PowerShell" }

    # --global is required: both are global-only keys. branch-prefix is set so
    # doctor has nothing left to warn about, which lets the check below assert
    # a clean bill of health rather than merely "it ran".
    Wsp config set workspaces-dir $workspaces --global | Out-Null
    if ($global:LastRc -ne 0) { Bad "config set workspaces-dir exited $($global:LastRc)" } else { Ok "config set workspaces-dir" }
    Wsp config set branch-prefix smoke --global | Out-Null
    if ($global:LastRc -ne 0) { Bad "config set branch-prefix exited $($global:LastRc)" }

    # doctor exits non-zero on warnings, not just errors. On failure, report what
    # it objected to: "doctor reported problems" alone means instrumenting this
    # script to find out, and the answer is usually a check above having left
    # state behind.
    $dout = (Wsp doctor) -notmatch '✓'
    if ($global:LastRc -ne 0) {
        Bad "doctor in a fresh sandbox: $($dout -join '|')"
    } else { Ok "doctor (clean)" }

    Wsp ls | Out-Null
    if ($global:LastRc -ne 0) { Bad "ls exited $($global:LastRc)" } else { Ok "ls" }

    # --size measures disk usage. For a removed workspace the number comes from
    # the gc metadata, written when it was removed, so it costs a metadata read
    # rather than a walk. Asserted by removing the payload and checking it holds.
    $sizews = "smoke-du-$((Get-Date).ToString('HHmmss'))"
    Wsp new $sizews --empty | Out-Null
    # Anchored on the header row and case-sensitive (`-cmatch`): `-match` ignores
    # case, so a workspace named "...size..." satisfied a bare search.
    if (((Wsp ls --size) -join "`n") -cnotmatch '(?m)^NAME.*SIZE') { Bad "ls --size printed no SIZE column" }
    else { Ok "ls --size adds a size column" }
    if (((Wsp ls) -join "`n") -cmatch '(?m)^NAME.*SIZE') { Bad "ls without --size printed a SIZE column" }
    else { Ok "ls without --size leaves the table alone" }

    Wsp rm $sizews --force | Out-Null
    $before = ((Wsp ls --removed --size --json) -join "") -replace '.*"size_bytes":\s*(\d+).*', '$1'
    Get-ChildItem -Path (Join-Path $env:XDG_DATA_HOME "wsp\gc") -Recurse -File |
        Where-Object { $_.Name -ne '.wsp-gc.yaml' } | Remove-Item -Force -ErrorAction SilentlyContinue
    $after = ((Wsp ls --removed --size --json) -join "") -replace '.*"size_bytes":\s*(\d+).*', '$1'
    if ($before -and $before -eq $after) {
        Ok "ls --removed --size reads the size recorded at removal"
    } else {
        Bad "removed size changed when the files went ($before -> $after), so it was recomputed"
    }
    Wsp recover $sizews | Out-Null
    Wsp rm $sizews --force | Out-Null

    # Non-interactive setup prints the manual guide instead of prompting, and
    # omits the branch-prefix line when one is already configured -- which the
    # check above did. Asserting that absence makes this a statement about real
    # config rather than "the command ran". The empty-string pipe forces a
    # non-TTY stdin, so it cannot prompt and cannot hang a run from a terminal.
    $setupOut = ('' | & $Wsp setup 2>&1) -join "`n"
    if (($setupOut -match [regex]::Escape("requires an interactive terminal")) -and
        ($setupOut -notmatch [regex]::Escape("config set branch-prefix"))) {
        Ok "setup declines non-interactively and reflects config"
    } else { Bad "setup did not print the expected non-interactive guide: $setupOut" }

    # Removal and recovery, end to end. Worth smoking rather than trusting to
    # unit tests: this is the one path where a bug loses a user's work, and an
    # --empty workspace exercises all of it without network. On Windows it also
    # covers the rename/delete-while-cwd restriction, which no unit test can.
    $gcws = "smoke-gc-$((Get-Date).ToString('HHmmss'))"
    Wsp new $gcws --empty | Out-Null
    if ($global:LastRc -ne 0) { Bad "new --empty exited $($global:LastRc)" } else { Ok "new --empty" }

    Wsp rm $gcws --force | Out-Null
    if ($global:LastRc -ne 0) { Bad "rm --force exited $($global:LastRc)" } else { Ok "rm --force" }

    $rem = Wsp ls --removed
    if ($rem -notmatch [regex]::Escape($gcws)) { Bad "ls --removed does not list $gcws" }
    else { Ok "ls --removed shows the removed workspace" }

    $act = Wsp ls
    if ($act -notmatch "recoverable") {
        Bad "ls does not mention that something is recoverable"
    } else { Ok "ls footer points at the removed workspace" }

    # Bare `wsp` runs the same listing through a different path, which used to
    # overwrite the footer with navigation advice.
    $bare = Wsp
    if ($bare -notmatch "recoverable") { Bad "bare wsp dropped the recoverable footer" }
    else { Ok "bare wsp keeps the recoverable footer" }

    # Bare `recover` must refuse rather than list: an argumentless mutation is
    # what made the command's name mean two things. Non-zero exit is the contract.
    Wsp recover | Out-Null
    if ($global:LastRc -eq 0) { Bad "bare recover succeeded; it must ask for a workspace name" }
    else { Ok "bare recover refuses without a name" }

    Wsp recover $gcws | Out-Null
    if ($global:LastRc -ne 0) { Bad "recover exited $($global:LastRc)" } else { Ok "recover <name>" }

    $act = Wsp ls
    if ($act -notmatch [regex]::Escape($gcws)) { Bad "$gcws missing from ls after recover" }
    else { Ok "recovered workspace is back in ls" }

    Wsp rm $gcws --force | Out-Null

    # Guides are compiled into the binary, so a build that lost them still
    # passes every unit test. Assert on the body, not just the exit code.
    $g = Wsp help gc
    if ($g -notmatch "retention-days") { Bad "help gc did not print the gc guide" }
    else { Ok "help gc prints the guide" }

    # The only non-interactive path through init. The sample is what a user
    # pastes into a repo, so it has to contain the key it is a sample of.
    $sample = Wsp init --print-sample
    if ($sample -notmatch "setup_commands") { Bad "init --print-sample printed no setup_commands key" }
    else { Ok "init --print-sample" }

    # Templates round-trip entirely offline: an unregistered URL is stored
    # verbatim and never cloned.
    $tmpl = "smoke-tmpl-$((Get-Date).ToString('HHmmss'))"
    Wsp template new $tmpl 'git@test.local:user/repo.git' | Out-Null
    if ((Wsp template ls) -notmatch [regex]::Escape($tmpl)) { Bad "$tmpl missing from template ls" }
    else { Ok "template new shows up in template ls" }

    # A second template, so removing the first asserts a presence as well as an
    # absence. Absence alone is satisfied by a binary that does nothing at all.
    Wsp template new "$tmpl-keep" 'git@test.local:user/other.git' | Out-Null
    Wsp template rm $tmpl | Out-Null
    $remaining = (Wsp template ls) -join "`n"
    if (($remaining -match [regex]::Escape("$tmpl-keep")) -and
        ($remaining -notmatch [regex]::Escape("$tmpl "))) {
        Ok "template rm removes one and keeps the other"
    } else { Bad "template rm left the wrong set: $remaining" }
    # Leave nothing behind: its repo is not in the registry, so `wsp doctor` in
    # the network half would warn and exit non-zero on a template this check
    # created.
    Wsp template rm "$tmpl-keep" | Out-Null

    # One local workspace covers the commands that need a workspace but no
    # network. Left behind for the sandbox teardown to collect.
    $lws = "smoke-local-$((Get-Date).ToString('HHmmss'))"
    Wsp new $lws --empty | Out-Null

    # -- collects the trailing tokens, so a description needs no shell quoting.
    # It has to reach the listing, which is the only place a user ever sees it.
    # Quoted so PowerShell passes it along instead of eating it as its own
    # end-of-parameters marker.
    Wsp describe $lws '--' described by smoke | Out-Null
    if ((Wsp ls) -notmatch "described by smoke") {
        Bad "the description set by describe is missing from ls"
    } else { Ok "describe reaches the ls listing" }

    # Without shell integration cd prints the destination instead of moving.
    # Read stdout alone -- the "integration not active" hints go to stderr, and
    # the Wsp helper merges the two.
    $dest = (& $Wsp cd $lws 2>$null | Out-String).Trim()
    # Compare against the workspace asked for: "is a workspace" would accept
    # the wrong one. Windows can hand back an 8.3 short path, so resolve both
    # sides before comparing rather than trusting the spelling.
    $want = (Resolve-Path (Join-Path $workspaces $lws) -ErrorAction SilentlyContinue).Path
    $got = if ($dest) { (Resolve-Path $dest -ErrorAction SilentlyContinue).Path } else { $null }
    if (-not $got -or $got -ne $want) {
        Bad "cd printed '$dest', expected '$want'"
    } else { Ok "cd prints the workspace path" }

    # rename moves the directory on disk, which is the half that only a real
    # filesystem can check -- and the half Windows can refuse outright.
    Wsp rename $lws "${lws}-renamed" | Out-Null
    if ((Test-Path (Join-Path $workspaces "${lws}-renamed")) -and
        -not (Test-Path (Join-Path $workspaces $lws))) {
        Ok "rename moved the workspace directory"
    } else { Bad "rename did not move $lws to ${lws}-renamed on disk" }

    if ($ExpectVersion) {
        $w = Wsp whatsnew
        if ($w -notmatch [regex]::Escape($ExpectVersion)) {
            Bad "whatsnew does not mention $ExpectVersion (are the release notes in the build?)"
        } else { Ok "whatsnew shows the expected version" }
    }

    # --- network checks --------------------------------------------------
    # wsp cannot register a local path (the registry requires a host/user/repo
    # identity and clones over the network), so these need connectivity.
    if ($Offline) {
        Write-Host ""
        Write-Host "network: skipped (-Offline)"
    } else {
        Write-Host ""
        Write-Host "network"

        $repo1 = "https://github.com/octocat/Hello-World.git"
        $repo2 = "https://github.com/octocat/Spoon-Knife.git"

        Wsp registry add $repo1 | Out-Null
        if ($global:LastRc -ne 0) { Bad "registry add exited $($global:LastRc)" } else { Ok "registry add" }

        $ws = "smoke-$((Get-Date).ToString('HHmmss'))"
        Wsp new $ws github.com/octocat/Hello-World | Out-Null
        if ($global:LastRc -ne 0) { Bad "new exited $($global:LastRc)" } else { Ok "new $ws" }

        $wsDir = Join-Path $workspaces $ws
        if (-not (Test-Path (Join-Path $wsDir "Hello-World"))) { Bad "clone directory missing in $wsDir" }
        else { Ok "repo cloned into the workspace" }

        Push-Location $wsDir
        try {
            Wsp st | Out-Null
            if ($global:LastRc -ne 0) { Bad "st exited $($global:LastRc)" } else { Ok "st" }

            # Exercises the fetch-before-clone fix on the add path.
            Wsp registry add $repo2 | Out-Null
            Wsp repo add github.com/octocat/Spoon-Knife | Out-Null
            if ($global:LastRc -ne 0) { Bad "repo add exited $($global:LastRc)" }
            elseif (-not (Test-Path (Join-Path $wsDir "Spoon-Knife"))) { Bad "repo add did not clone Spoon-Knife" }
            else { Ok "repo add" }

            $d2 = (Wsp doctor) -notmatch '✓'
            if ($global:LastRc -ne 0) {
                Bad "doctor in a real workspace: $($d2 -join '|')"
            } else { Ok "doctor in a real workspace" }

            # One unpushed commit is the fixture for the rest of this section:
            # diff bases on the merge-base with upstream, log lists what is
            # unpushed, and rm must refuse to throw it away. Needs a real clone
            # with a real upstream, which is why it lives here and not in the
            # offline half.
            $repoDir = Join-Path $wsDir "Hello-World"
            Set-Content -Path (Join-Path $repoDir "smoke.txt") -Value "smoke"
            & git -C $repoDir add smoke.txt 2>&1 | Out-Null
            $addRc = $LASTEXITCODE
            & git -C $repoDir commit --no-verify -m "smoke fixture commit" 2>&1 | Out-Null
            if ($addRc -ne 0 -or $LASTEXITCODE -ne 0) {
                Bad "fixture commit failed; the checks below will fail for the wrong reason"
            } else { Ok "fixture commit" }

            if ((Wsp diff $ws) -notmatch [regex]::Escape("smoke.txt")) {
                Bad "diff does not mention smoke.txt"
            } else { Ok "diff shows the change" }

            if ((Wsp log $ws) -notmatch [regex]::Escape("smoke fixture commit")) {
                Bad "log does not mention the commit that is ahead of upstream"
            } else { Ok "log shows the unpushed commit" }

            # Proves the command ran inside each clone, not just that exec
            # exited 0. '--' is quoted so PowerShell passes it along instead of
            # eating it as its own end-of-parameters marker.
            if ((Wsp exec $ws '--' git rev-parse --abbrev-ref HEAD) -notmatch [regex]::Escape("smoke/$ws")) {
                Bad "exec did not report the workspace branch from the clones"
            } else { Ok "exec runs git in each clone" }

            # Rewind a branch behind upstream so sync has something to do. A
            # branch that is ahead takes sync's no-op path and reports "already
            # up to date" without moving anything, so asserting that string
            # proves only that the fetch did not fail. Rewind Spoon-Knife, not
            # Hello-World: the latter carries the commit the rm check needs.
            & git -C (Join-Path $wsDir "Spoon-Knife") reset --hard HEAD~1 2>&1 | Out-Null
            $sy = (Wsp sync $ws) -join "`n"
            # "fast-forwarded", not "rebase": the latter is the ACTION column,
            # printed whether or not anything moved.
            if (($sy -notmatch "fast-forwarded") -or ($sy -match "fetch failed")) {
                Bad "sync did not move a behind branch: $sy"
            } else { Ok "sync brings a behind branch up to upstream" }
        } finally { Pop-Location }

        # An unmerged branch must block a plain rm. That guard is the difference
        # between a recoverable mistake and a lost afternoon, and --force is the
        # documented way past it. Assert the reason, not just the exit status:
        # any unrelated failure of rm would satisfy "did not succeed".
        $rmOut = (Wsp rm $ws --yes) -join "`n"
        if ($global:LastRc -eq 0) {
            Bad "rm removed $ws despite unsaved work; --force should be required"
        } elseif ($rmOut -match [regex]::Escape("unsaved work")) {
            Ok "rm refuses a workspace with unsaved work"
        } else { Bad "rm failed but not because of unsaved work: $rmOut" }

        Wsp rm $ws --force | Out-Null
        if ($global:LastRc -ne 0) { Bad "rm --force exited $($global:LastRc)" } else { Ok "rm --force $ws" }
    }
} catch {
    # Without this, a *terminating* error -- a cmdlet refusing an empty
    # argument, a Push-Location into a directory a failed clone never made --
    # unwinds straight past every remaining check to `finally`, and the script
    # then prints "all checks passed" and exits 0. This gate blocks release
    # tags, so silence is the one failure it must not have.
    Bad "unexpected error: $_"
} finally {
    # --- cleanup ---------------------------------------------------------
    Pop-Location -ErrorAction SilentlyContinue
    if ($null -eq $oldXdg) { Remove-Item Env:\XDG_DATA_HOME -ErrorAction SilentlyContinue }
    else { $env:XDG_DATA_HOME = $oldXdg }
    if ($null -eq $oldGitConfigGlobal) { Remove-Item Env:\GIT_CONFIG_GLOBAL -ErrorAction SilentlyContinue }
    else { $env:GIT_CONFIG_GLOBAL = $oldGitConfigGlobal }
    if ($null -eq $oldGitConfigNoSystem) { Remove-Item Env:\GIT_CONFIG_NOSYSTEM -ErrorAction SilentlyContinue }
    else { $env:GIT_CONFIG_NOSYSTEM = $oldGitConfigNoSystem }
    Remove-Item -Recurse -Force $sandbox -ErrorAction SilentlyContinue
}

Write-Host ""
if ($failures.Count -gt 0) {
    Write-Host "$($failures.Count) check(s) failed" -ForegroundColor Red
    exit 1
}
Write-Host "all checks passed"
exit 0
