#[path = "src/branding.rs"]
mod branding;

fn main() {
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");
    if let Ok(output) = std::process::Command::new("git").args(["rev-parse", "--short=12", "HEAD"]).output()
        && output.status.success()
    {
        println!("cargo:rustc-env=SPOUTPEAK_BUILD_REVISION={}", String::from_utf8_lossy(&output.stdout).trim());
    }
    println!("cargo:rerun-if-changed=assets/logo.svg");
    println!("cargo:rerun-if-changed=src/branding.rs");
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo OUT_DIR"));
    let profile = profile_dir(&out);
    for (filename, size) in [("spoutpeak.png", 512), ("spoutpeak-macos.png", 1024)] {
        branding::render_pixmap(size, true)
            .expect("valid logo.svg")
            .save_png(profile.join(filename))
            .expect("write packaging icon");
    }
    watch_url();
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        let icon = windows_icon();
        res.set_icon(icon.to_str().expect("UTF-8 icon path"));
        res.set("ProductName", "Spoutpeak");
        res.set("FileDescription", "Spoutpeak");
        res.compile().expect("failed to embed Windows resources");
    }
}

/// Cargo 1.100 nests package names beneath `build/`; older Cargo puts the
/// package and hash in one directory. Find the profile without counting levels.
fn profile_dir(out: &std::path::Path) -> &std::path::Path {
    out.ancestors()
        .find(|path| path.file_name().is_some_and(|name| name == "build"))
        .and_then(std::path::Path::parent)
        .expect("Cargo profile directory above build/")
}

/// Build both the PE resource and installer icon from the same SVG. ICO uses
/// standard 32-bit DIB frames, so the build needs no PNG intermediates.
#[cfg(windows)]
fn windows_icon() -> std::path::PathBuf {
    let sizes = [16u32, 24, 32, 48, 64, 128, 256];
    let mut frames = Vec::new();
    for size in sizes {
        let (_, _, rgba) = branding::render_logo(size, true).expect("valid logo.svg");
        let mask_stride = size.div_ceil(32) as usize * 4;
        let mut frame = Vec::new();
        frame.extend_from_slice(&40u32.to_le_bytes());
        frame.extend_from_slice(&(size as i32).to_le_bytes());
        frame.extend_from_slice(&((size * 2) as i32).to_le_bytes());
        frame.extend_from_slice(&1u16.to_le_bytes());
        frame.extend_from_slice(&32u16.to_le_bytes());
        frame.extend_from_slice(&0u32.to_le_bytes());
        frame.extend_from_slice(&(size * size * 4).to_le_bytes());
        frame.extend_from_slice(&[0; 16]);
        for y in (0..size as usize).rev() {
            for x in 0..size as usize {
                let at = (y * size as usize + x) * 4;
                frame.extend_from_slice(&[rgba[at + 2], rgba[at + 1], rgba[at], rgba[at + 3]]);
            }
        }
        for y in (0..size as usize).rev() {
            let mut mask = vec![0u8; mask_stride];
            for x in 0..size as usize {
                if rgba[(y * size as usize + x) * 4 + 3] == 0 {
                    mask[x / 8] |= 0x80 >> (x % 8);
                }
            }
            frame.extend_from_slice(&mask);
        }
        frames.push(frame);
    }
    let mut ico = vec![0, 0, 1, 0];
    ico.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + sizes.len() * 16;
    for (size, frame) in sizes.into_iter().zip(&frames) {
        ico.extend_from_slice(&[size as u8, size as u8, 0, 0]);
        ico.extend_from_slice(&1u16.to_le_bytes());
        ico.extend_from_slice(&32u16.to_le_bytes());
        ico.extend_from_slice(&(frame.len() as u32).to_le_bytes());
        ico.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += frame.len();
    }
    for frame in frames {
        ico.extend_from_slice(&frame);
    }
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo OUT_DIR"));
    let icon = out.join("spoutpeak.ico");
    std::fs::write(&icon, &ico).expect("write Windows icon");
    // The installer already consumes target/release/spoutpeak.exe; keep its
    // generated icon beside that executable for either Cargo build profile.
    let profile = profile_dir(&out);
    std::fs::write(profile.join("spoutpeak.ico"), ico).expect("write installer icon");
    icon
}

/// Bakes the watch-together server URL into the app: `SPOUTPEAK_WATCH_URL`
/// from the environment, else from `.env`.
fn watch_url() {
    println!("cargo:rerun-if-changed=.env");
    println!("cargo:rerun-if-env-changed=SPOUTPEAK_WATCH_URL");
    let from_file = std::fs::read_to_string(".env").ok().and_then(|file| {
        file.lines().find_map(|line| {
            let (key, value) = line.trim().split_once('=')?;
            (key.trim() == "SPOUTPEAK_WATCH_URL")
                .then(|| value.trim().trim_matches('"').trim_matches('\'').to_owned())
        })
    });
    let url = std::env::var("SPOUTPEAK_WATCH_URL")
        .ok()
        .or(from_file)
        .unwrap_or_default();
    if !url.is_empty() {
        println!("cargo:rustc-env=SPOUTPEAK_WATCH_URL={url}");
    }
}
