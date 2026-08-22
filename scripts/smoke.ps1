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
$gitConfig = Join-Path $sandbox "gitconfig"
@"
[user]
    email = smoke@test.local
    name = Smoke Test
"@ | Set-Content -Path $gitConfig -Encoding utf8
$oldGitConfigGlobal = $env:GIT_CONFIG_GLOBAL
$env:GIT_CONFIG_GLOBAL = $gitConfig

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

    # doctor exits non-zero on warnings, not just errors.
    Wsp doctor | Out-Null
    if ($global:LastRc -ne 0) { Bad "doctor reported problems in a fresh sandbox" } else { Ok "doctor (clean)" }

    Wsp ls | Out-Null
    if ($global:LastRc -ne 0) { Bad "ls exited $($global:LastRc)" } else { Ok "ls" }

    if ($ExpectVersion) {
        $w = Wsp whatsnew
        if ($w -notmatch [regex]::Escape($ExpectVersion)) {
            Bad "whatsnew does not mention $ExpectVersion (are the release notes in the build?)"
        } else { Ok "whatsnew shows $ExpectVersion" }
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

            $d2 = Wsp doctor
            if ($global:LastRc -ne 0) { Bad "doctor (in workspace) exited $($global:LastRc)" } else { Ok "doctor in a real workspace" }
        } finally { Pop-Location }

        Wsp rm $ws --yes | Out-Null
        if ($global:LastRc -ne 0) { Bad "rm exited $($global:LastRc)" } else { Ok "rm $ws" }
    }
} finally {
    # --- cleanup ---------------------------------------------------------
    Pop-Location -ErrorAction SilentlyContinue
    if ($null -eq $oldXdg) { Remove-Item Env:\XDG_DATA_HOME -ErrorAction SilentlyContinue }
    else { $env:XDG_DATA_HOME = $oldXdg }
    if ($null -eq $oldGitConfigGlobal) { Remove-Item Env:\GIT_CONFIG_GLOBAL -ErrorAction SilentlyContinue }
    else { $env:GIT_CONFIG_GLOBAL = $oldGitConfigGlobal }
    Remove-Item -Recurse -Force $sandbox -ErrorAction SilentlyContinue
}

Write-Host ""
if ($failures.Count -gt 0) {
    Write-Host "$($failures.Count) check(s) failed" -ForegroundColor Red
    exit 1
}
Write-Host "all checks passed"
exit 0
