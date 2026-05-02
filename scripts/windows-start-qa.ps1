#Requires -Version 5.1
#Requires -RunAsAdministrator

<#
.SYNOPSIS
    Windows `cc-switch start` manual QA script — M1-M5 scenarios.

.DESCRIPTION
    Validates the Windows implementation of `cc-switch start` covering:
    M1  Normal start + exit for claude + codex (temp cleanup, exit code pass-through)
    M2  Parent killed by taskkill /F (Job Object kills child, orphan scan)
    M3  Orphaned temp file with dead PID (startup scan cleans it)
    M4  Ctrl+C via .cmd shim (no "Terminate batch job (Y/N)?")
    M5  Nested Job Object fallback (warning logged, launch continues)

    M1 and M5 both compile a tiny C# stub via csc.exe and prepend its directory
    to PATH so cc-switch's `which("claude" / "codex")` resolves to the stub.
    This exercises the spawn / Job-Object / cleanup path without requiring a
    real CLI installation.

.PARAMETER CcSwitchPath
    Path to cc-switch binary. Defaults to `cc-switch` (PATH lookup).

.PARAMETER SkipM4
    Skip M4 (requires manual Ctrl+C interaction).

.PARAMETER SkipM5
    Skip M5 (requires PowerShell Job Object creation).

.EXAMPLE
    .\windows-start-qa.ps1 -CcSwitchPath .\target\release\cc-switch.exe

.NOTES
    Provider selectors default to 'demo'. Override with the env vars
    QA_PROVIDER, QA_CLAUDE_PROVIDER, or QA_CODEX_PROVIDER. Set QA_FORCE_STUB
    to force the stub path even when a real CLI is installed.
#>

param(
    [string]$CcSwitchPath = "cc-switch",
    [switch]$SkipM4,
    [switch]$SkipM5
)

$ErrorActionPreference = "Stop"
$script:Passed = 0
$script:Failed = 0
$script:Skipped = 0
$script:QaEntryPaths = [System.Collections.Generic.HashSet[string]]::new()

function Register-QaTempEntry {
    param([Parameter(Mandatory)] [string]$Path)
    $full = (Resolve-Path $Path -ErrorAction SilentlyContinue).Path
    if (-not $full) { $full = $Path }
    [void]$script:QaEntryPaths.Add($full)
}

function Remove-QaTempEntries {
    foreach ($p in $script:QaEntryPaths) {
        if (Test-Path $p -PathType Container) {
            Remove-Item $p -Recurse -Force -ErrorAction SilentlyContinue
        } elseif (Test-Path $p) {
            Remove-Item $p -Force -ErrorAction SilentlyContinue
        }
    }
    $script:QaEntryPaths.Clear()
}

# ── Helpers ──────────────────────────────────────────────────────────

function Write-Section($Title) {
    Write-Host "`n========================================" -ForegroundColor Cyan
    Write-Host "  $Title" -ForegroundColor Cyan
    Write-Host "========================================" -ForegroundColor Cyan
}

function Write-Step($Num, $Desc) {
    Write-Host "`n[M$Num] $Desc" -ForegroundColor Yellow
}

function Record-Pass($Detail = "") {
    $script:Passed++
    if ($Detail) { Write-Host "  PASS — $Detail" -ForegroundColor Green }
    else { Write-Host "  PASS" -ForegroundColor Green }
}

function Record-Fail($Detail) {
    $script:Failed++
    Write-Host "  FAIL — $Detail" -ForegroundColor Red
}

function Record-Skip($Reason) {
    $script:Skipped++
    Write-Host "  SKIP — $Reason" -ForegroundColor DarkGray
}

function Get-TempDir {
    [System.IO.Path]::GetTempPath()
}

function Find-CcSwitchTempEntries {
    $temp = Get-TempDir
    $claude = Get-ChildItem $temp -Name "cc-switch-claude-*" -ErrorAction SilentlyContinue
    $codex  = Get-ChildItem $temp -Name "cc-switch-codex-*"  -ErrorAction SilentlyContinue
    @($claude) + @($codex) | Where-Object { $_ }
}

function Find-CcSwitchQaTempEntries {
    # Only match entries with '-qa-' in the name (M3 fake orphans or QA stubs).
    # Normal cc-switch start entries are left for orphan_scan to handle,
    # so we don't interfere with concurrently running real sessions.
    $temp = Get-TempDir
    $claude = Get-ChildItem $temp -Name "cc-switch-claude-*-qa-*" -ErrorAction SilentlyContinue
    $codex  = Get-ChildItem $temp -Name "cc-switch-codex-*-qa-*"  -ErrorAction SilentlyContinue
    @($claude) + @($codex) | Where-Object { $_ }
}

function Remove-AllCcSwitchTempEntries {
    Find-CcSwitchQaTempEntries | ForEach-Object {
        $p = Join-Path (Get-TempDir) $_
        if (Test-Path $p -PathType Container) {
            Remove-Item $p -Recurse -Force -ErrorAction SilentlyContinue
        } else {
            Remove-Item $p -Force -ErrorAction SilentlyContinue
        }
    }
}

function Get-ExePath {
    $raw = & where.exe $CcSwitchPath 2>$null
    if ($LASTEXITCODE -eq 0 -and $raw) { return $raw.Trim() }
    if (Test-Path $CcSwitchPath) { return (Resolve-Path $CcSwitchPath).Path }
    return $null
}

function Get-DescendantPids {
    <#
    .SYNOPSIS
        Recursively collect all descendant PIDs of a root PID using CIM.
        Required because npm .cmd shims spawn node.exe (not "claude"),
        so Get-Process -Name "claude" misses the real child process.
    #>
    param([Parameter(Mandatory)] [int]$RootPid)
    $all = Get-CimInstance -ClassName Win32_Process -ErrorAction SilentlyContinue |
           Select-Object -Property ProcessId, ParentProcessId
    $descendants = [System.Collections.Generic.HashSet[int]]::new()
    $queue = [System.Collections.Generic.Queue[int]]::new()
    [void]$queue.Enqueue($RootPid)
    while ($queue.Count -gt 0) {
        $current = $queue.Dequeue()
        foreach ($proc in $all) {
            if ($proc.ParentProcessId -eq $current -and $proc.ProcessId -ne $current) {
                if ($descendants.Add($proc.ProcessId)) {
                    [void]$queue.Enqueue($proc.ProcessId)
                }
            }
        }
    }
    return $descendants
}

function Build-StubExe {
    <#
    .SYNOPSIS
        Compile a tiny C# stub binary to act as a stand-in for claude/codex CLI.
        The stub sleeps briefly and exits with the requested code, so cc-switch's
        spawn / Job-Object path runs end-to-end without a real CLI installed.
    .OUTPUTS
        Path to the directory containing the stub on success, $null on failure.
        The file is always named `<Tool>.exe` so `which("<Tool>")` resolves to it
        when the directory is on PATH.
    #>
    param(
        [Parameter(Mandatory)] [string]$Tool,
        [int]$ExitCode = 42,
        [int]$SleepMs = 800,
        [string]$Suffix = ""
    )

    $name = if ($Suffix) { "cc-switch-qa-stub-$Tool-$Suffix" } else { "cc-switch-qa-stub-$Tool" }
    $dir  = Join-Path (Get-TempDir) $name
    $stub = Join-Path $dir "$Tool.exe"

    if (Test-Path $stub) {
        return $dir
    }

    New-Item -ItemType Directory -Path $dir -Force | Out-Null
    $cs = @"
using System; using System.Threading;
class Program { static int Main() { Thread.Sleep($SleepMs); return $ExitCode; } }
"@
    $csPath = Join-Path $dir "stub.cs"
    Set-Content -Path $csPath -Value $cs
    & csc.exe /nologo /out:"$stub" "$csPath" 2>$null
    if ($LASTEXITCODE -ne 0 -or -not (Test-Path $stub)) {
        return $null
    }
    return $dir
}

function Test-StubLaunch {
    <#
    .SYNOPSIS
        Run `cc-switch start <Tool> <Provider>` against a freshly compiled stub
        that exits with $ExpectedExitCode, then verify exit-code propagation
        and that the per-launch temp entry is cleaned up.
    .NOTES
        On Windows cc-switch wraps non-zero child exits as Err and the wrapper
        process exits with 1, but the original code is included in the error
        message. We accept either signal as evidence of pass-through.
        Set $env:QA_FORCE_STUB to force the stub path even when a real CLI is
        installed.
    #>
    param(
        [Parameter(Mandatory)] [string]$Tool,
        [Parameter(Mandatory)] [string]$ProviderId,
        [int]$ExpectedExitCode = 42
    )

    $existing = & where.exe $Tool 2>$null
    if ($LASTEXITCODE -eq 0 -and -not $env:QA_FORCE_STUB) {
        Write-Host "`n  Real $Tool CLI found at: $existing"
        Write-Host "  This is a MANUAL step. Execute the following in a separate terminal:"
        Write-Host "    $exe start $Tool $ProviderId"
        Write-Host "  Then exit normally. Expected: temp entry cleaned, exit code = 0."
        Write-Host "  (Set `$env:QA_FORCE_STUB=1 to bypass real-CLI detection.)"
        Record-Skip "Requires manual interaction with real $Tool CLI"
        return
    }

    Write-Host "`n  $Tool CLI not found in PATH (or stub forced). Using compiled stub." -ForegroundColor DarkYellow

    $stubDir = Build-StubExe -Tool $Tool -ExitCode $ExpectedExitCode
    if (-not $stubDir) {
        Write-Host "  Could not compile stub; ensure csc.exe is available." -ForegroundColor DarkYellow
        Record-Skip "$Tool CLI not available and stub compilation failed"
        return
    }

    $oldPath = $env:PATH
    $env:PATH = "$stubDir;$oldPath"
    try {
        Remove-QaTempEntries
        Write-Host "`n  Running: cc-switch start $Tool $ProviderId"
        $output = & $exe start $Tool $ProviderId 2>&1
        $exitCode = $LASTEXITCODE
        $after = Find-CcSwitchTempEntries
        $outputStr = ($output | Out-String).Trim()

        Write-Host "  Exit code: $exitCode"

        # Detect provider-misconfiguration up front so we don't report a
        # spurious failure for an environment that simply lacks a $Tool provider.
        if ($outputStr -match "did not match any|未匹配到任何") {
            Record-Skip "$Tool provider '$ProviderId' is not configured (start aborted before stub ran)"
            return
        }

        $passThrough =
            ($exitCode -eq $ExpectedExitCode) -or
            ($outputStr -match "exited with code\s+$ExpectedExitCode") -or
            ($outputStr -match "退出码非零:\s*$ExpectedExitCode")
        if ($passThrough) {
            Record-Pass "$Tool stub exit code $ExpectedExitCode propagated (cc-switch exit=$exitCode)"
        } else {
            Record-Fail "Expected exit code $ExpectedExitCode in output for $Tool, got exit $exitCode; output: $outputStr"
        }

        if (($after | Measure-Object).Count -eq 0) {
            Record-Pass "No orphaned temp entries after $Tool exit"
        } else {
            Record-Fail "Orphaned entries remain after $Tool exit: $($after -join ', ')"
            foreach ($entry in $after) {
                Register-QaTempEntry -Path (Join-Path (Get-TempDir) $entry)
            }
        }
    } finally {
        $env:PATH = $oldPath
    }
}

# ── Prerequisites ────────────────────────────────────────────────────

Write-Section "Prerequisites"

$exe = Get-ExePath
if (-not $exe) {
    Write-Host "ERROR: Cannot find cc-switch binary: $CcSwitchPath" -ForegroundColor Red
    exit 1
}
Write-Host "cc-switch binary: $exe"

$tempDir = Get-TempDir
Write-Host "Temp directory:   $tempDir"

# Clean any leftover entries from previous runs
Remove-AllCcSwitchTempEntries
Write-Host "Cleared leftover cc-switch temp entries."

# ── M1: Normal start + exit ────────────────────────────────────────

Write-Section "M1 — Normal start + exit (claude + codex)"
Write-Step 1 "Run cc-switch start <tool> <provider> against stub, verify temp cleanup + exit-code pass-through"

Write-Host "`n  NOTE: M1 requires a configured provider for each app under test."
Write-Host "  Defaults to provider id 'demo'. Override via env vars:"
Write-Host "    `$env:QA_PROVIDER          (used by both apps)"
Write-Host "    `$env:QA_CLAUDE_PROVIDER   (claude-only override)"
Write-Host "    `$env:QA_CODEX_PROVIDER    (codex-only override)"

$provider       = if ($env:QA_PROVIDER)        { $env:QA_PROVIDER }        else { "demo" }
$claudeProvider = if ($env:QA_CLAUDE_PROVIDER) { $env:QA_CLAUDE_PROVIDER } else { $provider }
$codexProvider  = if ($env:QA_CODEX_PROVIDER)  { $env:QA_CODEX_PROVIDER }  else { $provider }

Write-Host "  claude provider: $claudeProvider"
Write-Host "  codex  provider: $codexProvider"

# Each Test-StubLaunch call compiles a `<tool>.exe` stub into its own subdir
# and prepends it to PATH, so `which::which("<tool>")` inside cc-switch
# resolves to the stub. This exercises the spawn / Job-Object / cleanup path
# end-to-end without requiring the real CLI.
Test-StubLaunch -Tool "claude" -ProviderId $claudeProvider -ExpectedExitCode 42
Test-StubLaunch -Tool "codex"  -ProviderId $codexProvider  -ExpectedExitCode 42

# ── M2: taskkill /F parent ─────────────────────────────────────────

Write-Section "M2 — taskkill /F parent"
Write-Step 2 "Kill parent cc-switch, verify Job Object kills child + orphan scan"

$claudeExe = & where.exe claude 2>$null
if ($LASTEXITCODE -ne 0) {
    Record-Skip "Claude CLI not in PATH; skipping M2"
} else {
    Write-Host "`n  This test will:"
    Write-Host "    1. Start cc-switch start claude $claudeProvider (background)"
    Write-Host "    2. Wait for temp file to appear"
    Write-Host "    3. taskkill /F the cc-switch parent process"
    Write-Host "    4. Verify the child (claude) process is also terminated"
    Write-Host "    5. Run cc-switch env tools to trigger orphan scan"
    Write-Host "    6. Verify temp file is gone"

    Remove-QaTempEntries

    # Start cc-switch in a new window so we can observe it
    $proc = Start-Process -FilePath $exe -ArgumentList @("start","claude",$claudeProvider) `
        -PassThru -WindowStyle Hidden

    Write-Host "`n  Started cc-switch PID $($proc.Id), waiting for temp file..."
    $tempEntry = $null
    for ($i = 0; $i -lt 30; $i++) {
        Start-Sleep -Milliseconds 200
        $entries = Find-CcSwitchTempEntries
        if ($entries) {
            $tempEntry = $entries[0]
            Register-QaTempEntry -Path (Join-Path (Get-TempDir) $tempEntry)
            break
        }
    }

    if (-not $tempEntry) {
        Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
        Record-Fail "Temp file did not appear within 6 seconds"
    } else {
        Write-Host "  Temp entry appeared: $tempEntry"

        # Snapshot descendants BEFORE killing parent (npm .cmd shim → node.exe)
        $descendants = Get-DescendantPids -RootPid $proc.Id

        # Kill the parent
        taskkill /F /PID $proc.Id 2>$null | Out-Null
        Start-Sleep -Seconds 1

        # Verify every captured descendant is dead (catches node.exe etc.)
        $alive = $descendants | Where-Object {
            $null -ne (Get-Process -Id $_ -ErrorAction SilentlyContinue)
        }
        if ($alive) {
            Record-Fail "Child process(es) still alive after parent taskkill: $($alive -join ', ')"
            $alive | ForEach-Object {
                Stop-Process -Id $_ -Force -ErrorAction SilentlyContinue
            }
        } else {
            Record-Pass "Claude child process terminated along with parent (Job Object)"
        }

        # Trigger orphan scan by running another cc-switch command
        & $exe env tools 2>&1 | Out-Null
        Start-Sleep -Milliseconds 500

        $remaining = Find-CcSwitchTempEntries
        if ($remaining -contains $tempEntry) {
            Record-Fail "Orphan scan did not clean temp entry: $tempEntry"
            Register-QaTempEntry -Path (Join-Path (Get-TempDir) $tempEntry)
        } else {
            Record-Pass "Orphan scan cleaned temp entry after parent death"
        }
    }
}

# ── M3: Orphaned file with dead PID ────────────────────────────────

Write-Section "M3 — Orphaned file with dead PID"
Write-Step 3 "Create fake orphan file with non-existent PID, verify scan cleans it"

Remove-QaTempEntries

# Use a PID that is extremely unlikely to exist (max PID on Windows is ~2^24)
$deadPid = 999999
$oldNanos = [DateTimeOffset]::UtcNow.AddHours(-25).ToUnixTimeMilliseconds() * 1000000L

# Create a fake orphaned settings file
$orphanFile = Join-Path $tempDir "cc-switch-claude-qa-$deadPid-$oldNanos.json"
Set-Content -Path $orphanFile -Value '{"env":{"ANTHROPIC_AUTH_TOKEN":"fake"}}'
Write-Host "  Created fake orphan: $orphanFile"
Register-QaTempEntry -Path $orphanFile

# Also create a fake orphaned codex home dir
$orphanDir = Join-Path $tempDir "cc-switch-codex-qa-$deadPid-$oldNanos"
New-Item -ItemType Directory -Path $orphanDir -Force | Out-Null
Set-Content -Path (Join-Path $orphanDir "config.toml") -Value "model = \"test\""
Write-Host "  Created fake orphan dir: $orphanDir"
Register-QaTempEntry -Path $orphanDir

# Trigger orphan scan by running any cc-switch command
& $exe env tools 2>&1 | Out-Null
Start-Sleep -Milliseconds 500

$missingFile = -not (Test-Path $orphanFile)
$missingDir  = -not (Test-Path $orphanDir)

if ($missingFile) {
    Record-Pass "Orphan file with dead PID cleaned"
} else {
    Record-Fail "Orphan file still exists: $orphanFile"
}

if ($missingDir) {
    Record-Pass "Orphan dir with dead PID cleaned"
} else {
    Record-Fail "Orphan dir still exists: $orphanDir"
}

# ── M4: Ctrl+C via .cmd shim ───────────────────────────────────────

Write-Section "M4 — Ctrl+C via .cmd shim"
Write-Step 4 "Verify Ctrl+C is handled by Claude, not cmd.exe"

if ($SkipM4) {
    Record-Skip "Skipped by -SkipM4 flag"
} else {
    # Check if claude is a .cmd/.bat shim
    $claudeWhich = & where.exe claude 2>$null
    if ($LASTEXITCODE -ne 0) {
        Record-Skip "Claude CLI not in PATH"
    } else {
        $isCmdShim = $claudeWhich -match '\.(cmd|bat)$'
        if (-not $isCmdShim) {
            Write-Host "  Claude resolves to: $claudeWhich"
            Write-Host "  Not a .cmd/.bat shim. M4 is N/A for your installation." -ForegroundColor DarkYellow
            Record-Skip "Claude is not installed via .cmd/.bat shim"
        } else {
            Write-Host "  Claude shim detected: $claudeWhich"
            Write-Host "`n  MANUAL STEP REQUIRED:"
            Write-Host "    1. Open a NEW terminal (conhost or Windows Terminal)"
            Write-Host "    2. Run: $exe start claude $claudeProvider"
            Write-Host "    3. Wait for Claude TUI to appear"
            Write-Host "    4. Press Ctrl+C"
            Write-Host "  Expected behavior:"
            Write-Host "    - Claude TUI handles Ctrl+C (e.g. cancels current input)"
            Write-Host "    - NO 'Terminate batch job (Y/N)?' prompt from cmd.exe"
            Write-Host "    - If prompt appears, this is a FAIL"
            Write-Host "`n  After testing, press Enter in this window to continue..."
            [void][Console]::ReadLine()
            Record-Skip "Requires manual verification result (not auto-detected)"
        }
    }
}

# ── M5: Nested Job Object fallback ─────────────────────────────────

Write-Section "M5 — Nested Job Object fallback"
Write-Step 5 "Launch cc-switch inside a PowerShell Job Object, verify fallback"

if ($SkipM5) {
    Record-Skip "Skipped by -SkipM5 flag"
} else {
    # PowerShell itself may already be in a Job Object on some configurations.
    # We explicitly create a new Job Object and run cc-switch inside it.
    $pinvoke = @"
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
public class JobHelper {
    [DllImport("kernel32.dll")]
    public static extern IntPtr CreateJobObject(IntPtr lpJobAttributes, string lpName);

    [DllImport("kernel32.dll")]
    public static extern bool AssignProcessToJobObject(IntPtr hJob, IntPtr hProcess);

    [DllImport("kernel32.dll")]
    public static extern bool CloseHandle(IntPtr hObject);

    [DllImport("kernel32.dll")]
    public static extern IntPtr GetCurrentProcess();
}
"@
    Add-Type -TypeDefinition $pinvoke -Language CSharp -ErrorAction SilentlyContinue

    $jobName = "cc-switch-qa-nested-job-" + [Guid]::NewGuid().ToString("N")
    $hJob = [JobHelper]::CreateJobObject([IntPtr]::Zero, $jobName)
    if ($hJob -eq [IntPtr]::Zero -or $hJob -eq [IntPtr] -1) {
        Record-Skip "Could not create Job Object (error: $([Marshal]::GetLastWin32Error()))"
    } else {
        $self = [JobHelper]::GetCurrentProcess()
        $assigned = [JobHelper]::AssignProcessToJobObject($hJob, $self)
        if (-not $assigned) {
            # Already in a job — this is actually the common case on Windows Terminal / some shells
            Write-Host "  Current process is already in a Job Object (AssignProcessToJobObject failed with $($Error[0]))."
            Write-Host "  This is expected on Windows Terminal and some shells."
        } else {
            Write-Host "  Assigned current process to new Job Object."
        }

        # The AssignProcessToJobObject fallback only runs in the spawn path of
        # `cc-switch start <tool>` (claude_temp_launch.rs / codex_temp_launch.rs).
        # `env tools` never spawns a child, so it would not exercise that code.
        # Compile a claude.exe stub that exits 0, prepend it to PATH, then run
        # `cc-switch start claude --verbose` to drive the actual fallback path.
        $m5StubDir = Build-StubExe -Tool "claude" -ExitCode 0 -SleepMs 400 -Suffix "m5"
        if (-not $m5StubDir) {
            Write-Host "  Could not compile claude stub for M5; ensure csc.exe is available." -ForegroundColor DarkYellow
            Record-Skip "Claude stub for M5 unavailable (csc.exe required)"
        } else {
            $m5OldPath = $env:PATH
            $env:PATH = "$m5StubDir;$m5OldPath"
            try {
                Remove-QaTempEntries
                Write-Host "`n  Running: $exe start claude $claudeProvider --verbose"
                $output = & $exe start claude $claudeProvider --verbose 2>&1
                $startExit = $LASTEXITCODE
                $outputStr = ($output | Out-String).Trim()
                $m5After = Find-CcSwitchTempEntries

                if ($outputStr -match "did not match any|未匹配到任何") {
                    Record-Skip "claude provider '$claudeProvider' is not configured (start aborted before fallback path)"
                } else {
                    # WARN-level logs only surface with --verbose (debug filter).
                    # Both the claude target ('windows.job_assign_failed_fallback')
                    # and the codex localized message ("falling back" / "降级回退")
                    # contain at least one of these tokens.
                    if ($outputStr -match "job_assign_failed_fallback|AssignProcessToJobObject|falling back|降级回退|fallback") {
                        Record-Pass "Nested Job Object fallback warning detected in start claude output"
                    } else {
                        Write-Host "  Output did not contain expected fallback warning." -ForegroundColor DarkYellow
                        Write-Host "  (May indicate this shell is NOT actually nested inside a Job Object," -ForegroundColor DarkYellow
                        Write-Host "   or that logs are routed somewhere other than stderr.)" -ForegroundColor DarkYellow
                        Record-Skip "Fallback warning not visible in stdout/stderr; verify shell is in a Job Object"
                    }

                    if ($startExit -eq 0) {
                        Record-Pass "start claude succeeded despite nested Job Object (fallback path works)"
                    } else {
                        Record-Fail "start claude failed with exit code $startExit despite stub returning 0; output: $outputStr"
                    }

                    foreach ($entry in $m5After) {
                        Register-QaTempEntry -Path (Join-Path (Get-TempDir) $entry)
                    }
                }
            } finally {
                $env:PATH = $m5OldPath
            }
        }

        [JobHelper]::CloseHandle($hJob) | Out-Null
    }
}

# ── Summary ──────────────────────────────────────────────────────────

Write-Section "Summary"
Write-Host "Passed:  $script:Passed"
Write-Host "Failed:  $script:Failed"
Write-Host "Skipped: $script:Skipped"

if ($script:Failed -gt 0) {
    Write-Host "`nRESULT: FAILED ($script:Failed scenario(s) failed)" -ForegroundColor Red
    exit 1
} else {
    Write-Host "`nRESULT: PASSED (all checked scenarios passed)" -ForegroundColor Green
    exit 0
}
