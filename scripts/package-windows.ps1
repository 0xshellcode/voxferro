# Builds a portable Windows package: target\dist\Voxferro\ with voxferro.exe
# (the icon is embedded by crates/app/build.rs) and an assets\ folder, zipped
# to target\dist\Voxferro-<version>-windows-x64.zip.
#
# The the reference editor sample models and environment maps (assets\vox, assets\ibl)
# are third-party and not in git. They are copied only with
# -WithReferenceEditorAssets, for a package you keep for yourself: do not
# redistribute them.
#
# The app prefers Vulkan; on machines without a Vulkan driver it uses
# DirectX 12, whose Render mode needs DXC (src/windows_graphics.rs), so the
# package carries it as the fallback. -WithDxc downloads Microsoft's
# DirectXShaderCompiler release (pinned, SHA-256 checked) and puts
# dxcompiler.dll and dxil.dll, with their licences, next to voxferro.exe.
# dxil.dll is under a Microsoft licence with distribution requirements
# (see LICENSE-MS.txt): read it before sharing a package that includes it.
#
# Usage: powershell -ExecutionPolicy Bypass -File scripts\package-windows.ps1 [-WithDxc] [-WithReferenceEditorAssets]
param([switch]$WithReferenceEditorAssets, [switch]$WithDxc)

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
Copy-Item -Recurse "assets\scripts" (Join-Path $app "assets")
Copy-Item "assets\materials_demo.vox" (Join-Path $app "assets")
Copy-Item "LICENSE-MIT", "LICENSE-APACHE", "README.md" $app

if ($WithReferenceEditorAssets) {
    foreach ($dir in "vox", "ibl") {
        if (Test-Path "assets\$dir") {
            Copy-Item -Recurse "assets\$dir" (Join-Path $app "assets")
        }
    }
}

if ($WithDxc) {
    $dxcUrl = "https://github.com/microsoft/DirectXShaderCompiler/releases/download/v1.9.2607/dxc_2026_07_29.zip"
    $dxcHash = "A1DFB116BA3EEAE6A1582291B53A8E7BF65AD760676BD3194685C8F7367CD241"
    $dxcZip = Join-Path $dist "dxc_2026_07_29.zip"
    if (-not (Test-Path $dxcZip)) { Invoke-WebRequest -Uri $dxcUrl -OutFile $dxcZip }
    $actual = (Get-FileHash -Algorithm SHA256 $dxcZip).Hash
    if ($actual -ne $dxcHash) { Remove-Item $dxcZip; throw "DXC download hash mismatch: $actual" }
    $dxcDir = Join-Path $dist "dxc"
    if (Test-Path $dxcDir) { Remove-Item -Recurse -Force $dxcDir }
    Expand-Archive -Path $dxcZip -DestinationPath $dxcDir
    Copy-Item (Join-Path $dxcDir "bin\x64\dxcompiler.dll"), (Join-Path $dxcDir "bin\x64\dxil.dll") $app
    $licences = Join-Path $app "dxc-licences"
    New-Item -ItemType Directory -Force -Path $licences | Out-Null
    Copy-Item (Join-Path $dxcDir "LICENSE-MS.txt"), (Join-Path $dxcDir "LICENSE-LLVM.txt"), (Join-Path $dxcDir "LICENCE-MIT.txt") $licences
}

$zip = Join-Path $dist "Voxferro-$version-windows-x64.zip"
if (Test-Path $zip) { Remove-Item -Force $zip }
Compress-Archive -Path $app -DestinationPath $zip
Write-Host "built $zip"
