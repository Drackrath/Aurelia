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

// Headless Steam's helper wrapper.
fn embed_cef_wrapper() {
    println!("cargo::rustc-check-cfg=cfg(embedded_cef_wrapper)");
    println!("cargo:rerun-if-changed=crates/cef-wrapper");
    let env = |k: &str| std::env::var(k).unwrap_or_default();
    let mingw = "x86_64-w64-mingw32-gcc";
    let cross = !env("HOST").contains("windows");
    // Wine Steam on Linux; Steam on Windows.
    let (target, linker) = match env("CARGO_CFG_TARGET_OS").as_str() {
        "linux" if env("CARGO_CFG_TARGET_ARCH") == "x86_64" => {
            ("x86_64-pc-windows-gnu".to_string(), Some(mingw))
        }
        "windows" => {
            let gnu = env("CARGO_CFG_TARGET_ENV") == "gnu";
            (env("TARGET"), (gnu && cross).then_some(mingw))
        }
        _ => return,
    };
    let wrapper = std::path::PathBuf::from(env("OUT_DIR")).join("cef-wrapper.exe");
    let mut rustc = std::process::Command::new(env("RUSTC"));
    rustc
        .args(["--edition", "2024", "--crate-type", "bin"])
        .args(["--crate-name", "steamwebhelper", "--target", &target])
        .args(["-C", "opt-level=3", "-C", "strip=symbols"]);
    if let Some(linker) = linker {
        rustc.arg("-C").arg(format!("linker={linker}"));
    }
    let built = rustc
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
            "cargo:warning=headless Steam: cef-wrapper.exe not built \
             (needs the {target} Rust target{})",
            linker.map(|l| format!(" and {l}")).unwrap_or_default()
        );
    }
}
