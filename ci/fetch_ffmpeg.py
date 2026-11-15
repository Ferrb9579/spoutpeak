"""Fetch the same LGPL FFmpeg release line on Windows and Linux, with SHA-256 verification."""
import hashlib
from pathlib import Path
import shutil
import subprocess
import tarfile
import zipfile

ROOT = Path(__file__).resolve().parents[1]
DOWNLOADS = ROOT / "target" / "ci-downloads"
FFMPEG_RELEASE = "https://github.com/FFmpeg-Builds/FFmpeg-Builds/releases/download/latest"


def download(url):
    return subprocess.check_output([
        "curl", "--fail", "--location", "--silent", "--show-error", "--retry", "3",
        "--connect-timeout", "15", "--max-time", "600", "--user-agent", "Spoutpeak-CI", url,
    ])


def verified_download(base, filename, manifest):
    checksums = download(base + "/" + manifest).decode()
    expected = next((line.split()[0] for line in checksums.splitlines()
                     if len(line.split()) == 2 and line.split()[1].lstrip("*") == filename), None)
    if expected is None:
        raise RuntimeError("Missing checksum for " + filename)
    DOWNLOADS.mkdir(parents=True, exist_ok=True)
    path = DOWNLOADS / filename
    if not path.exists() or hashlib.sha256(path.read_bytes()).hexdigest() != expected:
        print("Downloading " + filename, flush=True)
        data = download(base + "/" + filename)
        if hashlib.sha256(data).hexdigest() != expected:
            raise RuntimeError("Checksum mismatch for " + filename)
        path.write_bytes(data)
    return path


def fetch(platform, dest):
    if platform not in ("windows", "linux"):
        raise ValueError("FFmpeg download supports Windows and Linux x64")
    name = ("ffmpeg-n9.0-latest-win64-lgpl-shared-9.0.zip" if platform == "windows"
            else "ffmpeg-n9.0-latest-linux64-lgpl-9.0.tar.xz")
    archive = verified_download(FFMPEG_RELEASE, name, "checksums.sha256")
    dest.mkdir(parents=True, exist_ok=True)
    # Copy only the required basenames, never archive paths or symlinks.
    def copy_entry(name, stream):
        parts = Path(name).parts
        leaf = parts[-1]
        binary = "bin" in parts and (leaf in ("ffmpeg", "ffprobe", "ffmpeg.exe", "ffprobe.exe")
                                      or leaf.lower().endswith(".dll"))
        notice = leaf.lower().startswith(("license", "copying"))
        if binary or notice:
            with (dest / leaf).open("wb") as output:
                shutil.copyfileobj(stream, output)
            if binary and platform != "windows":
                (dest / leaf).chmod(0o755)
    if platform == "windows":
        with zipfile.ZipFile(archive) as bundle:
            for item in bundle.infolist():
                if not item.is_dir():
                    with bundle.open(item) as stream:
                        copy_entry(item.filename, stream)
    else:
        with tarfile.open(archive) as bundle:
            for item in bundle:
                if item.isfile():
                    with bundle.extractfile(item) as stream:
                        copy_entry(item.name, stream)
    (dest / "SOURCE.txt").write_text(
        "FFmpeg LGPL build: " + FFMPEG_RELEASE + "/" + name + "\n"
        "Build recipes and corresponding source instructions:\n"
        "https://github.com/FFmpeg-Builds/FFmpeg-Builds\nhttps://ffmpeg.org/download.html\n",
        encoding="utf-8")
    suffix = ".exe" if platform == "windows" else ""
    for tool in ("ffmpeg", "ffprobe"):
        if not (dest / (tool + suffix)).is_file():
            raise RuntimeError("Missing bundled " + tool)
    return dest


if __name__ == "__main__":
    import argparse
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--platform", choices=("windows", "linux"), required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    fetch(args.platform, args.output)
