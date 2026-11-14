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

$Dist = Join-Path $Root 'dist'
$Name = "Cadence-$Version-windows-x64"
$Stage = Join-Path $Dist $Name
$Zip = Join-Path $Dist "$Name.zip"

if (Test-Path $Stage) { Remove-Item -Recurse -Force $Stage }
if (Test-Path $Zip) { Remove-Item -Force $Zip }
New-Item -ItemType Directory -Force -Path $Stage | Out-Null

$Exe = Join-Path $Stage 'Cadence.exe'
Copy-Item (Join-Path $Root 'target/release/cadence.exe') $Exe

# Smoke test: the GUI-subsystem binary still answers CLI requests.
$Out = Join-Path $Stage 'version.txt'
$Proc = Start-Process -FilePath $Exe -ArgumentList '--version' -Wait -PassThru -NoNewWindow -RedirectStandardOutput $Out
if ($Proc.ExitCode -ne 0) { throw "Cadence.exe --version failed with exit code $($Proc.ExitCode)" }
Get-Content $Out
Remove-Item $Out

Compress-Archive -Path $Stage -DestinationPath $Zip -CompressionLevel Optimal
Write-Host "Created $Exe"
Write-Host "Created $Zip"
