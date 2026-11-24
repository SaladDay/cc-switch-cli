# Install cc-switch on Windows without administrator rights:
#   irm https://raw.githubusercontent.com/SaladDay/cc-switch-cli/main/install.ps1 | iex
#
# Environment overrides (same names as install.sh where they overlap):
#   CC_SWITCH_INSTALL_DIR  install directory (default: %LOCALAPPDATA%\cc-switch)
#   CC_SWITCH_FORCE=1      overwrite an existing cc-switch.exe without prompting
#   CC_SWITCH_VERSION      release tag to install, e.g. v5.11.4 (default: latest)
#
# Everything lives inside one function so that `iex` does not leak variables or
# preference settings into the caller's session, and errors never close it.

function Install-CcSwitch {
    Set-StrictMode -Version 2.0
    $ErrorActionPreference = 'Stop'
    $ProgressPreference = 'SilentlyContinue'

    $releasesUrl = 'https://github.com/SaladDay/cc-switch-cli/releases'
    $assetName = 'cc-switch-cli-windows-x64.zip'
    $binName = 'cc-switch.exe'

    function Write-Info([string]$Message) { Write-Host "  info: $Message" -ForegroundColor Green }
    function Write-Warn([string]$Message) { Write-Host "  warn: $Message" -ForegroundColor Yellow }

    if ($ExecutionContext.SessionState.LanguageMode -ne 'FullLanguage') {
        throw "PowerShell is running in $($ExecutionContext.SessionState.LanguageMode) mode, which this installer cannot use. Follow the manual installation steps instead: https://github.com/SaladDay/cc-switch-cli#-installation"
    }
    if ([Environment]::OSVersion.Platform -ne [PlatformID]::Win32NT) {
        throw "This script only supports Windows. On macOS or Linux, use install.sh: $releasesUrl"
    }
    if (-not [Environment]::Is64BitOperatingSystem) {
        throw "Only 64-bit Windows is supported. See available assets: $releasesUrl"
    }

    $installDir = $env:CC_SWITCH_INSTALL_DIR
    if ([string]::IsNullOrWhiteSpace($installDir)) {
        $installDir = Join-Path $env:LOCALAPPDATA 'cc-switch'
    }
    $installDir = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($installDir)
    if ($installDir -notmatch '^[A-Za-z]:\\$') { $installDir = $installDir.TrimEnd('\') }
    if ($installDir.Contains(';')) {
        throw "Install directory must not contain ';' because it is added to PATH: $installDir"
    }
    $target = Join-Path $installDir $binName

    $version = $env:CC_SWITCH_VERSION
    if ([string]::IsNullOrWhiteSpace($version) -or $version -eq 'latest') {
        $url = "$releasesUrl/latest/download/$assetName"
    } else {
        if (-not $version.StartsWith('v')) { $version = "v$version" }
        $url = "$releasesUrl/download/$version/$assetName"
    }

    # --- overwrite rules (mirror install.sh) ---
    if (Test-Path -LiteralPath $target) {
        $existing = ''
        try { $existing = (& $target --version 2>$null | Select-Object -First 1) } catch { }
        $label = if ($existing) { "$target ($existing)" } else { $target }

        if ($env:CC_SWITCH_FORCE -eq '1') {
            Write-Warn "Existing installation detected at $label; continuing because CC_SWITCH_FORCE=1"
        } else {
            $interactive = [Environment]::UserInteractive -and -not [Console]::IsInputRedirected -and
                -not ([Environment]::GetCommandLineArgs() | Where-Object { $_ -like '-NonI*' })
            if (-not $interactive) {
                throw "Existing installation detected at $label. Nothing was overwritten. Re-run interactively to confirm the update, or set CC_SWITCH_FORCE=1 to allow overwrite."
            }
            $reply = Read-Host "  Existing installation detected at $label.`n`n  [U]pdate or [C]ancel? [U/c]"
            switch -Regex ($reply) {
                '^(u|update)?$' { break }
                '^(c|cancel)$' { Write-Info 'Installation canceled.'; return }
                default {
                    throw "Unrecognized choice: $reply. Nothing was overwritten. Re-run and choose Update, or set CC_SWITCH_FORCE=1 to allow overwrite."
                }
            }
        }
    }

    # Windows PowerShell 5.1 may not enable TLS 1.2 by default.
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

    $tmpDir = Join-Path ([IO.Path]::GetTempPath()) ("cc-switch-install-" + [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $tmpDir | Out-Null
    try {
        # --- download & extract ---
        $zip = Join-Path $tmpDir $assetName
        Write-Info "Downloading $assetName"
        try {
            Invoke-WebRequest -Uri $url -OutFile $zip -UseBasicParsing
        } catch {
            throw "Download failed: $url ($($_.Exception.Message)). No binary was installed or replaced. See available assets: $releasesUrl"
        }

        Write-Info 'Extracting archive'
        Expand-Archive -LiteralPath $zip -DestinationPath $tmpDir -Force
        $extracted = Join-Path $tmpDir $binName
        if (-not (Test-Path -LiteralPath $extracted)) {
            throw "Binary '$binName' not found in archive."
        }

        # --- install ---
        [void][IO.Directory]::CreateDirectory($installDir)
        # Stage next to the target, move the old exe aside, then move the new one
        # in. Windows allows renaming a running exe, so a running cc-switch keeps
        # working from its backup, which a later run removes.
        Get-ChildItem -LiteralPath $installDir -Filter "$binName.*.old" -ErrorAction SilentlyContinue |
            Remove-Item -Force -ErrorAction SilentlyContinue
        $staged = "$target.new"
        $backup = "$target.$([Guid]::NewGuid().ToString('N')).old"
        $hadTarget = Test-Path -LiteralPath $target
        try {
            Copy-Item -LiteralPath $extracted -Destination $staged -Force
            if ($hadTarget) { [IO.File]::Move($target, $backup) }
        } catch {
            Remove-Item -LiteralPath $staged -Force -ErrorAction SilentlyContinue
            throw "Could not replace $target ($($_.Exception.Message)). Close every program using it and try again. Nothing was overwritten."
        }
        try {
            [IO.File]::Move($staged, $target)
        } catch {
            $installError = $_.Exception.Message
            if ($hadTarget) {
                try {
                    [IO.File]::Move($backup, $target)
                } catch {
                    throw "Could not install $target ($installError), and restoring the previous version failed ($($_.Exception.Message)). The previous version is at $backup; rename it to $binName."
                }
            }
            Remove-Item -LiteralPath $staged -Force -ErrorAction SilentlyContinue
            throw "Could not install $target ($installError). Nothing was overwritten."
        }
        if ($hadTarget) {
            try {
                Remove-Item -LiteralPath $backup -Force
            } catch {
                Write-Warn "The previous cc-switch is still running; $backup will be removed by the next install."
            }
        }
        Write-Info "Installed cc-switch to $target"
    } finally {
        Remove-Item -LiteralPath $tmpDir -Recurse -Force -ErrorAction SilentlyContinue
    }

    # --- user PATH ---
    # Read the raw registry value so entries such as %USERPROFILE%\bin stay
    # unexpanded, and write it back with its existing kind (REG_EXPAND_SZ if new).
    $envKey = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Environment')
    try {
        $userPath = [string]$envKey.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
        $entries = @($userPath -split ';' | Where-Object { $_ -ne '' })
        $present = $entries | Where-Object {
            [Environment]::ExpandEnvironmentVariables($_).TrimEnd('\') -eq $installDir.TrimEnd('\')
        }
        if ($present) {
            Write-Info "$installDir is already in your user PATH"
        } else {
            $kind = if ($null -ne $envKey.GetValue('Path')) { $envKey.GetValueKind('Path') } else { [Microsoft.Win32.RegistryValueKind]::ExpandString }
            $envKey.SetValue('Path', (($entries + $installDir) -join ';'), $kind)
            Write-Info "Added $installDir to your user PATH"
            # Tell Explorer and new terminals that the environment changed.
            try {
                if (-not ('CcSwitchInstaller.Native' -as [type])) {
                    Add-Type -Namespace CcSwitchInstaller -Name Native -MemberDefinition @'
[DllImport("user32.dll", SetLastError = true, CharSet = CharSet.Auto)]
public static extern IntPtr SendMessageTimeout(IntPtr hWnd, uint Msg, UIntPtr wParam, string lParam, uint fuFlags, uint uTimeout, out UIntPtr lpdwResult);
'@
                }
                $result = [UIntPtr]::Zero
                [void][CcSwitchInstaller.Native]::SendMessageTimeout([IntPtr]0xffff, 0x1A, [UIntPtr]::Zero, 'Environment', 2, 5000, [ref]$result)
            } catch {
                Write-Warn "Could not notify other programs about the PATH change ($($_.Exception.Message)). Sign out and back in if new terminals cannot find cc-switch."
            }
        }
    } finally {
        $envKey.Close()
    }

    $sessionEntries = @($env:Path -split ';' | ForEach-Object { $_.TrimEnd('\') })
    if ($sessionEntries -notcontains $installDir.TrimEnd('\')) {
        $env:Path = "$($env:Path.TrimEnd(';'));$installDir"
    }

    $resolved = Get-Command cc-switch -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($resolved -and $resolved.Path -ne $target) {
        Write-Warn "cc-switch currently resolves to $($resolved.Path), so $target may be shadowed."
    }

    & $target --version
    if ($LASTEXITCODE -ne 0) {
        throw "$target --version failed with exit code $LASTEXITCODE."
    }
    Write-Host '  Optional: run cc-switch completions powershell to generate PowerShell completions.'
    Write-Host '  Open a new terminal if cc-switch is not found in other windows.'
}

Install-CcSwitch
