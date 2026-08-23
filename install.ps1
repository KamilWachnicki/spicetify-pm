<#
.SYNOPSIS
    spice-pm installer for Windows.

.DESCRIPTION
    Downloads the latest (or given) release from GitHub, verifies its
    checksum when a .sha256 sidecar is published, installs the binary into
    the user's Programs directory, and adds it to the user PATH.

.EXAMPLE
    ./install.ps1

.EXAMPLE
    ./install.ps1 -Version v0.1.0

.EXAMPLE
    ./install.ps1 -InstallDir D:\tools\spice-pm

.EXAMPLE
    ./install.ps1 -AllowAdmin
    # only if you really mean to install from an elevated session
#>
param(
    [string]$Version = "",
    [string]$InstallDir = "$env:LOCALAPPDATA\Programs\spice-pm",
    [switch]$AllowAdmin
)

$ErrorActionPreference = "Stop"
$Repo = "KamilWachnicki/spicetify-pm"
$Bin = "spice-pm"

function Write-Info($msg) { Write-Host $msg -ForegroundColor Green }
function Write-Warn2($msg) { Write-Host $msg -ForegroundColor Yellow }
function Write-Err($msg) { Write-Host $msg -ForegroundColor Red }

# refuse elevated sessions by default: the binary would land somewhere the
# normal user cannot touch, which breaks later self-updates
$principal = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
if ($principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator) -and -not $AllowAdmin) {
    Write-Err "refusing to run elevated:"
    Write-Err "the binary would be installed where self-update cannot touch it."
    Write-Err "run this from a non-admin terminal, or pass -AllowAdmin to override."
    exit 1
}

# Windows PowerShell on older Windows installations needs TLS 1.2 explicitly.
# PowerShell 7 uses the modern .NET HTTP stack, so do not restrict its TLS set.
if ($PSVersionTable.PSEdition -eq "Desktop") {
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
}

# A 32-bit PowerShell process on 64-bit Windows reports x86 here; Windows
# exposes the native architecture separately in that case.
$processorArchitecture = if ($env:PROCESSOR_ARCHITEW6432) {
    $env:PROCESSOR_ARCHITEW6432
} else {
    $env:PROCESSOR_ARCHITECTURE
}
switch ($processorArchitecture.ToUpperInvariant()) {
    "AMD64" { $arch = "x86_64" }
    "ARM64" {
        Write-Err "Windows on ARM is not supported yet (no ARM64 release is published)"
        exit 1
    }
    default { Write-Err "unsupported architecture: $processorArchitecture"; exit 1 }
}

function Get-StatusCode($err) {
    if ($err.Exception.Response) { return [int]$err.Exception.Response.StatusCode }
    return 0
}

function Invoke-DownloadWithRetry($Url, $OutFile) {
    for ($attempt = 1; $attempt -le 3; $attempt++) {
        try {
            $request = @{ Uri = $Url; OutFile = $OutFile }
            # Windows PowerShell otherwise depends on the legacy IE engine;
            # PowerShell 6+ already uses basic parsing and needs no switch.
            if ($PSVersionTable.PSEdition -eq "Desktop") {
                $request.UseBasicParsing = $true
            }
            Invoke-WebRequest @request
            return
        } catch {
            $code = Get-StatusCode $_
            # client mistakes are not worth retrying (except rate limits)
            if ($code -ge 400 -and $code -lt 500 -and $code -ne 429) { throw }
            if ($attempt -eq 3) { throw }
            Start-Sleep -Seconds $attempt
        }
    }
}

function Test-PathEntry($PathValue, $Entry) {
    if (-not $PathValue) { return $false }
    return @($PathValue -split ';' | Where-Object {
        $_.TrimEnd('\\') -ieq $Entry.TrimEnd('\\')
    }).Count -gt 0
}

function Add-PathEntry($PathValue, $Entry) {
    if (-not $PathValue) { return $Entry }
    return "$PathValue;$Entry"
}

if (-not $Version) {
    Write-Info "fetching the latest release"
    try {
        # use a token when provided so installs don't trip over
        # unauthenticated rate limits (same vars the CLI reads)
        $token = @($env:SPICEPM_GITHUB_TOKEN, $env:GITHUB_TOKEN, $env:GH_TOKEN) |
            Where-Object { $_ } |
            Select-Object -First 1
        $headers = if ($token) { @{ Authorization = "Bearer $token" } } else { @{} }
        $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest" `
            -Headers $headers
        $Version = $release.tag_name
    } catch {
        $code = Get-StatusCode $_
        if ($code -in 403, 429) {
            Write-Err "rate limited by the GitHub API while looking up the latest release"
            Write-Err "set SPICEPM_GITHUB_TOKEN (or GITHUB_TOKEN/GH_TOKEN) and re-run"
        } elseif ($code -eq 404) {
            Write-Err "no releases published on $Repo yet"
            Write-Err "build from source instead:  cargo install --git https://github.com/$Repo"
        } else {
            Write-Err "could not reach the GitHub API (HTTP $code)"
        }
        exit 1
    }
}

# tolerate bare versions: -Version 0.4.0 means v0.4.0
if (-not $Version.StartsWith("v")) { $Version = "v$Version" }

$asset = "$Bin-$Version-$arch-windows.zip"
$url = "https://github.com/$Repo/releases/download/$Version/$asset"
$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ([System.Guid]::NewGuid().ToString())
New-Item -ItemType Directory -Path $tmp | Out-Null

try {
    Write-Info "downloading $asset"
    try {
        Invoke-DownloadWithRetry $url (Join-Path $tmp $asset)
    } catch {
        $code = Get-StatusCode $_
        Write-Err "download failed (HTTP $code): $url"
        Write-Err "check that release $Version exists and has a build for $arch-windows"
        exit 1
    }

    # A missing checksum sidecar is supported for older releases. Do not turn
    # a network/server error into an unverified install, though.
    $hasSidecar = $false
    try {
        Invoke-DownloadWithRetry "$url.sha256" (Join-Path $tmp "$asset.sha256")
        $hasSidecar = $true
    } catch {
        $code = Get-StatusCode $_
        if ($code -ne 404) {
            Write-Err "could not download the release checksum (HTTP $code): $url.sha256"
            exit 1
        }
    }

    if ($hasSidecar) {
        Write-Info "verifying checksum"
        $expected = ((Get-Content (Join-Path $tmp "$asset.sha256") |
            Select-Object -First 1).Trim() -split '\s+')[0]
        $actual = (Get-FileHash -Algorithm SHA256 `
            -Path (Join-Path $tmp $asset)).Hash.ToLower()
        if ($expected -ne $actual) {
            Write-Err "checksum mismatch: expected $expected, got $actual"
            exit 1
        }
    } else {
        Write-Warn2 "release has no checksum sidecar; skipping verification"
    }

    Expand-Archive -Path (Join-Path $tmp $asset) -DestinationPath $tmp -Force
    $exe = Join-Path $tmp "$Bin.exe"
    if (-not (Test-Path $exe)) {
        Write-Err "archive did not contain $Bin.exe"
        exit 1
    }

    $destExe = Join-Path $InstallDir "$Bin.exe"
    if (Test-Path $destExe) {
        $old = (& $destExe --version 2>$null | Select-Object -First 1)
        if ($old) {
            Write-Info "upgrading ($old -> $Version)"
        } else {
            Write-Info "replacing existing installation at $destExe"
        }
    } else {
        Write-Info "fresh install into $InstallDir"
    }

    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    Copy-Item $exe $destExe -Force

    # persist to the user PATH when missing
    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if (-not $userPath) { $userPath = "" }
    if (-not (Test-PathEntry $userPath $InstallDir)) {
        [Environment]::SetEnvironmentVariable(
            "Path", (Add-PathEntry $userPath $InstallDir), "User")
        Write-Warn2 "$InstallDir was added to your user PATH"
        Write-Warn2 "open a new terminal for it to take effect"
    }
    if (-not (Test-PathEntry $env:Path $InstallDir)) {
        $env:Path = Add-PathEntry $env:Path $InstallDir
        Write-Info "updated PATH for this PowerShell session"
    }

    Write-Info "installed $Bin $Version -> $destExe"
    Write-Info "next steps:"
    Write-Info "  - spicetify must be installed (https://spicetify.app)"
    Write-Info '  - [Environment]::SetEnvironmentVariable("GITHUB_TOKEN", "YOUR_TOKEN_HERE", "User") - to avoid API rate limits (optional)'
} finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
