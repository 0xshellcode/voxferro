# Builds a portable Windows package: target\dist\Voxferro\ with voxferro.exe
# (the icon is embedded by crates/app/build.rs) and an assets\ folder, zipped
# to target\dist\Voxferro-<version>-windows-x64.zip.
#
# The the reference editor sample models and environment maps (assets\vox, assets\ibl)
# are third-party and not in git. They are copied only with
# -WithReferenceEditorAssets, for a package you keep for yourself: do not
# redistribute them.
#
# Usage: powershell -ExecutionPolicy Bypass -File scripts\package-windows.ps1
param([switch]$WithReferenceEditorAssets)

$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..")

cargo build --release -p voxferro
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.*)"' | Select-Object -First 1).Matches[0].Groups[1].Value
$dist = Join-Path "target" "dist"
$app = Join-Path $dist "Voxferro"
if (Test-Path $app) { Remove-Item -Recurse -Force $app }
New-Item -ItemType Directory -Force -Path (Join-Path $app "assets") | Out-Null

Copy-Item (Join-Path "target\release" "voxferro.exe") $app
Copy-Item -Recurse "assets\showcase" (Join-Path $app "assets")
Copy-Item "assets\materials_demo.vox" (Join-Path $app "assets")
Copy-Item "LICENSE-MIT", "LICENSE-APACHE", "README.md" $app

if ($WithReferenceEditorAssets) {
    foreach ($dir in "vox", "ibl") {
        if (Test-Path "assets\$dir") {
            Copy-Item -Recurse "assets\$dir" (Join-Path $app "assets")
        }
    }
}

$zip = Join-Path $dist "Voxferro-$version-windows-x64.zip"
if (Test-Path $zip) { Remove-Item -Force $zip }
Compress-Archive -Path $app -DestinationPath $zip
Write-Host "built $zip"
