#!/usr/bin/env pwsh
# Verify Windows workspace confinement against a real release binary.
#
# The child runs as a unique local account. Only the fixture workspace and
# copied binary are granted to that account; global, sibling, and unlisted
# canaries remain private to the runner. This is an authority-boundary gate,
# not an exact filesystem allowlist. Missing account-management support fails
# this mandatory CI check.
param([Parameter(Mandatory = $true)][string]$Wsp)

$ErrorActionPreference = 'Stop'
$Wsp = (Resolve-Path $Wsp -ErrorAction Stop).Path
if (-not (Test-Path $Wsp -PathType Leaf)) { throw "not executable: $Wsp" }
if (-not (Get-Command New-LocalUser -ErrorAction SilentlyContinue)) { throw 'New-LocalUser is required for the Windows confinement gate' }
if (-not (Get-Command Remove-LocalUser -ErrorAction SilentlyContinue)) { throw 'Remove-LocalUser is required for the Windows confinement gate' }

$root = Join-Path ([IO.Path]::GetTempPath()) ("wsp-posix-" + [guid]::NewGuid().ToString('N'))
$workspace = Join-Path $root 'workspace'
$global = Join-Path $root 'global'
$sibling = Join-Path $root 'sibling'
$bin = Join-Path $root 'bin'
$outsideGlobal = Join-Path ([IO.Path]::GetTempPath()) ("wsp-posix-global-" + [guid]::NewGuid().ToString('N'))
$user = 'wspci' + [guid]::NewGuid().ToString('N').Substring(0, 15)
$passwordText = [guid]::NewGuid().ToString('N') + '!aA1'
$password = ConvertTo-SecureString $passwordText -AsPlainText -Force
$credential = [pscredential]::new("$env:COMPUTERNAME\$user", $password)
$createdUser = $false

function Invoke-Icacls([string]$Path, [string[]]$Arguments) {
    & icacls.exe $Path @Arguments | Out-Host
    if ($LASTEXITCODE -ne 0) { throw "icacls failed for $Path ($LASTEXITCODE)" }
}

try {
    New-LocalUser -Name $user -Password $password -PasswordNeverExpires -AccountNeverExpires | Out-Null
    $createdUser = $true
    if (-not (Get-LocalUser -Name $user -ErrorAction Stop).Enabled) { throw "created confinement account is disabled: $user" }

    New-Item -ItemType Directory -Force -Path $workspace, $global, $sibling, $bin, (Join-Path $workspace 'tmp'), (Join-Path $outsideGlobal 'wsp') | Out-Null
    $wspCopy = Join-Path $bin 'wsp.exe'
    $child = Join-Path $workspace 'child.cmd'
    $result = Join-Path $workspace 'result.json'

    # The root permits traversal only. The runner and system retain every
    # protected path; the confined account receives workspace mutation and
    # binary read/execute access only.
    Invoke-Icacls $root @('/inheritance:r', '/grant:r', "$user`:(X)", 'SYSTEM:(OI)(CI)F', 'Administrators:(OI)(CI)F')
    Invoke-Icacls $workspace @('/inheritance:r', '/grant:r', "$user`:(OI)(CI)M", 'SYSTEM:(OI)(CI)F', 'Administrators:(OI)(CI)F')
    Invoke-Icacls $bin @('/inheritance:r', '/grant:r', "$user`:(OI)(CI)RX", 'SYSTEM:(OI)(CI)F', 'Administrators:(OI)(CI)F')
    foreach ($denied in @($global, $sibling, $outsideGlobal)) {
        Invoke-Icacls $denied @('/inheritance:r', '/grant:r', 'SYSTEM:(OI)(CI)F', 'Administrators:(OI)(CI)F')
    }

    Copy-Item -LiteralPath $Wsp -Destination $wspCopy
    Invoke-Icacls $wspCopy @('/grant:r', "$user`:(RX)")
    @"
name: mounted
branch: main
repos: {}
created: 2026-09-18T00:00:00Z
"@ | Set-Content -LiteralPath (Join-Path $workspace '.wsp.yaml') -Encoding utf8NoBOM
    'global sentinel' | Set-Content -LiteralPath (Join-Path $global 'sentinel') -Encoding ascii
    'sibling sentinel' | Set-Content -LiteralPath (Join-Path $sibling 'sentinel') -Encoding ascii
    'outside sentinel' | Set-Content -LiteralPath (Join-Path $outsideGlobal 'sentinel') -Encoding ascii
    'not: [valid' | Set-Content -LiteralPath (Join-Path $outsideGlobal 'wsp/config.yaml') -Encoding utf8NoBOM

    @"
@echo off
setlocal EnableExtensions
set "XDG_DATA_HOME=$outsideGlobal"
set "HOME=$global\home"
set "USERPROFILE=$global\home"
set "TEMP=$workspace\tmp"
set "TMP=$workspace\tmp"
type "$global\sentinel" >nul 2>&1 && exit /b 10
echo blocked > "$global\must-not-create" 2>nul && exit /b 11
type "$sibling\sentinel" >nul 2>&1 && exit /b 12
echo blocked > "$sibling\must-not-create" 2>nul && exit /b 13
type "$outsideGlobal\sentinel" >nul 2>&1 && exit /b 14
echo blocked > "$outsideGlobal\must-not-create" 2>nul && exit /b 15
cd /d "$workspace" || exit /b 16
"$wspCopy" --json describe "Windows confined workspace" > "$result" || exit /b 17
"$wspCopy" --json st > "$workspace\status.json" || exit /b 18
"$wspCopy" --json repo ls > "$workspace\repos.json" || exit /b 19
"@ | Set-Content -LiteralPath $child -Encoding ascii
    Invoke-Icacls $child @('/grant:r', "$user`:(RX)")

    $process = Start-Process -FilePath $env:ComSpec -ArgumentList @('/d', '/c', $child) -WorkingDirectory $workspace -Credential $credential -Wait -PassThru
    if ($process.ExitCode -ne 0) { throw "confined child failed with exit code $($process.ExitCode)" }

    if ((Get-Content -LiteralPath (Join-Path $global 'sentinel') -Raw).Trim() -ne 'global sentinel') { throw 'global sentinel changed' }
    if ((Get-Content -LiteralPath (Join-Path $sibling 'sentinel') -Raw).Trim() -ne 'sibling sentinel') { throw 'sibling sentinel changed' }
    if ((Get-Content -LiteralPath (Join-Path $outsideGlobal 'sentinel') -Raw).Trim() -ne 'outside sentinel') { throw 'outside sentinel changed' }
    if (Test-Path -LiteralPath (Join-Path $global 'must-not-create')) { throw 'confined child created a global file' }
    if (Test-Path -LiteralPath (Join-Path $sibling 'must-not-create')) { throw 'confined child created a sibling file' }
    if (Test-Path -LiteralPath (Join-Path $outsideGlobal 'must-not-create')) { throw 'confined child created an outside-global file' }
    if ((Get-Content -LiteralPath (Join-Path $workspace '.wsp.yaml') -Raw) -notmatch [regex]::Escape('Windows confined workspace')) { throw 'wsp did not update the workspace' }
    Write-Host 'Windows distinct-principal confinement passed'
}
finally {
    if ($createdUser) { Remove-LocalUser -Name $user -ErrorAction SilentlyContinue }
    if (Test-Path -LiteralPath $root) { Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue }
    if (Test-Path -LiteralPath $outsideGlobal) { Remove-Item -LiteralPath $outsideGlobal -Recurse -Force -ErrorAction SilentlyContinue }
}
