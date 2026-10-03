fn main() {
    if std::env::var_os("CARGO_CFG_TARGET_OS").as_deref() == Some(std::ffi::OsStr::new("windows")) {
        // The optimized GUI constructor and CPAL/WGPU startup path contain a
        // few large fixed-capacity realtime values before ownership reaches
        // their long-lived heap/callback homes. Reserve virtual stack space so
        // Windows release builds cannot fail before the first window appears.
        // Pages are committed on demand; this does not allocate 16 MiB eagerly.
        let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
        if matches!(target_env.as_str(), "gnu" | "gnullvm") {
            println!("cargo:rustc-link-arg-bin=citrus-studio=-Wl,--stack,16777216");
        } else {
            println!("cargo:rustc-link-arg-bin=citrus-studio=/STACK:16777216");
        }
    }
}
