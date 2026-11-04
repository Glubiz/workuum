<#
.SYNOPSIS
Installs workuum on Windows.
.EXAMPLE
irm https://raw.githubusercontent.com/Glubiz/workuum/main/install.ps1 | iex
.EXAMPLE
& ([scriptblock]::Create((irm https://raw.githubusercontent.com/Glubiz/workuum/main/install.ps1))) -Service
#>
param(
    [string]$Version,
    [string]$InstallDir = $(if ($env:WORKUUM_INSTALL_DIR) { $env:WORKUUM_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'Programs\workuum' }),
    [switch]$Service,
    [switch]$Force = ($env:WORKUUM_INSTALL_FORCE -eq '1')
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

$Repo = 'Glubiz/workuum'

if (-not $Version) {
    $Version = (Invoke-RestMethod "https://api.github.com/repos/$Repo/releases/latest" -Headers @{ 'User-Agent' = 'workuum-installer' }).tag_name
}
$Version = $Version -replace '^v', ''
if (-not $Version) { throw 'Could not determine latest version.' }

$InstallDir = [IO.Path]::GetFullPath($InstallDir)
$Target = Join-Path $InstallDir 'workuum.exe'

# Chocolatey shims and installs live under ChocolateyInstall; only refuse when we would overwrite one of its files.
if ((Test-Path $Target) -and $env:ChocolateyInstall -and $Target.StartsWith($env:ChocolateyInstall, [StringComparison]::OrdinalIgnoreCase) -and -not $Force) {
    Write-Error "$Target is a Chocolatey-managed installation. Use 'choco upgrade workuum -y' to update, or pass -Force (WORKUUM_INSTALL_FORCE=1) to override."
}

$Asset = "workuum-$Version-windows.exe"
$Url = "https://github.com/$Repo/releases/download/v$Version/$Asset"
$Tmp = Join-Path ([IO.Path]::GetTempPath()) ("workuum-" + [Guid]::NewGuid())
New-Item -ItemType Directory -Path $Tmp | Out-Null

try {
    Write-Host "Installing workuum v$Version for windows..."
    Write-Host "Downloading $Url..."
    Invoke-WebRequest -UseBasicParsing -Uri $Url -OutFile (Join-Path $Tmp $Asset)

    Write-Host "Downloading $Url.sha256..."
    try {
        Invoke-WebRequest -UseBasicParsing -Uri "$Url.sha256" -OutFile (Join-Path $Tmp "$Asset.sha256")
    } catch {
        throw "This release does not publish a checksum for $Asset. Refusing to install an unverified download."
    }

    $expected = ((Get-Content (Join-Path $Tmp "$Asset.sha256") -Raw).Trim() -split '\s+')[0]
    $actual = (Get-FileHash -Algorithm SHA256 (Join-Path $Tmp $Asset)).Hash
    if ($actual -ne $expected) {
        throw "Checksum verification failed for $Asset. The download may be corrupted or tampered with, not installing it."
    }
    Write-Host 'Checksum verified.'

    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    # A running task holds workuum.exe open. Remember whether it exists, then stop it before replacing.
    # Native stderr must not become a terminating error on 5.1, so relax the preference for these calls.
    $ErrorActionPreference = 'Continue'
    & schtasks.exe /Query /TN workuum *> $null
    $taskExisted = ($LASTEXITCODE -eq 0)
    if ($taskExisted) {
        & schtasks.exe /End /TN workuum *> $null
        # /End can return before the process exits; give the old image a moment to go away.
        Get-Process workuum -ErrorAction SilentlyContinue | Where-Object { $_.Path -and $_.Path.StartsWith($Target, [StringComparison]::OrdinalIgnoreCase) } | Wait-Process -Timeout 15 -ErrorAction SilentlyContinue
    }
    $ErrorActionPreference = 'Stop'
    # A running exe cannot be overwritten but can be renamed. Leftovers from earlier installs may still be held.
    Get-ChildItem -Path $InstallDir -Filter 'workuum.exe*.old' -ErrorAction SilentlyContinue | Remove-Item -Force -ErrorAction SilentlyContinue
    $Old = "$Target.old"
    if (Test-Path $Old) { $Old = "$Target.$PID.old" }
    if (Test-Path $Target) { Move-Item -Force $Target $Old }
    try {
        Move-Item -Force (Join-Path $Tmp $Asset) $Target
    } catch {
        if (Test-Path $Old) { Move-Item -Force $Old $Target }
        throw
    }
    # Fails while the old exe is still running; the next install removes it.
    Remove-Item -Force $Old -ErrorAction SilentlyContinue
    Write-Host "workuum v$Version installed to $Target"

    # Read the raw value so %VAR% entries and the REG_EXPAND_SZ type survive the write-back.
    $envKey = Get-Item 'HKCU:\Environment'
    $rawPath = [string]$envKey.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
    $entries = @($rawPath -split ';' | Where-Object { $_ })
    $expanded = @($entries | ForEach-Object { [Environment]::ExpandEnvironmentVariables($_).TrimEnd('\') })
    if ($expanded -notcontains $InstallDir.TrimEnd('\')) {
        New-ItemProperty -Path 'HKCU:\Environment' -Name Path -Value (($entries + $InstallDir) -join ';') -PropertyType ExpandString -Force | Out-Null
        Write-Host "Added $InstallDir to your user PATH."
        # Registry writes do not notify running apps; broadcast WM_SETTINGCHANGE so new terminals pick it up.
        if (-not ('WorkuumNative.Env' -as [type])) {
            Add-Type -Namespace WorkuumNative -Name Env -MemberDefinition '[DllImport("user32.dll", SetLastError = true, CharSet = CharSet.Auto)] public static extern IntPtr SendMessageTimeout(IntPtr hWnd, uint Msg, UIntPtr wParam, string lParam, uint fuFlags, uint uTimeout, out UIntPtr lpdwResult);'
        }
        $broadcastResult = [UIntPtr]::Zero
        [void][WorkuumNative.Env]::SendMessageTimeout([IntPtr]0xffff, 0x1A, [UIntPtr]::Zero, 'Environment', 2, 5000, [ref]$broadcastResult)
    }
    if (@($env:Path -split ';') -notcontains $InstallDir) {
        $env:Path = "$env:Path;$InstallDir"
    }

    if ($Service) {
        try {
            $base = "https://raw.githubusercontent.com/$Repo/v$Version/service"
            $xml = (Invoke-WebRequest -UseBasicParsing -Uri "$base/workuum.xml").Content
            $escaped = [Security.SecurityElement]::Escape($Target)
            $xml = $xml.Replace('<Command>workuum.exe</Command>', "<Command>$escaped</Command>")
            # schtasks only reads the XML as UTF-16, so the declaration and the file encoding must both say so.
            $xml = $xml.Replace('encoding="UTF-8"', 'encoding="UTF-16"')
            $xmlFile = Join-Path $Tmp 'workuum.xml'
            [IO.File]::WriteAllText($xmlFile, $xml, [Text.Encoding]::Unicode)
            $out = & schtasks.exe /Create /TN workuum /XML $xmlFile /F 2>&1
            if ($LASTEXITCODE -ne 0) { throw "schtasks failed: $out" }
            $out = & schtasks.exe /Run /TN workuum 2>&1
            if ($LASTEXITCODE -ne 0) { throw "schtasks /Run failed: $out" }
            Write-Host 'Scheduled task "workuum" registered and started (runs at logon).'
        } catch {
            Write-Host "workuum IS installed at $Target, but the service could not be registered: $_" -ForegroundColor Red
            Write-Host 'Register it manually: download service/workuum.xml, set <Command> to the full path above, then run:'
            Write-Host '  schtasks /Create /TN workuum /XML workuum.xml /F'
            throw 'Service registration failed.'
        }
    } elseif ($taskExisted) {
        # The task was stopped to replace the exe, so start it again.
        $ErrorActionPreference = 'Continue'
        & schtasks.exe /Run /TN workuum *> $null
        if ($LASTEXITCODE -ne 0) { Write-Warning 'Could not restart the workuum scheduled task; run: schtasks /Run /TN workuum' }
        $ErrorActionPreference = 'Stop'
    }

    Write-Host "Run '$Target --version' to verify."
} finally {
    Remove-Item -Recurse -Force $Tmp -ErrorAction SilentlyContinue
}
