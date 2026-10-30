//! Windows `steamwebhelper.exe` stand-in.
//!
//! Runs `steamwebhelper.real.exe` with extra flags.
//! Built by the root `build.rs`.
#![windows_subsystem = "windows"]

/// Set while running; also our file marker.
const GUARD: &str = "AURELIA_CEF_WRAPPER";

fn main() {
    // Real exe is a wrapper: stop.
    if std::env::var_os(GUARD).is_some() {
        std::process::exit(1);
    }
    let me = std::env::current_exe().expect("own path");
    let real = me.with_file_name("steamwebhelper.real.exe");
    let mut args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    // Flags live beside the exe.
    if let Ok(extra) = std::fs::read_to_string(me.with_file_name("steamwebhelper.flags")) {
        args.extend(extra.split_whitespace().map(Into::into));
    }
    let code = std::process::Command::new(real)
        .args(args)
        .env(GUARD, "1")
        .status()
        .map(|s| s.code().unwrap_or(1))
        .unwrap_or(1);
    std::process::exit(code);
}
