# Renders the app icon (assets\icon\icon.png and icon-square.png) from its
# model, a list of editor commands (assets\icon\icon.cmds), with the path
# tracer, then writes assets\icon\icon.ico for the Windows executable.
# The PowerShell twin of make-icon.sh.
$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..")

cargo build -p voxferro
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

$voxferro = "target\debug\voxferro.exe"
& $voxferro --script assets\icon\icon.cmds `
    --run "camera frame" --run "camera zoom 1.05" --camera -55 28 `
    --render 512 --icon assets\icon\icon.png
if ($LASTEXITCODE -ne 0) { throw "rendering the icon failed" }

& $voxferro --make-ico assets\icon\icon.png assets\icon\icon.ico
if ($LASTEXITCODE -ne 0) { throw "writing the .ico failed" }
