mod client_install;
#[cfg(target_os = "macos")]
mod detection;
#[cfg(target_os = "linux")]
mod launcher;
#[cfg(target_os = "linux")]
mod logon;
