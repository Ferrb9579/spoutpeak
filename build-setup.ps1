pwsh installer\fetch-ffmpeg.ps1   # only on a new machine or to update ffmpeg
cargo build --release
& "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe" installer\spoutpeak.iss
