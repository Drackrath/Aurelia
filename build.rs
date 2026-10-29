// Generate protobuf messages that steam-vent-proto does not ship, with pure-Rust
// rust-protobuf codegen — no protoc required. Each proto lands in its own
// `$OUT_DIR/<name>/` and is `include!`d from the matching src/steam_client module.
fn main() {
    for (proto, out) in [
        ("proto/service_cloudconfigstore.proto", "cloudconfig"),
        ("proto/service_storequery.proto", "storequery"),
        ("proto/service_wishlist.proto", "wishlist"),
    ] {
        println!("cargo:rerun-if-changed={proto}");
        protobuf_codegen::Codegen::new()
            .pure()
            .include("proto")
            .input(proto)
            .cargo_out_dir(out)
            .run_from_script();
    }
    embed_cef_wrapper();
}

// Headless in-Wine Steam's helper wrapper.
fn embed_cef_wrapper() {
    println!("cargo::rustc-check-cfg=cfg(embedded_cef_wrapper)");
    println!("cargo:rerun-if-changed=crates/cef-wrapper");
    let env = |k: &str| std::env::var(k).unwrap_or_default();
    // Only Linux hosts run Wine Steam.
    if env("CARGO_CFG_TARGET_OS") != "linux" || env("CARGO_CFG_TARGET_ARCH") != "x86_64" {
        return;
    }
    let wrapper = std::path::PathBuf::from(env("OUT_DIR")).join("cef-wrapper.exe");
    let linker = "x86_64-w64-mingw32-gcc";
    let built = std::process::Command::new(env("RUSTC"))
        .args(["--edition", "2024", "--crate-type", "bin"])
        .args(["--crate-name", "steamwebhelper", "--target", "x86_64-pc-windows-gnu"])
        .args(["-C", "opt-level=3", "-C", "strip=symbols"])
        .arg("-C")
        .arg(format!("linker={linker}"))
        .arg("-o")
        .arg(&wrapper)
        .arg("crates/cef-wrapper/main.rs")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
        && wrapper.is_file();
    if built {
        println!("cargo:rustc-cfg=embedded_cef_wrapper");
    } else {
        println!(
            "cargo:warning=headless Wine Steam: cef-wrapper.exe not built \
             (needs `rustup target add x86_64-pc-windows-gnu` and {linker})"
        );
    }
}
