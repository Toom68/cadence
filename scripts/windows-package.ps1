# Build Cadence.exe, a portable zip and the installer for Windows.
#
# Usage:  powershell -ExecutionPolicy Bypass -File scripts/windows-package.ps1
# Output: dist/Cadence-<version>-windows-x64/Cadence.exe,
#         dist/Cadence-<version>-windows-x64.zip and (with Inno Setup 6)
#         dist/Cadence-<version>-windows-x64-setup.exe
# The executable and installer are unsigned (no Authenticode certificate).
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

# Buyer-facing files: README, third-party notices, and the EULA when the
# owner has provided one at the repo root.
Copy-Item (Join-Path $Root 'packaging/README.txt') (Join-Path $Stage 'README.txt')
Copy-Item (Join-Path $Root 'THIRD-PARTY-NOTICES.txt') (Join-Path $Stage 'THIRD-PARTY-NOTICES.txt')
$Eula = Join-Path $Root 'EULA.txt'
if (Test-Path $Eula) {
    Copy-Item $Eula (Join-Path $Stage 'EULA.txt')
} else {
    Write-Warning 'No EULA.txt at the repo root — add your end-user license before selling.'
}

# Smoke test: the GUI-subsystem binary still answers CLI requests.
$Out = Join-Path $Stage 'version.txt'
$Proc = Start-Process -FilePath $Exe -ArgumentList '--version' -Wait -PassThru -NoNewWindow -RedirectStandardOutput $Out
if ($Proc.ExitCode -ne 0) { throw "Cadence.exe --version failed with exit code $($Proc.ExitCode)" }
Get-Content $Out
Remove-Item $Out

Compress-Archive -Path $Stage -DestinationPath $Zip -CompressionLevel Optimal
Write-Host "Created $Exe"
Write-Host "Created $Zip"

# The installer (Inno Setup 6): per-user, no admin, Start menu entry, .cadence
# association, uninstaller — and the copy the in-app updater can upgrade.
# Skipped with a warning when ISCC.exe is missing, unless REQUIRE_INSTALLER=1.
$Iscc = (Get-Command iscc.exe -ErrorAction SilentlyContinue | Select-Object -First 1).Source
if (-not $Iscc) {
    $Iscc = @(
        "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
        "$env:ProgramFiles\Inno Setup 6\ISCC.exe",
        "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe"
    ) | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
}
if ($Iscc) {
    $Setup = Join-Path $Dist "$Name-setup.exe"
    if (Test-Path $Setup) { Remove-Item -Force $Setup }
    & $Iscc /Qp "/DAppVersion=$Version" "/DSourceDir=$Stage" "/DOutputDir=$Dist" "/DRepoRoot=$Root" (Join-Path $Root 'packaging/windows/cadence.iss')
    if ($LASTEXITCODE -ne 0) { throw "ISCC failed with exit code $LASTEXITCODE" }
    if (-not (Test-Path $Setup)) { throw "expected installer not found: $Setup" }
    Write-Host "Created $Setup"
} elseif ($env:REQUIRE_INSTALLER -eq '1') {
    throw 'Inno Setup 6 (ISCC.exe) not found; install it (choco install innosetup) or unset REQUIRE_INSTALLER.'
} else {
    Write-Warning 'Inno Setup 6 (ISCC.exe) not found; skipped the installer. The portable zip is ready.'
}
