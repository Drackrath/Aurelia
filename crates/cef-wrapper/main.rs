//! Windows `steamwebhelper.exe` stand-in.
//!
//! Runs `steamwebhelper.real.exe` with extra flags.
//! Built by the root `build.rs`.
#![windows_subsystem = "windows"]

fn main() {
    let me = std::env::current_exe().expect("own path");
    let real = me.with_file_name("steamwebhelper.real.exe");
    let mut args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    // Flags live beside the exe.
    if let Ok(extra) = std::fs::read_to_string(me.with_file_name("steamwebhelper.flags")) {
        args.extend(extra.split_whitespace().map(Into::into));
    }
    let code = std::process::Command::new(real)
        .args(args)
        .status()
        .map(|s| s.code().unwrap_or(1))
        .unwrap_or(1);
    std::process::exit(code);
}
