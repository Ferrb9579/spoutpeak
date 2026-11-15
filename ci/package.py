"""Create and validate native desktop packages. Run after cargo build --release."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import plistlib
import re
import shutil
import subprocess
import tarfile
import tempfile
import zipfile

from fetch_ffmpeg import ROOT, download, fetch, verified_download

DIST = ROOT / "dist"
RELEASE = ROOT / "target" / os.environ.get("CARGO_BUILD_TARGET", "") / "release"
HOMEPAGE = "https://github.com/Ferrb9579/spoutpeak"


def run(*args, **kwargs):
    print("+ " + " ".join(map(str, args)), flush=True)
    return subprocess.run(list(map(str, args)), check=True, **kwargs)


def version():
    return re.search(r'^version\s*=\s*"([^"]+)"',
                     (ROOT / "Cargo.toml").read_text(), re.M).group(1)


def stage(platform):
    path = ROOT / "target" / "package" / platform
    # Never delete a caller-supplied or out-of-workspace location.
    if not path.resolve().is_relative_to((ROOT / "target" / "package").resolve()):
        raise RuntimeError("Invalid staging path")
    if path.exists():
        shutil.rmtree(path)
    path.mkdir(parents=True)
    return path


def copy_notices(dest):
    shutil.copytree(ROOT / "assets" / "icons", dest / "licenses" / "icons")


def verify_media(directory):
    """Exercise the relocated helpers, rather than accidentally using PATH's FFmpeg."""
    suffix = ".exe" if os.name == "nt" else ""
    ffmpeg, ffprobe = [directory / (tool + suffix) for tool in ("ffmpeg", "ffprobe")]
    env = {key: value for key, value in os.environ.items() if not key.startswith("DYLD_")}
    env["PATH"] = os.defpath
    for tool in (ffmpeg, ffprobe):
        run(tool, "-version", stdout=subprocess.DEVNULL, env=env)
    filters = subprocess.check_output([str(ffmpeg), "-hide_banner", "-filters"], env=env, text=True)
    if "zscale" not in filters:
        raise RuntimeError("The bundled FFmpeg must support zscale for HDR playback")
    with tempfile.TemporaryDirectory(prefix="spoutpeak media ") as temp:
        clip = Path(temp) / "sample.mkv"
        run(ffmpeg, "-v", "error", "-nostdin", "-f", "lavfi", "-i", "color=s=64x36:r=10",
            "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000", "-t", "0.3",
            "-c:v", "mpeg4", "-c:a", "aac", "-y", clip, env=env)
        probe = json.loads(subprocess.check_output([
            str(ffprobe), "-v", "error", "-show_streams", "-of", "json", str(clip)], env=env))
        kinds = {stream["codec_type"] for stream in probe["streams"]}
        if not {"video", "audio"}.issubset(kinds):
            raise RuntimeError("Media smoke test did not produce both audio and video")
        run(ffmpeg, "-v", "error", "-nostdin", "-i", clip, "-f", "null", "-", env=env)


def windows(work, name, app_version):
    payload = work / name
    payload.mkdir()
    shutil.copy2(RELEASE / "spoutpeak.exe", payload)
    media = fetch("windows", ROOT / "vendor" / "ffmpeg")
    shutil.copytree(media, payload / "ffmpeg")
    copy_notices(payload)
    verify_media(payload / "ffmpeg")
    shutil.make_archive(str(DIST / name), "zip", work, name)
    candidates = [shutil.which("ISCC.exe")]
    for env_name in ("ProgramFiles(x86)", "ProgramFiles", "LOCALAPPDATA"):
        base = Path(os.environ.get(env_name, "C:/missing"))
        candidates.extend([base / "Inno Setup 6" / "ISCC.exe",
                           base / "Programs" / "Inno Setup 6" / "ISCC.exe"])
    compiler = next((p for p in candidates if p and Path(p).is_file()), None)
    if compiler is None:
        raise RuntimeError("Install Inno Setup 6 to create the Windows installer")
    run(compiler, "/DAppVersion=" + app_version, "/DAppBinaryDir=" + str(RELEASE),
        ROOT / "installer" / "spoutpeak.iss")
    installer = DIST / ("Spoutpeak-Setup-" + app_version + ".exe")
    if not installer.is_file():
        raise RuntimeError("Inno Setup did not produce the expected installer")


def linux(work, name, app_version):
    payload = work / "payload"
    payload.mkdir()
    shutil.copy2(RELEASE / "spoutpeak", payload)
    fetch("linux", payload / "ffmpeg")
    copy_notices(payload)
    verify_media(payload / "ffmpeg")
    desktop = work / "spoutpeak.desktop"
    desktop.write_text("[Desktop Entry]\nType=Application\nName=Spoutpeak\n"
                       "Comment=Addon-based media center\nExec=spoutpeak\nIcon=spoutpeak\n"
                       "Terminal=false\nCategories=AudioVideo;Video;Player;\n", encoding="utf-8")
    run("desktop-file-validate", desktop)
    launcher = work / "spoutpeak-launcher"
    launcher.write_text('#!/bin/sh\nexec /opt/spoutpeak/spoutpeak "$@"\n', encoding="utf-8")
    launcher.chmod(0o755)

    nfpm_version = "2.49.0"
    archive = verified_download("https://github.com/goreleaser/nfpm/releases/download/v" + nfpm_version,
                                "nfpm_" + nfpm_version + "_Linux_x86_64.tar.gz", "checksums.txt")
    with tarfile.open(archive) as bundle:
        with bundle.extractfile("nfpm") as source, (work / "nfpm").open("wb") as dest:
            shutil.copyfileobj(source, dest)
    (work / "nfpm").chmod(0o755)
    contents = [{"src": str(p), "dst": "/opt/spoutpeak/" + p.relative_to(payload).as_posix()}
                for p in payload.rglob("*") if p.is_file()]
    contents.extend([
        {"src": str(launcher), "dst": "/usr/bin/spoutpeak"},
        {"src": str(desktop), "dst": "/usr/share/applications/spoutpeak.desktop"},
        {"src": str(RELEASE / "spoutpeak.png"),
         "dst": "/usr/share/icons/hicolor/1024x1024/apps/spoutpeak.png"},
    ])
    common = {"name": "spoutpeak", "arch": "amd64", "platform": "linux", "version": app_version,
              "maintainer": "Spoutpeak contributors <noreply@github.com>", "homepage": HOMEPAGE,
              "description": "A native, addon-based media center", "section": "video",
              "priority": "optional", "contents": contents}
    dependencies = {
        "deb": ["libc6 (>= 2.35)", "libasound2", "libfontconfig1", "libfreetype6", "libx11-6",
                "libxkbcommon0", "libxkbcommon-x11-0", "libxcb1", "libwayland-client0", "libudev1",
                "libgcc-s1", "libstdc++6", "libgl1", "libegl1", "libvulkan1", "ca-certificates"],
        "rpm": ["glibc >= 2.35", "alsa-lib", "fontconfig", "freetype", "libX11", "libxkbcommon",
                "libxkbcommon-x11", "libxcb", "wayland-libs", "systemd-libs", "libgcc", "libstdc++",
                "libglvnd-glx", "libglvnd-egl", "vulkan-loader", "ca-certificates"],
    }
    for packager, depends in dependencies.items():
        config = dict(common, depends=depends)
        config_path = work / (packager + ".json")
        config_path.write_text(json.dumps(config, indent=2), encoding="utf-8")
        output = DIST / (name + "." + packager)
        run(work / "nfpm", "package", "--config", config_path, "--packager", packager, "--target", output)
        run(*(["dpkg-deb", "--info", output] if packager == "deb" else ["rpm", "-qpi", output]))

    appdir = work / "AppDir"
    shutil.copytree(payload, appdir / "usr" / "bin")
    tool = work / "linuxdeploy.AppImage"
    # Pin the tool release and verify its digest before executing it.
    tool.write_bytes(download("https://github.com/linuxdeploy/linuxdeploy/releases/download/"
                              "1-alpha-20251107-1/linuxdeploy-x86_64.AppImage"))
    if hashlib.sha256(tool.read_bytes()).hexdigest() != "c20cd71e3a4e3b80c3483cef793cda3f4e990aca14014d23c544ca3ce1270b4d":
        raise RuntimeError("linuxdeploy checksum mismatch")
    tool.chmod(0o755)
    output = DIST / (name + ".AppImage")
    env = dict(os.environ, APPIMAGE_EXTRACT_AND_RUN="1", ARCH="x86_64", VERSION=app_version,
               OUTPUT=str(output), NO_STRIP="1")
    run(tool, "--appdir", appdir, "--executable", appdir / "usr" / "bin" / "spoutpeak",
        "--desktop-file", desktop, "--icon-file", RELEASE / "spoutpeak.png", "--output", "appimage",
        cwd=work, env=env)
    if not output.is_file():
        raise RuntimeError("linuxdeploy did not produce the expected AppImage")
    with tarfile.open(DIST / (name + ".tar.gz"), "w:gz") as bundle:
        bundle.add(appdir, arcname=name)
    extracted = work / "extracted"
    extracted.mkdir()
    run(output, "--appimage-extract", cwd=extracted, stdout=subprocess.DEVNULL)
    # Validate the actual AppImage payload, not just the build output.
    executable = extracted / "squashfs-root" / "usr" / "bin" / "spoutpeak"
    linkage = subprocess.check_output(["ldd", str(executable)], text=True)
    if "not found" in linkage:
        raise RuntimeError("Missing packaged library:\n" + linkage)


def fetch_macos_media(dest):
    metadata = json.loads((ROOT / "ci" / "macos-ffmpeg.json").read_text())
    spec = metadata["arm64"]
    for tool in ("ffmpeg", "ffprobe"):
        data = download(spec["base"] + "/" + tool + ".zip")
        if hashlib.sha256(data).hexdigest() != spec[tool]:
            raise RuntimeError("Mac FFmpeg checksum mismatch for " + tool)
        import io
        with zipfile.ZipFile(io.BytesIO(data)) as bundle:
            entry = next(p for p in bundle.namelist() if Path(p).name == tool)
            (dest / tool).write_bytes(bundle.read(entry))
            (dest / tool).chmod(0o755)
    notices = dest / "licenses"
    notices.mkdir()
    (notices / "versions.txt").write_bytes(download(spec["base"] + "/versions.txt"))
    (notices / "COPYING.GPLv3").write_bytes(download(
        "https://raw.githubusercontent.com/FFmpeg/FFmpeg/n9.0.2/COPYING.GPLv3"))
    (notices / "sources.json").write_text(json.dumps(metadata, indent=2), encoding="utf-8")
    (notices / "SOURCE.txt").write_text(
        "Static FFmpeg 9.0.2 GPL build by Martin Riedl.\n"
        "Source and dependency build recipes: " + metadata["source"] + "\n"
        "FFmpeg source: https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz\n"
        "Included library versions and configure flags: versions.txt\n", encoding="utf-8")


def macos(work, name, app_version):
    app = work / "Spoutpeak.app"
    contents = app / "Contents"
    binaries, resources = contents / "MacOS", contents / "Resources"
    binaries.mkdir(parents=True)
    resources.mkdir()
    shutil.copy2(RELEASE / "spoutpeak", binaries)
    media = binaries / "ffmpeg"
    media.mkdir()
    fetch_macos_media(media)
    copy_notices(resources)
    iconset = work / "spoutpeak.iconset"
    iconset.mkdir()
    for size in (16, 32, 128, 256, 512):
        for scale in (1, 2):
            output = iconset / ("icon_{}x{}{}.png".format(size, size, "@2x" if scale == 2 else ""))
            run("sips", "-z", size * scale, size * scale, RELEASE / "spoutpeak.png", "--out", output,
                stdout=subprocess.DEVNULL)
    run("iconutil", "-c", "icns", iconset, "-o", resources / "spoutpeak.icns")
    with (contents / "Info.plist").open("wb") as dest:
        plistlib.dump({"CFBundleName": "Spoutpeak", "CFBundleDisplayName": "Spoutpeak",
                      "CFBundleIdentifier": "io.github.Ferrb9579.spoutpeak", "CFBundleExecutable": "spoutpeak",
                      "CFBundleIconFile": "spoutpeak.icns", "CFBundlePackageType": "APPL",
                      "CFBundleShortVersionString": app_version, "CFBundleVersion": app_version,
                      "LSMinimumSystemVersion": "15.0", "NSHighResolutionCapable": True}, dest)
    # Ad-hoc signing makes CI bundles runnable without distribution secrets.
    for file in [media / "ffmpeg", media / "ffprobe", binaries / "spoutpeak"]:
        run("lipo", "-verify_arch", "arm64", file)
        linkage = subprocess.check_output(["otool", "-L", str(file)], text=True)
        if "/opt/homebrew/" in linkage or "/usr/local/" in linkage:
            raise RuntimeError("Unexpected external dependency:\n" + linkage)
        run("codesign", "--force", "--sign", "-", file)
    run("codesign", "--force", "--sign", "-", app)
    run("codesign", "--verify", "--deep", "--strict", app)
    # Move the helpers to a path with spaces and no custom loader variables.
    with tempfile.TemporaryDirectory(prefix="Spoutpeak relocated ") as temp:
        relocated = Path(temp) / "ffmpeg"
        shutil.copytree(media, relocated)
        verify_media(relocated)
    run("ditto", "-c", "-k", "--sequesterRsrc", "--keepParent", app, DIST / (name + ".zip"))
    image = work / "dmg"
    image.mkdir()
    shutil.copytree(app, image / app.name)
    (image / "Applications").symlink_to("/Applications")
    run("hdiutil", "create", "-volname", "Spoutpeak", "-srcfolder", image, "-ov", "-format", "UDZO",
        DIST / (name + ".dmg"))
    run("hdiutil", "verify", DIST / (name + ".dmg"))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--platform", choices=("windows", "linux", "macos"), required=True)
    parser.add_argument("--arch", choices=("x86_64", "arm64"), required=True)
    args = parser.parse_args()
    if args.arch == "arm64" and args.platform != "macos":
        parser.error("Windows and Linux packages currently target x86_64")
    if args.platform == "macos" and args.arch != "arm64":
        parser.error("macOS packages target Apple Silicon (arm64)")
    app_version = version()
    if os.environ.get("GITHUB_REF_TYPE") == "tag" and os.environ.get("GITHUB_REF_NAME") != "v" + app_version:
        raise RuntimeError("Release tag must match Cargo.toml version: v" + app_version)
    DIST.mkdir(exist_ok=True)
    name = "spoutpeak-{}-{}-{}".format(app_version, args.platform, args.arch)
    globals()[args.platform](stage(args.platform), name, app_version)
    outputs = sorted(p for p in DIST.iterdir() if p.is_file() and p.suffix != ".sha256")
    checksum = DIST / (name + ".sha256")
    checksum.write_text("".join(hashlib.sha256(p.read_bytes()).hexdigest() + "  " + p.name + "\n"
                                for p in outputs), encoding="utf-8")


if __name__ == "__main__":
    main()
