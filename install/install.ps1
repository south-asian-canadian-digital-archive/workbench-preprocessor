# Install (or update) organise, the SACDA ingest tool, on Windows.
#
#   irm https://github.com/south-asian-canadian-digital-archive/workbench-preprocessor/releases/latest/download/install.ps1 | iex
#
# Picks the build for this machine from GitHub releases, checks its SHA-256,
# puts organise.exe in %LOCALAPPDATA%\sacda\bin (and on your user PATH),
# remembers where the g\ configs folder is, and finishes with `organise doctor`.
# Safe to re-run.
#
# Options: set these before piping into iex -
#   $OrganiseVersion = "v0.2.0"   pin a release (default: latest)
#   $SacdaG = "C:\...\g"          skip the g\ search

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$Repo = "south-asian-canadian-digital-archive/workbench-preprocessor"
$BinDir = Join-Path $env:LOCALAPPDATA "sacda\bin"
$Version = if ($OrganiseVersion) { $OrganiseVersion } else { "latest" }

function Say($m) { Write-Host "  $m" }
function Die($m) { Write-Host "  ! $m" -ForegroundColor Yellow; throw $m }

$arch = $env:PROCESSOR_ARCHITECTURE
if ($arch -ne "AMD64" -and $arch -ne "ARM64") { Die "unsupported CPU '$arch'" }
# ARM64 Windows runs the x86_64 build under emulation.
$asset = "workbench-preprocessor-windows-x86_64.zip"
$base = if ($Version -eq "latest") { "https://github.com/$Repo/releases/latest/download" }
        else { "https://github.com/$Repo/releases/download/$Version" }

Write-Host "`nInstalling organise ($Version, windows)" -ForegroundColor Cyan

$tmp = Join-Path $env:TEMP "organise-$([guid]::NewGuid().ToString('N'))"
New-Item -ItemType Directory -Force -Path $tmp | Out-Null
try {
    $zip = Join-Path $tmp $asset
    try { Invoke-WebRequest "$base/$asset" -OutFile $zip -UseBasicParsing }
    catch { Die "couldn't download $asset from $base - check the internet connection" }

    try {
        $sums = (Invoke-WebRequest "$base/SHASUMS256.txt" -UseBasicParsing).Content
        $line = ($sums -split "`n") | Where-Object { $_ -match [regex]::Escape($asset) + '\s*$' } | Select-Object -First 1
        if (-not $line) { Die "SHASUMS256.txt doesn't list $asset" }
        $want = ($line -split '\s+')[0].ToLower()
        $got = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
        if ($want -ne $got) { Die "checksum mismatch for $asset - download corrupted? re-run" }
        Say "checksum ok"
    }
    catch [System.Net.WebException] { Say "no SHASUMS256.txt on this release - skipping the checksum" }

    $x = Join-Path $tmp "x"
    Expand-Archive -Path $zip -DestinationPath $x -Force
    $exe = Get-ChildItem $x -Recurse -Filter "organise.exe" | Select-Object -First 1
    if (-not $exe) { Die "$asset has no organise.exe" }
    New-Item -ItemType Directory -Force -Path $BinDir | Out-Null
    Copy-Item $exe.FullName (Join-Path $BinDir "organise.exe") -Force
    Say "installed -> $BinDir\organise.exe"
}
finally { Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue }

# --- PATH (user scope, no admin needed) --------------------------------------
$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
if (-not (($userPath -split ';') -contains $BinDir)) {
    [Environment]::SetEnvironmentVariable("Path", "$BinDir;$userPath", "User")
    Say "added $BinDir to your PATH (new windows pick it up)"
}
$env:Path = "$BinDir;$env:Path"

# --- where's g\? ---------------------------------------------------------------
function Test-G($p) { $p -and (Test-Path (Join-Path $p "config\sacda.toml")) }
$g = $SacdaG
if (-not (Test-G $g)) {
    $g = @(
        (Get-Location).Path
        (Join-Path (Get-Location).Path "g")
        "$env:USERPROFILE\Desktop\islandora_workbench\g"
        "$env:USERPROFILE\Documents\GitHub\islandora_workbench\g"
        "$env:USERPROFILE\islandora_workbench\g"
    ) | Where-Object { Test-G $_ } | Select-Object -First 1
}
if (-not $g) {
    $typed = (Read-Host "  Where is the workbench g folder (has config\sacda.toml)? Leave empty to skip").Trim().Trim('"')
    if (Test-G $typed) { $g = (Resolve-Path $typed).Path }
}
$cfgDir = Join-Path $env:APPDATA "sacda"
if ($g) {
    New-Item -ItemType Directory -Force -Path $cfgDir | Out-Null
    # TOML literal string: backslashes stay as typed.
    Set-Content -Path (Join-Path $cfgDir "organise.toml") -Encoding utf8 -Value @(
        "# written by the organise installer"
        "g_root = '$g'"
    )
    Say "configs: $g (saved in $cfgDir\organise.toml)"
}
else { Say "no g folder found - organise will look for one from wherever you run it" }

Write-Host ""
& (Join-Path $BinDir "organise.exe") doctor
Write-Host "`nDone. Run 'organise' in the folder with your spreadsheet.`n" -ForegroundColor Green
