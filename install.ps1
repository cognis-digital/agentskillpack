# Build agentskillpack in release mode and install it onto your PATH.
#
# Usage:  ./install.ps1 [-Dest <dir>]
# Default dest: $env:USERPROFILE\.cargo\bin
param([string]$Dest = "")

$ErrorActionPreference = "Stop"
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location $here

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Error "cargo not found. Install Rust from https://rustup.rs"
    exit 1
}

Write-Host "building release binary..."
cargo build --release
if ($LASTEXITCODE -ne 0) { throw "build failed" }

$bin = Join-Path $here "target/release/agentskillpack.exe"
if (-not (Test-Path $bin)) { $bin = Join-Path $here "target/release/agentskillpack" }

if ([string]::IsNullOrEmpty($Dest)) {
    $Dest = Join-Path $env:USERPROFILE ".cargo\bin"
}
New-Item -ItemType Directory -Force -Path $Dest | Out-Null
Copy-Item $bin -Destination $Dest -Force
Write-Host "installed agentskillpack -> $Dest"
Write-Host "run: agentskillpack --help"
