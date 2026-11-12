fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    watch_url();
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.set("ProductName", "SpoutPeak");
        res.set("FileDescription", "SpoutPeak");
        res.compile().expect("failed to embed Windows resources");
    }
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
    let url = std::env::var("SPOUTPEAK_WATCH_URL").ok().or(from_file).unwrap_or_default();
    if !url.is_empty() {
        println!("cargo:rustc-env=SPOUTPEAK_WATCH_URL={url}");
    }
}
