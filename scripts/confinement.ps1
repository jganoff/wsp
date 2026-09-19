#!/usr/bin/env pwsh
# Verify Windows-enforced workspace confinement against a real release binary.
#
# The child runs in a zero-capability Less Privileged AppContainer (LPAC), not
# merely a different local account. The package SID has access only to a copied
# binary, a fixture workspace, and its trusted Windows runtime. Every failure
# to create or verify that boundary fails this CI gate.
param([Parameter(Mandatory = $true)][string]$Wsp)

$ErrorActionPreference = 'Stop'
$Wsp = (Resolve-Path $Wsp -ErrorAction Stop).Path
if (-not (Test-Path $Wsp -PathType Leaf)) { throw "not executable: $Wsp" }

Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Text;

public static class Lpac {
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    struct StartupInfo {
        public int cb;
        public IntPtr reserved, desktop, title;
        public int x, y, xSize, ySize, xCount, yCount, fill, flags;
        public short show, reserved2;
        public IntPtr reservedPtr, input, output, error;
    }
    [StructLayout(LayoutKind.Sequential)]
    struct StartupInfoEx { public StartupInfo startup; public IntPtr attributes; }
    [StructLayout(LayoutKind.Sequential)]
    struct ProcessInformation { public IntPtr process, thread; public int processId, threadId; }
    [StructLayout(LayoutKind.Sequential)]
    struct SecurityCapabilities { public IntPtr sid, capabilities; public int count, reserved; }

    [DllImport("userenv.dll", CharSet = CharSet.Unicode)]
    static extern int DeriveAppContainerSidFromAppContainerName(string name, out IntPtr sid);
    [DllImport("advapi32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern bool ConvertSidToStringSidW(IntPtr sid, out IntPtr text);
    [DllImport("advapi32.dll")] static extern IntPtr FreeSid(IntPtr sid);
    [DllImport("kernel32.dll")] static extern IntPtr LocalFree(IntPtr value);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool InitializeProcThreadAttributeList(IntPtr list, int count, int flags, ref IntPtr size);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool UpdateProcThreadAttribute(IntPtr list, uint flags, IntPtr attribute, IntPtr value, IntPtr size, IntPtr previous, IntPtr returned);
    [DllImport("kernel32.dll")] static extern void DeleteProcThreadAttributeList(IntPtr list);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern bool CreateProcessW(string app, StringBuilder command, IntPtr processSecurity, IntPtr threadSecurity, bool inherit, uint flags, IntPtr environment, string cwd, ref StartupInfoEx startup, out ProcessInformation process);
    [DllImport("advapi32.dll", SetLastError = true)] static extern bool OpenProcessToken(IntPtr process, uint access, out IntPtr token);
    [DllImport("advapi32.dll", SetLastError = true)] static extern bool GetTokenInformation(IntPtr token, int kind, out int value, int size, out int returned);
    [DllImport("kernel32.dll", SetLastError = true)] static extern uint ResumeThread(IntPtr thread);
    [DllImport("kernel32.dll", SetLastError = true)] static extern uint WaitForSingleObject(IntPtr handle, uint milliseconds);
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool GetExitCodeProcess(IntPtr process, out uint exit);
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool TerminateProcess(IntPtr process, uint exit);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr handle);

    static void Check(bool value) { if (!value) throw new Win32Exception(Marshal.GetLastWin32Error()); }

    public static string Sid(string name) {
        IntPtr sid = IntPtr.Zero, text = IntPtr.Zero;
        try {
            Marshal.ThrowExceptionForHR(DeriveAppContainerSidFromAppContainerName(name, out sid));
            Check(ConvertSidToStringSidW(sid, out text));
            return Marshal.PtrToStringUni(text);
        } finally {
            if (text != IntPtr.Zero) LocalFree(text);
            if (sid != IntPtr.Zero) FreeSid(sid);
        }
    }

    public static uint Run(string name, string app, string arguments, string cwd, string[] environment) {
        const uint CreateSuspended = 0x00000004;
        const uint CreateUnicodeEnvironment = 0x00000400;
        const uint ExtendedStartupInfoPresent = 0x00080000;
        const uint CreateNoWindow = 0x08000000;
        IntPtr sid = IntPtr.Zero, list = IntPtr.Zero, capabilities = IntPtr.Zero, policy = IntPtr.Zero, environmentBlock = IntPtr.Zero, token = IntPtr.Zero;
        ProcessInformation process = new ProcessInformation();
        bool initialized = false, exited = false;
        try {
            Marshal.ThrowExceptionForHR(DeriveAppContainerSidFromAppContainerName(name, out sid));
            IntPtr size = IntPtr.Zero;
            InitializeProcThreadAttributeList(IntPtr.Zero, 2, 0, ref size);
            if (size == IntPtr.Zero) throw new Win32Exception(Marshal.GetLastWin32Error());
            list = Marshal.AllocHGlobal(size);
            Check(InitializeProcThreadAttributeList(list, 2, 0, ref size));
            initialized = true;
            capabilities = Marshal.AllocHGlobal(Marshal.SizeOf(typeof(SecurityCapabilities)));
            Marshal.StructureToPtr(new SecurityCapabilities { sid = sid }, capabilities, false);
            Check(UpdateProcThreadAttribute(list, 0, (IntPtr)0x00020009, capabilities, (IntPtr)Marshal.SizeOf(typeof(SecurityCapabilities)), IntPtr.Zero, IntPtr.Zero));
            // Disable broad ALL APPLICATION PACKAGES access, making this LPAC.
            policy = Marshal.AllocHGlobal(4);
            Marshal.WriteInt32(policy, 1);
            Check(UpdateProcThreadAttribute(list, 0, (IntPtr)0x0002000F, policy, (IntPtr)4, IntPtr.Zero, IntPtr.Zero));
            Array.Sort(environment, StringComparer.OrdinalIgnoreCase);
            environmentBlock = Marshal.StringToHGlobalUni(String.Join("\0", environment) + "\0\0");
            StartupInfoEx startup = new StartupInfoEx();
            startup.startup.cb = Marshal.SizeOf(typeof(StartupInfoEx));
            startup.attributes = list;
            Check(CreateProcessW(app, new StringBuilder("\"" + app + "\" " + arguments), IntPtr.Zero, IntPtr.Zero, false, CreateSuspended | CreateUnicodeEnvironment | ExtendedStartupInfoPresent | CreateNoWindow, environmentBlock, cwd, ref startup, out process));
            Check(OpenProcessToken(process.process, 8, out token));
            int value, returned;
            Check(GetTokenInformation(token, 29, out value, 4, out returned));
            if (value != 1) throw new InvalidOperationException("child is not an AppContainer");
            Check(GetTokenInformation(token, 46, out value, 4, out returned));
            if (value != 1) throw new InvalidOperationException("child is not an LPAC");
            if (ResumeThread(process.thread) == UInt32.MaxValue) throw new Win32Exception(Marshal.GetLastWin32Error());
            uint wait = WaitForSingleObject(process.process, 120000);
            if (wait != 0) throw new InvalidOperationException("LPAC child wait failed or timed out: " + wait);
            uint exit;
            Check(GetExitCodeProcess(process.process, out exit));
            exited = true;
            return exit;
        } finally {
            if (process.process != IntPtr.Zero && !exited) TerminateProcess(process.process, 99);
            if (token != IntPtr.Zero) CloseHandle(token);
            if (process.thread != IntPtr.Zero) CloseHandle(process.thread);
            if (process.process != IntPtr.Zero) CloseHandle(process.process);
            if (initialized) DeleteProcThreadAttributeList(list);
            if (list != IntPtr.Zero) Marshal.FreeHGlobal(list);
            if (capabilities != IntPtr.Zero) Marshal.FreeHGlobal(capabilities);
            if (policy != IntPtr.Zero) Marshal.FreeHGlobal(policy);
            if (environmentBlock != IntPtr.Zero) Marshal.FreeHGlobal(environmentBlock);
            if (sid != IntPtr.Zero) FreeSid(sid);
        }
    }
}
'@

$root = Join-Path ([IO.Path]::GetTempPath()) ("wsp-lpac-" + [guid]::NewGuid().ToString('N'))
$workspace = Join-Path $root 'workspace'
$global = Join-Path $root 'global'
$sibling = Join-Path $root 'sibling'
$bin = Join-Path $root 'bin'
$childCmd = Join-Path $bin 'child.cmd'
$outsideGlobal = Join-Path ([IO.Path]::GetTempPath()) ("wsp-lpac-global-" + [guid]::NewGuid().ToString('N'))
$containerName = 'wsp.lpac.' + [guid]::NewGuid().ToString('N')

function Invoke-Icacls([string]$Path, [string[]]$Arguments) {
    & icacls.exe $Path @Arguments | Out-Host
    if ($LASTEXITCODE -ne 0) { throw "icacls failed for $Path ($LASTEXITCODE)" }
}

try {
    New-Item -ItemType Directory -Force -Path $workspace, $global, $sibling, $bin, (Join-Path $workspace 'tmp'), (Join-Path $outsideGlobal 'wsp') | Out-Null
    $sid = [Lpac]::Sid($containerName)
    $trustee = "*$sid"
    $administrators = 'Administrators:(OI)(CI)F'
    $system = 'SYSTEM:(OI)(CI)F'

    # Root permits only traversal. The package receives an explicit ACL for the
    # executable and the mounted workspace, never an inherited root grant.
    Invoke-Icacls $root @('/inheritance:r', '/grant:r', "$trustee`:(X)", $system, $administrators)
    Invoke-Icacls $workspace @('/inheritance:r', '/grant:r', "$trustee`:(OI)(CI)M", $system, $administrators)
    Invoke-Icacls $workspace @('/setintegritylevel', '(OI)(CI)L')
    Invoke-Icacls $bin @('/inheritance:r', '/grant:r', "$trustee`:(OI)(CI)RX", $system, $administrators)
    foreach ($denied in @($global, $sibling)) {
        Invoke-Icacls $denied @('/inheritance:r', '/grant:r', $system, $administrators)
    }

    $wspCopy = Join-Path $bin 'wsp.exe'
    Copy-Item -LiteralPath $Wsp -Destination $wspCopy
    @"
name: mounted
branch: main
repos: {}
created: 2026-09-18T00:00:00Z
"@ | Set-Content -LiteralPath (Join-Path $workspace '.wsp.yaml') -Encoding utf8NoBOM
    'global sentinel' | Set-Content -LiteralPath (Join-Path $global 'sentinel') -Encoding ascii
    'sibling sentinel' | Set-Content -LiteralPath (Join-Path $sibling 'sentinel') -Encoding ascii
    'outside sentinel' | Set-Content -LiteralPath (Join-Path $outsideGlobal 'sentinel') -Encoding ascii
    # This path deliberately grants ordinary AppContainers read access. LPAC's
    # explicit opt-out must still deny it; if it were visible, wsp would reject
    # the malformed global configuration before touching the workspace.
    'not: [valid' | Set-Content -LiteralPath (Join-Path $outsideGlobal 'wsp/config.yaml') -Encoding utf8NoBOM
    Invoke-Icacls $outsideGlobal @('/grant', 'ALL APPLICATION PACKAGES:(OI)(CI)RX')

    @"
@echo off
setlocal EnableExtensions
set "XDG_DATA_HOME=$outsideGlobal"
set "HOME=$global\home"
set "USERPROFILE=$global\home"
type "$global\sentinel" >nul 2>nul && exit /b 10
echo forbidden > "$global\must-not-create"
if not errorlevel 1 exit /b 11
type "$sibling\sentinel" >nul 2>nul && exit /b 12
echo forbidden > "$sibling\must-not-create"
if not errorlevel 1 exit /b 13
type "$outsideGlobal\sentinel" >nul 2>nul && exit /b 14
echo forbidden > "$outsideGlobal\must-not-create"
if not errorlevel 1 exit /b 15
cd /d "$workspace" || exit /b 20
"$wspCopy" --json describe "lpac confined workspace" > result.json 2> stderr.txt || exit /b 21
findstr /c:"lpac confined workspace" .wsp.yaml >nul || exit /b 22
exit /b 0
"@ | Set-Content -LiteralPath $childCmd -Encoding ascii

    $environment = [string[]]@(
        "ComSpec=$env:ComSpec",
        "SystemRoot=$env:SystemRoot",
        "WINDIR=$env:WINDIR",
        "PATH=$env:SystemRoot\System32;$env:SystemRoot",
        "TEMP=$workspace\tmp",
        "TMP=$workspace\tmp"
    )
    $arguments = '/d /c ""' + $childCmd + '""'
    $exitCode = [Lpac]::Run($containerName, $env:ComSpec, $arguments, $workspace, $environment)
    if ($exitCode -ne 0) { throw "LPAC child failed with exit code $exitCode" }

    if ((Get-Content -LiteralPath (Join-Path $global 'sentinel') -Raw).Trim() -ne 'global sentinel') { throw 'global sentinel changed' }
    if ((Get-Content -LiteralPath (Join-Path $sibling 'sentinel') -Raw).Trim() -ne 'sibling sentinel') { throw 'sibling sentinel changed' }
    if ((Get-Content -LiteralPath (Join-Path $outsideGlobal 'sentinel') -Raw).Trim() -ne 'outside sentinel') { throw 'outside sentinel changed' }
    if (Test-Path -LiteralPath (Join-Path $global 'must-not-create')) { throw 'LPAC child created a global file' }
    if (Test-Path -LiteralPath (Join-Path $sibling 'must-not-create')) { throw 'LPAC child created a sibling file' }
    if (Test-Path -LiteralPath (Join-Path $outsideGlobal 'must-not-create')) { throw 'LPAC child created an outside-global file' }
    if ((Get-Content -LiteralPath (Join-Path $workspace 'result.json') -Raw) -notmatch [regex]::Escape('lpac confined workspace')) { throw 'wsp did not report the workspace mutation' }
    Write-Host 'Windows LPAC confinement passed'
}
finally {
    if (Test-Path -LiteralPath $root) {
        Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue
    }
    if (Test-Path -LiteralPath $outsideGlobal) {
        Remove-Item -LiteralPath $outsideGlobal -Recurse -Force -ErrorAction SilentlyContinue
    }
}
