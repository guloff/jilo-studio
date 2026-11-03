<#
.SYNOPSIS
  Build and package Jilo Studio for Windows 10 and 11 (x64).

.DESCRIPTION
  Produces, in $env:DIST (default: dist/release):
    jilo-studio-<version>-windows-x64-setup.exe      installer (Inno Setup 6): per user, no administrator
    jilo-studio-<version>-windows-x64-portable.zip   jilo-studio.exe + jilo-cli.exe + portable.txt

  The binaries link the C runtime statically (+crt-static), so they need nothing but Windows. Built
  with the Rust toolchain for x86_64-pc-windows-gnu (or -msvc, -Toolchain msvc). The GNU toolchain
  needs MinGW-w64 GCC on PATH (ring's C code and windres for the icon), for example
  `winget install BrechtSanders.WinLibs.POSIX.MSVCRT`; the MSVC one, the Visual Studio Build Tools.

  Needs Inno Setup 6 (`winget install JRSoftware.InnoSetup`); ISCC.exe is looked for in the usual
  places or taken from $env:ISCC. Not signed (no code-signing certificate): SmartScreen warns once.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File packaging/windows/package.ps1
  powershell -ExecutionPolicy Bypass -File packaging/windows/package.ps1 -SkipBuild
#>
param(
  [ValidateSet('gnu', 'msvc')] [string] $Toolchain = 'gnu',
  [switch] $SkipBuild
)
$ErrorActionPreference = 'Stop'
$Root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path

function Invoke-Native([string] $What, [scriptblock] $Block) {
  Write-Output "==> $What"
  & $Block
  if ($LASTEXITCODE -ne 0) { throw "$What failed with exit code $LASTEXITCODE" }
}

# The version lives in one place: [workspace.package] version in the root Cargo.toml.
$Version = $env:JILO_VERSION
if (-not $Version) {
  $inPkg = $false
  foreach ($line in Get-Content (Join-Path $Root 'Cargo.toml')) {
    if ($line -match '^\s*\[') { $inPkg = ($line.Trim() -eq '[workspace.package]'); continue }
    if ($inPkg -and $line -match '^\s*version\s*=\s*"([^"]+)"') { $Version = $Matches[1]; break }
  }
}
if (-not $Version) { throw 'could not read [workspace.package] version from Cargo.toml' }
# Windows file versions are numeric (a.b.c.d); pre-release tags are dropped there.
$FileVersion = ($Version -split '-')[0]

$Target = "x86_64-pc-windows-$Toolchain"
$Dist = if ($env:DIST) { $env:DIST } else { Join-Path $Root 'dist\release' }
$TargetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $Root 'target' }
New-Item -ItemType Directory -Force -Path $Dist | Out-Null

if (-not $env:JILO_BUILD_SHA) { $env:JILO_BUILD_SHA = (git -C $Root rev-parse HEAD 2>$null) }
if (-not $env:JILO_BUILD_DATE) { $env:JILO_BUILD_DATE = (Get-Date).ToUniversalTime().ToString('yyyy-MM-dd') }

Write-Output "Jilo Studio $Version for Windows x64 ($Target)"

if (-not $SkipBuild) {
  # Static CRT: no runtime DLLs to ship. Scoped to the target so host build scripts and
  # proc-macros are unaffected.
  $flagVar = 'CARGO_TARGET_' + ($Target.ToUpper() -replace '-', '_') + '_RUSTFLAGS'
  [Environment]::SetEnvironmentVariable($flagVar, '-C target-feature=+crt-static')
  # Fail the build (rather than warn) if the icon/VERSIONINFO can't be embedded.
  $env:JILO_REQUIRE_WINRES = '1'
  Invoke-Native "cargo build ($Target)" { cargo build --release --locked -p jilo-studio -p jilo-cli --target $Target }
}

$Bin = Join-Path $TargetDir "$Target\release"

# Check both binaries' PE headers before packaging: x64 (0x8664), and the subsystem: 2 = Windows GUI
# (no console window opens with the app), 3 = console (the CLI's output reaches the terminal).
function Get-PeHeader([string] $Path) {
  $bytes = [System.IO.File]::ReadAllBytes($Path)
  $pe = [BitConverter]::ToInt32($bytes, 0x3C)
  return @{ Machine = [BitConverter]::ToUInt16($bytes, $pe + 4); Subsystem = [BitConverter]::ToUInt16($bytes, $pe + 0x5C) }
}
foreach ($check in @(@('jilo-studio.exe', 2), @('jilo-cli.exe', 3))) {
  $h = Get-PeHeader (Join-Path $Bin $check[0])
  if ($h.Machine -ne 0x8664) { throw "$($check[0]) is for machine 0x$('{0:X}' -f $h.Machine), expected 0x8664 (x64)" }
  if ($h.Subsystem -ne $check[1]) { throw "$($check[0]) has PE subsystem $($h.Subsystem), expected $($check[1])" }
  Write-Output "ok $($check[0]): x64, PE subsystem $($h.Subsystem)"
}

# Only system DLLs: a MinGW runtime DLL (libgcc_s, libwinpthread, libstdc++) would be missing on
# other machines.
$Objdump = Get-Command objdump -ErrorAction SilentlyContinue
if ($Objdump) {
  foreach ($exe in 'jilo-studio.exe', 'jilo-cli.exe') {
    $dlls = & $Objdump.Source -p (Join-Path $Bin $exe) | Select-String 'DLL Name: (.+)' | ForEach-Object { $_.Matches[0].Groups[1].Value.Trim() }
    $foreign = $dlls | Where-Object { $_ -match '^(libgcc|libwinpthread|libstdc\+\+|vcruntime|msvcp)' }
    if ($foreign) { throw "$exe needs DLLs that Windows does not have: $($foreign -join ', ')" }
    Write-Output "ok ${exe}: imports only system DLLs ($($dlls.Count))"
  }
}

$Stage = Join-Path $TargetDir 'windows-package\x64'
Remove-Item -Recurse -Force $Stage -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $Stage | Out-Null
Copy-Item (Join-Path $Bin 'jilo-studio.exe'), (Join-Path $Bin 'jilo-cli.exe') $Stage

# ---- installer ---------------------------------------------------------------------------------
$Iscc = $env:ISCC
if (-not $Iscc) {
  $Iscc = @(
    (Join-Path $env:LOCALAPPDATA 'Programs\Inno Setup 6\ISCC.exe'),
    (Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6\ISCC.exe'),
    (Join-Path $env:ProgramFiles 'Inno Setup 6\ISCC.exe')
  ) | Where-Object { Test-Path $_ } | Select-Object -First 1
}
if (-not $Iscc) { throw 'Inno Setup 6 (ISCC.exe) not found: winget install JRSoftware.InnoSetup, or set $env:ISCC' }
Invoke-Native 'ISCC (installer)' {
  & $Iscc /Q "/DVersion=$FileVersion" "/DBinDir=$Stage" "/DRoot=$Root" "/DOutDir=$Dist" (Join-Path $PSScriptRoot 'jilo-studio.iss')
}
$Setup = Join-Path $Dist "jilo-studio-$FileVersion-windows-x64-setup.exe"

# ---- portable zip ------------------------------------------------------------------------------
$Portable = Join-Path $TargetDir "windows-package\jilo-studio-$Version-windows-x64-portable"
Remove-Item -Recurse -Force $Portable -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $Portable | Out-Null
Copy-Item (Join-Path $Stage '*.exe') $Portable
# The marker that makes Jilo Studio keep its data in .\data next to the exe instead of %APPDATA%.
Set-Content -Encoding ascii -Path (Join-Path $Portable 'portable.txt') -Value @(
  'Jilo Studio portable: preferences, presets, models, logs and crash recovery are kept in the data',
  'folder next to jilo-studio.exe. Delete this file to use %APPDATA%\com.rustamguloff.jilostudio instead.'
)
foreach ($f in 'README.md', 'LICENSE-MIT', 'LICENSE-APACHE', 'NOTICE', 'ATTRIBUTION.md', 'assets\brand\jilo\LICENSE-brand.txt') {
  $p = Join-Path $Root $f
  if (Test-Path $p) { Copy-Item $p $Portable }
}
$Zip = Join-Path $Dist "jilo-studio-$Version-windows-x64-portable.zip"
Remove-Item -Force $Zip -ErrorAction SilentlyContinue
Compress-Archive -Path $Portable -DestinationPath $Zip

Invoke-Native 'jilo-cli --version' { & (Join-Path $Stage 'jilo-cli.exe') --version }
foreach ($f in $Setup, $Zip) {
  $hash = (Get-FileHash -Algorithm SHA256 $f).Hash.ToLowerInvariant()
  Write-Output ("{0}  {1}  {2:N1} MB" -f $hash, (Split-Path $f -Leaf), ((Get-Item $f).Length / 1MB))
}
