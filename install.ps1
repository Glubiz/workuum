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
    # A running task holds workuum.exe open; stop it before replacing. Failure or "not running" is fine.
    try { & schtasks.exe /End /TN workuum 2>&1 | Out-Null } catch { }
    Move-Item -Force (Join-Path $Tmp $Asset) $Target
    Write-Host "workuum v$Version installed to $Target"

    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    $entries = @($userPath -split ';' | Where-Object { $_ })
    if ($entries -notcontains $InstallDir) {
        [Environment]::SetEnvironmentVariable('Path', (($entries + $InstallDir) -join ';'), 'User')
        Write-Host "Added $InstallDir to your user PATH."
    }
    if (@($env:Path -split ';') -notcontains $InstallDir) {
        $env:Path = "$env:Path;$InstallDir"
    }

    if ($Service) {
        try {
            $base = "https://raw.githubusercontent.com/$Repo/v$Version/service"
            $xml = (Invoke-RestMethod -UseBasicParsing "$base/workuum.xml")
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
            exit 1
        }
    }

    Write-Host "Run '$Target --version' to verify."
} finally {
    Remove-Item -Recurse -Force $Tmp -ErrorAction SilentlyContinue
}
