# Continuous integration

[CI](../.github/workflows/ci.yml) runs on pushes, pull requests, and manual
dispatches. Every desktop job runs Clippy, all application tests, a release
build, packaging tests, and a bundled FFmpeg encode/probe/decode smoke test.
Cargo dependencies use the committed lockfile; build caches are separate for
each platform and architecture. In-progress runs are cancelled when replaced.

| Runner | Architecture | Downloadable artifacts |
| --- | --- | --- |
| Windows 2022 | x86_64 | Inno Setup installer, portable ZIP |
| Ubuntu 22.04 | x86_64 | DEB, RPM, AppImage, portable AppDir TAR.GZ |
| macOS 15 | Apple Silicon | APP bundle in ZIP, DMG |

Open a workflow run in GitHub Actions and download its platform artifact.
Each includes SHA-256 checksums. Artifact retention is 14 days. Packages are
built for pull requests too; this workflow does not publish GitHub releases.
Windows uses the static MSVC runtime. Linux packages require glibc 2.35 or
newer (Ubuntu 22.04 or equivalent) and the listed package dependencies.
macOS bundles target macOS 15 or newer.
AppImages bundle application libraries, while graphics drivers remain supplied
by the operating system. After downloading, use `chmod +x *.AppImage` to run one.

The Linux job also extracts the produced AppImage and starts it under Xvfb
with Mesa software rendering. It uploads a screenshot as `linux-startup`.
This checks startup, not interactive playback on a hardware GPU. A separate
job builds/tests the watch server and checks its HTTP health endpoint.

Application tests cover tooltip coordinates on translated/scaled rails, screen
edge positioning, preview cache limits, and cancellation of stale frames.
Chinese, Japanese, and Thai text layout tests fail on missing segmentation
models. With FFmpeg and FFprobe on PATH, run
`cargo test real_decoder_returns_frames -- --ignored`
to generate a test video and check actual preview-frame pixels.
Full-screen screenshot baseline comparisons are not currently configured.

Windows/Linux include checksum-verified FFmpeg 9.0 LGPL builds from
[FFmpeg-Builds](https://github.com/FFmpeg-Builds/FFmpeg-Builds). macOS uses
[Martin Riedl's static FFmpeg 9.0.2 GPL builds](https://ffmpeg.martin-riedl.de/),
with pinned URLs and SHA-256 hashes in `macos-ffmpeg.json`. License files and
source/build metadata are included. Media tests run against the packaged
helper paths; macOS also tests after relocating the helpers. Platform icons are
generated from `assets/logo.svg` during the Cargo build.

Packages are CI builds without a distribution certificate. macOS bundles
are ad-hoc signed for local execution, with no Apple notarization. Public
distribution signing/notarization can be added when certificates are available.

Set the optional repository **Actions variable** `SPOUTPEAK_WATCH_URL` to the
public `wss://.../ws` endpoint to bake into all desktop builds. No secrets are
required for CI. Package versions come from the root `Cargo.toml`; a version
tag must match it exactly, such as `v0.1.0`.

To reproduce a package on its native platform:

```sh
cargo test --locked --release --all-targets
cargo build --locked --release
python -m unittest discover -s ci -p 'test_*.py' -v
python ci/package.py --platform linux --arch x86_64
```

Use `windows` on Windows with Inno Setup 6 installed. On macOS, use `macos`
with `arm64`; no Homebrew dependencies are needed. Linux system
dependencies are listed in the workflow. The package
script honors `CARGO_BUILD_TARGET` when an explicit Rust target is used.
