//! Windows registry lookups for Steam.

#[cfg_attr(not(target_os = "windows"), allow(unused_imports))]
use super::*;

/// Read a value from `HKCU\Software\Valve\Steam` via `reg query` (Windows).
#[cfg(target_os = "windows")]
pub(super) fn read_steam_registry(value: &str) -> Option<String> {
    read_registry(r"HKCU\Software\Valve\Steam", value)
}

/// Parse `reg query` DWORD data: `0x1a2b` (how REG_DWORD prints) or decimal.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(super) fn parse_reg_dword(data: &str) -> Option<u32> {
    let data = data.trim();
    data.strip_prefix("0x")
        .map(|h| u32::from_str_radix(h, 16).ok())
        .unwrap_or_else(|| data.parse().ok())
}

/// Read a registry value's data via `reg query` (Windows).
#[cfg(target_os = "windows")]
pub(super) fn read_registry(key: &str, value: &str) -> Option<String> {
    let output = Command::new("reg")
        .args(["query", key, "/v", value])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with(value) {
            // Line looks like: `SteamExe    REG_SZ    c:/program files (x86)/steam/steam.exe`
            if let Some(pos) = line.find("REG_") {
                let mut parts = line[pos..].splitn(2, char::is_whitespace);
                let _ty = parts.next();
                if let Some(data) = parts.next() {
                    return Some(data.trim().to_string());
                }
            }
        }
    }
    None
}

/// Resolve the path to `steam.exe` (Windows).
#[cfg(target_os = "windows")]
pub(super) fn steam_exe_path() -> Option<PathBuf> {
    if let Some(raw) = read_steam_registry("SteamExe") {
        let path = PathBuf::from(raw.replace('/', "\\"));
        if path.exists() {
            return Some(path);
        }
    }
    let fallback = PathBuf::from(r"C:\Program Files (x86)\Steam\steam.exe");
    fallback.exists().then_some(fallback)
}
