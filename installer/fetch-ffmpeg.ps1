# Downloads the LGPL ffmpeg build that the installer bundles into vendor\ffmpeg.
# Run from the repo root: pwsh installer\fetch-ffmpeg.ps1
$ErrorActionPreference = 'Stop'

$name = 'ffmpeg-n9.0-latest-win64-lgpl-shared-9.0'
$url = "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/$name.zip"
$root = Split-Path $PSScriptRoot -Parent
$dest = Join-Path $root 'vendor\ffmpeg'
$zip = Join-Path $env:TEMP "$name.zip"
$unpacked = Join-Path $env:TEMP $name

Write-Host "Downloading $url"
Invoke-WebRequest $url -OutFile $zip
if (Test-Path $unpacked) { Remove-Item $unpacked -Recurse -Force }
Expand-Archive $zip -DestinationPath $env:TEMP -Force

if (Test-Path $dest) { Remove-Item $dest -Recurse -Force }
New-Item -ItemType Directory -Path $dest | Out-Null
# ffmpeg.exe, ffprobe.exe and the shared DLLs; ffplay and dev files are not needed.
Get-ChildItem (Join-Path $unpacked 'bin') -Include 'ffmpeg.exe', 'ffprobe.exe', '*.dll' -Recurse |
    Copy-Item -Destination $dest
Copy-Item (Join-Path $unpacked 'LICENSE.txt') (Join-Path $dest 'LICENSE.txt')

Remove-Item $zip, $unpacked -Recurse -Force
& (Join-Path $dest 'ffmpeg.exe') -hide_banner -version | Select-Object -First 1
Write-Host "ffmpeg is ready in $dest"
