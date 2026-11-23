# Download (once) and verify the General MIDI SoundFont Cadence ships with.
# PowerShell twin of scripts/fetch-soundfont.sh (Windows packaging / CI).
#
# Usage:  pwsh scripts/fetch-soundfont.ps1 [-Dest DIR]
#
# Reads the pinned URL and SHA-256 from packaging/soundfont/pin.txt, keeps a
# verified copy in the cache folder ($env:CADENCE_SOUNDFONT_CACHE, default
# .cache/soundfonts in the repo — gitignored) as "Cadence GM.sf2" next to its
# license text, and, given -Dest, copies both there. A checksum mismatch is
# always fatal (the bad download is deleted). Throws on any failure.
param([string]$Dest = '')
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$Root = Split-Path -Parent $PSScriptRoot
$PinFile = Join-Path $Root 'packaging/soundfont/pin.txt'
$Pin = @{}
foreach ($line in Get-Content $PinFile) {
    if ($line -match '^([A-Z0-9_]+)=(.*)$') { $Pin[$Matches[1]] = $Matches[2].Trim() }
}
foreach ($key in 'SOUNDFONT_URL', 'SOUNDFONT_SHA256', 'SOUNDFONT_FILE', 'SOUNDFONT_LICENSE', 'SOUNDFONT_VERSION') {
    if (-not $Pin.ContainsKey($key) -or -not $Pin[$key]) { throw "$key missing from $PinFile" }
}
$Url = $Pin['SOUNDFONT_URL']
$Sha256 = $Pin['SOUNDFONT_SHA256'].ToLowerInvariant()
$File = $Pin['SOUNDFONT_FILE']
$License = Join-Path $Root "packaging/soundfont/$($Pin['SOUNDFONT_LICENSE'])"

$Cache = if ($env:CADENCE_SOUNDFONT_CACHE) { $env:CADENCE_SOUNDFONT_CACHE } else { Join-Path $Root '.cache/soundfonts' }
if (-not [System.IO.Path]::IsPathRooted($Cache)) { $Cache = Join-Path $Root $Cache }
New-Item -ItemType Directory -Force -Path $Cache | Out-Null
$Font = Join-Path $Cache $File

function Get-Sha256([string]$Path) {
    (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLowerInvariant()
}

if ((Test-Path -LiteralPath $Font) -and ((Get-Sha256 $Font) -eq $Sha256)) {
    Write-Host "SoundFont cached: $Font"
} else {
    if (Test-Path -LiteralPath $Font) { Remove-Item -LiteralPath $Font -Force }
    Write-Host "Downloading $($Pin['SOUNDFONT_VERSION']) from $Url"
    $Tmp = Join-Path $Cache ".download-$PID.tmp"
    try {
        $ProgressPreference = 'SilentlyContinue'
        $attempt = 0
        while ($true) {
            try {
                Invoke-WebRequest -Uri $Url -OutFile $Tmp -UseBasicParsing
                break
            } catch {
                $attempt++
                if ($attempt -ge 3) { throw "could not download the SoundFont from ${Url}: $_" }
                Start-Sleep -Seconds 2
            }
        }
        $Got = Get-Sha256 $Tmp
        if ($Got -ne $Sha256) {
            throw "SoundFont checksum mismatch for $Url`n  expected $Sha256`n  got      $Got"
        }
        Move-Item -LiteralPath $Tmp -Destination $Font -Force
    } finally {
        if (Test-Path -LiteralPath $Tmp) { Remove-Item -LiteralPath $Tmp -Force }
    }
    Write-Host "SoundFont verified: $Font"
}
Copy-Item -LiteralPath $License -Destination (Join-Path $Cache (Split-Path -Leaf $License)) -Force

if ($Dest) {
    New-Item -ItemType Directory -Force -Path $Dest | Out-Null
    Copy-Item -LiteralPath $Font -Destination (Join-Path $Dest $File) -Force
    Copy-Item -LiteralPath $License -Destination (Join-Path $Dest (Split-Path -Leaf $License)) -Force
    Write-Host "Staged $File into $Dest"
}
