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

## Crash diagnostics

Logging starts before the native window or renderer. On Windows, logs are in
`%LOCALAPPDATA%\spoutpeak\logs`; if that directory is unwritable, the app tries
`%TEMP%\spoutpeak-logs`. Settings → Diagnostics opens the active directory or
exports a ZIP to Downloads. An app that cannot open its window can still run
`spoutpeak.exe --export-diagnostics`, or the log folder can be copied manually.
`--open-logs` opens the folder without creating the UI. Reports are never uploaded.

Each session records UTC timestamps, version/revision, OS/architecture, startup
stages, renderer/dependency warnings, playback settings/errors and FFmpeg output.
OS, CPU, memory and GPU driver details are collected in the background. A
`.status.json` records startup and clean/error shutdown; a leftover `running`
status can also mean forced termination, power loss, or an open instance.
Rust panics save a `.panic.txt` with a forced backtrace, including worker threads.
Windows unhandled exceptions attempt a `.native.txt` and a small `.dmp` from a
dedicated writer thread. This is best-effort; forced kills and some severe
process failures cannot invoke a crash handler.

Logs rotate at 4 MiB with one previous segment per session. The most recent
eight sessions and three native dumps are retained. Exports include diagnostic
text only: no profiles, addon configuration, media, or binary memory dumps.
URL credentials, paths/queries and common secret fields are redacted; review
the report before sharing it. Native dumps stay local for optional analysis.
Set `SPOUTPEAK_LOG=debug` for more detail (`RUST_LOG` is also supported), or
`SPOUTPEAK_LOG_DIR` to choose another log directory.

Keep `spoutpeak.pdb` with the exact Windows executable distributed to users.
Release builds retain debug symbols, and CI saves matching PDBs in a separate
`spoutpeak-windows-symbols-<commit>` artifact for 90 days. Download and archive
those symbols for releases supported longer than that. Dumps from one build
cannot be reliably symbolized with a different build's PDB.

`cargo test diagnostics::tests` verifies rotation, retention, redaction, ZIP
contents, panic backtraces and (on Windows) real native-exception minidumps in
isolated child processes. It does not crash the running desktop app.
