mod client_install;
#[cfg(target_os = "macos")]
mod detection;
mod exit_sync;
#[cfg(target_os = "linux")]
mod launcher;
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod logon;
