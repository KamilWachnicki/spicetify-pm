<#
.SYNOPSIS
    spicepm installer for Windows.

.DESCRIPTION
    Downloads the latest (or given) release from GitHub, verifies its
    checksum when a .sha256 sidecar is published, installs the binary into
    the user's Programs directory, and adds it to the user PATH.

.EXAMPLE
    ./install.ps1
    ./install.ps1 -Version v0.1.0
    ./install.ps1 -InstallDir D:\tools\spicepm
#>
param(
    [string]$Version = "",
    [string]$InstallDir = "$env:LOCALAPPDATA\Programs\spicepm"
)

$ErrorActionPreference = "Stop"
$Repo = "KamilWachnicki/spicetify-pm"
$Bin = "spicepm"

function Write-Info($msg) { Write-Host $msg -ForegroundColor Green }
function Write-Warn2($msg) { Write-Host $msg -ForegroundColor Yellow }
function Write-Err($msg) { Write-Host $msg -ForegroundColor Red }

# TLS 1.2 for older hosts
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

switch ($env:PROCESSOR_ARCHITECTURE) {
    "AMD64" { $arch = "x86_64" }
    "ARM64" { $arch = "aarch64" }
    default { Write-Err "unsupported architecture: $($env:PROCESSOR_ARCHITECTURE)"; exit 1 }
}

if (-not $Version) {
    Write-Info "fetching the latest release"
    try {
        $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest"
        $Version = $release.tag_name
    } catch {
        Write-Err "no release found on $Repo"
        Write-Err "build from source instead:  cargo install --path ."
        exit 1
    }
}

$asset = "$Bin-$Version-$arch-windows.zip"
$url = "https://github.com/$Repo/releases/download/$Version/$asset"
$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ([System.Guid]::NewGuid().ToString())
New-Item -ItemType Directory -Path $tmp | Out-Null

try {
    Write-Info "downloading $asset"
    Invoke-WebRequest -Uri $url -OutFile (Join-Path $tmp $asset)

    # verify when the release publishes a .sha256 sidecar
    $hasSidecar = $false
    try {
        Invoke-WebRequest -Uri "$url.sha256" `
            -OutFile (Join-Path $tmp "$asset.sha256") `
            -ErrorAction Stop
        $hasSidecar = $true
    } catch {
        # 404 / network hiccup: handled below by $hasSidecar being false
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

    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    Copy-Item $exe (Join-Path $InstallDir "$Bin.exe") -Force

    # persist to the user PATH when missing
    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if (-not $userPath) { $userPath = "" }
    if ($userPath -notlike "*$InstallDir*") {
        [Environment]::SetEnvironmentVariable(
            "Path", "$userPath;$InstallDir", "User")
        Write-Warn2 "$InstallDir was added to your user PATH"
        Write-Warn2 "open a new terminal for it to take effect"
        $env:Path += ";$InstallDir"
    }

    Write-Info "installed $Bin $Version -> $(Join-Path $InstallDir "$Bin.exe")"
    Write-Info "next steps:"
    Write-Info "  - spicetify must be installed (https://spicetify.app)"
    Write-Info '  - $env:GITHUB_TOKEN = "..." to avoid API rate limits'
} finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
