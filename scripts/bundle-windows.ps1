# Builds Ond for Windows and packages it as a .zip in target\bundle:
#
#   pwsh scripts/bundle-windows.ps1
#
# The zip holds Ond.exe (icon and version info embedded by crates/app/build.rs)
# and the licences. No installer yet: unzip anywhere and run Ond.exe.
$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..")

# "path+file:///…/crates/app#ond-player@0.1.0" → "0.1.0"
$version = (cargo pkgid -p ond-player).Split("@")[-1]
if (-not $version -or $version.Contains("#")) { throw "could not read the ond-player package version from cargo pkgid" }

cargo build --release -p ond-player
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

$arch = if ($env:PROCESSOR_ARCHITECTURE -eq "ARM64") { "arm64" } else { "x86_64" }
$name = "Ond-$version-windows-$arch"
$out = "target\bundle"
$stage = Join-Path $out $name
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
New-Item -ItemType Directory -Force $stage | Out-Null

Copy-Item target\release\Ond.exe $stage
Copy-Item README.md, LICENSE-MIT, LICENSE-APACHE $stage

$zip = Join-Path $out "$name.zip"
if (Test-Path $zip) { Remove-Item -Force $zip }
Compress-Archive -Path $stage -DestinationPath $zip
Remove-Item -Recurse -Force $stage
Write-Output "built: $zip"
