# Build Cadence.exe and a portable zip for Windows.
#
# Usage:  powershell -ExecutionPolicy Bypass -File scripts/windows-package.ps1
# Output: dist/Cadence-<version>-windows-x64/Cadence.exe and
#         dist/Cadence-<version>-windows-x64.zip
# The executable is unsigned (no Authenticode certificate).
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$Root = Split-Path -Parent $PSScriptRoot
Set-Location $Root

$Version = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
if (-not $Version) { throw 'Could not read version from Cargo.toml' }

cargo build --release --locked
if ($LASTEXITCODE -ne 0) { throw "cargo build failed with exit code $LASTEXITCODE" }

# Cargo builds into CARGO_TARGET_DIR when it is set (relative paths resolve
# from the repo root, like cargo's), else into ./target.
$TargetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { 'target' }
if (-not [System.IO.Path]::IsPathRooted($TargetDir)) { $TargetDir = Join-Path $Root $TargetDir }
$Built = Join-Path $TargetDir 'release/cadence.exe'
if (-not (Test-Path $Built)) { throw "expected binary not found: $Built" }

$Dist = Join-Path $Root 'dist'
$Name = "Cadence-$Version-windows-x64"
$Stage = Join-Path $Dist $Name
$Zip = Join-Path $Dist "$Name.zip"

if (Test-Path $Stage) { Remove-Item -Recurse -Force $Stage }
if (Test-Path $Zip) { Remove-Item -Force $Zip }
New-Item -ItemType Directory -Force -Path $Stage | Out-Null

$Exe = Join-Path $Stage 'Cadence.exe'
Copy-Item $Built $Exe

# Font licenses (SIL OFL 1.1 for Inter, Bitstream Vera / DejaVu for DejaVu Sans).
$Licenses = Join-Path $Stage 'licenses'
New-Item -ItemType Directory -Force -Path $Licenses | Out-Null
Copy-Item (Join-Path $Root 'assets/fonts/OFL.txt') (Join-Path $Licenses 'Inter-OFL.txt')
Copy-Item (Join-Path $Root 'assets/fonts/DejaVu-LICENSE.txt') (Join-Path $Licenses 'DejaVu-LICENSE.txt')

# Smoke test: the GUI-subsystem binary still answers CLI requests.
$Out = Join-Path $Stage 'version.txt'
$Proc = Start-Process -FilePath $Exe -ArgumentList '--version' -Wait -PassThru -NoNewWindow -RedirectStandardOutput $Out
if ($Proc.ExitCode -ne 0) { throw "Cadence.exe --version failed with exit code $($Proc.ExitCode)" }
Get-Content $Out
Remove-Item $Out

Compress-Archive -Path $Stage -DestinationPath $Zip -CompressionLevel Optimal
Write-Host "Created $Exe"
Write-Host "Created $Zip"
