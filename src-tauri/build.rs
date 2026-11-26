fn main() {
    copy_conpty();
    // Android 15+ devices may use 16 KB memory pages; NDK 27 links for 4 KB
    // by default. A Cargo config would not do: the Tauri CLI sets RUSTFLAGS.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("android") {
        println!("cargo:rustc-cdylib-link-arg=-Wl,-z,max-page-size=16384");
    }
    tauri_build::build()
}

/// Puts the bundled ConPTY next to the built exe, so `target/<profile>/myterm.exe`
/// uses it too (the installers get it from tauri.windows.conf.json).
fn copy_conpty() {
    println!("cargo:rerun-if-changed=conpty");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    // OUT_DIR is target/<profile>/build/<pkg>/out.
    let Ok(out) = std::env::var("OUT_DIR") else { return };
    let Some(profile_dir) = std::path::Path::new(&out).ancestors().nth(3) else { return };
    for f in ["conpty.dll", "OpenConsole.exe"] {
        let src = std::path::Path::new("conpty").join(f);
        let dst = profile_dir.join(f);
        let same = std::fs::metadata(&dst).ok().map(|m| m.len()) == std::fs::metadata(&src).ok().map(|m| m.len());
        if !same {
            // Fails harmlessly while the app is running and holds the file.
            let _ = std::fs::copy(&src, &dst);
        }
    }
}
