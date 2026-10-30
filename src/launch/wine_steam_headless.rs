//! Headless mode for the in-Wine Steam.
//!
//! Aurelia owns the prefix; swaps files.
//! Originals are kept beside them.
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

#[cfg(embedded_cef_wrapper)]
const WRAPPER: Option<&[u8]> = Some(include_bytes!(concat!(env!("OUT_DIR"), "/cef-wrapper.exe")));
#[cfg(not(embedded_cef_wrapper))]
const WRAPPER: Option<&[u8]> = None;

/// Chromium switches for the Windows helper.
///
/// Wine PSS: 3250 to 994 MB.
pub const CEF_FLAGS: &str = "--single-process --disable-gpu --disable-gpu-compositing \
     --disable-software-rasterizer --use-gl=disabled --in-process-gpu --no-sandbox \
     --disable-extensions --disable-background-networking --disable-component-update \
     --mute-audio --enable-features=NetworkServiceInProcess --js-flags=--lite-mode";

struct Paths {
    helper: PathBuf,
    real: PathBuf,
    flags: PathBuf,
    crash_cfg: PathBuf,
    crash_off: PathBuf,
    page: PathBuf,
    page_orig: PathBuf,
}

fn paths(steam_dir: &Path) -> Paths {
    let cef = steam_dir.join("bin/cef/cef.win64");
    let ui = steam_dir.join("steamui");
    Paths {
        helper: cef.join("steamwebhelper.exe"),
        real: cef.join("steamwebhelper.real.exe"),
        flags: cef.join("steamwebhelper.flags"),
        crash_cfg: cef.join("crash_reporter.cfg"),
        crash_off: cef.join("crash_reporter.cfg.aurelia-off"),
        page: ui.join("index.html"),
        page_orig: ui.join("index.html.aurelia-orig"),
    }
}

/// Turn headless on with `page`.
///
/// Needs the embedded wrapper.
pub fn enable(steam_dir: &Path, page: &str) -> Result<()> {
    let Some(wrapper) = WRAPPER else {
        anyhow::bail!("this build has no CEF wrapper (needs mingw at build time)");
    };
    let p = paths(steam_dir);

    // Steam may have updated the helper.
    let current =
        std::fs::read(&p.helper).with_context(|| format!("reading {}", p.helper.display()))?;
    if current != wrapper {
        std::fs::rename(&p.helper, &p.real)?;
        std::fs::write(&p.helper, wrapper)?;
    }
    std::fs::write(&p.flags, CEF_FLAGS)?;

    // Its crash reporter costs ~250 MB.
    if p.crash_cfg.is_file() {
        std::fs::rename(&p.crash_cfg, &p.crash_off)?;
    }

    let current = std::fs::read_to_string(&p.page).unwrap_or_default();
    if !current.contains(crate::core::headless_steam::PAGE_MARKER) && p.page.is_file() {
        std::fs::rename(&p.page, &p.page_orig)?;
    }
    std::fs::write(&p.page, page)?;
    Ok(())
}

/// Whether the stand-in page is installed.
pub fn is_enabled(steam_dir: &Path) -> bool {
    std::fs::read_to_string(paths(steam_dir).page)
        .is_ok_and(|page| page.contains(crate::core::headless_steam::PAGE_MARKER))
}

/// Put Valve's files back.
pub fn disable(steam_dir: &Path) -> Result<()> {
    let p = paths(steam_dir);
    if p.real.is_file() {
        let current = std::fs::read(&p.helper).unwrap_or_default();
        if WRAPPER.is_some_and(|w| current == w) || current.is_empty() {
            std::fs::rename(&p.real, &p.helper)?;
        } else {
            // Steam updated it; keep that.
            std::fs::remove_file(&p.real)?;
        }
    }
    let _ = std::fs::remove_file(&p.flags);
    if p.crash_off.is_file() {
        if p.crash_cfg.is_file() {
            std::fs::remove_file(&p.crash_off)?;
        } else {
            std::fs::rename(&p.crash_off, &p.crash_cfg)?;
        }
    }
    if p.page_orig.is_file() {
        let current = std::fs::read_to_string(&p.page).unwrap_or_default();
        if current.contains(crate::core::headless_steam::PAGE_MARKER) || current.is_empty() {
            std::fs::rename(&p.page_orig, &p.page)?;
        } else {
            std::fs::remove_file(&p.page_orig)?;
        }
    }
    Ok(())
}

/// Wine reads this at desktop start.
const TRAY_POLICY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Policies\Explorer";

/// Whether `user.reg` hides tray icons.
pub fn tray_hidden(wineprefix: &Path) -> bool {
    std::fs::read_to_string(wineprefix.join("user.reg")).is_ok_and(|reg| {
        reg.split("\n[").any(|section| {
            section
                .starts_with(r"Software\\Microsoft\\Windows\\CurrentVersion\\Policies\\Explorer]")
                && section.contains("\"NoTrayItemsDisplay\"=dword:00000001")
        })
    })
}

/// Hide or show the prefix's tray.
///
/// Steam's headless icon is dead.
pub fn set_tray_hidden(
    wine: &Path,
    wineprefix: &Path,
    envs: &[(&str, &str)],
    hide: bool,
) -> Result<()> {
    if tray_hidden(wineprefix) == hide {
        return Ok(());
    }
    let mut reg = std::process::Command::new(wine);
    if hide {
        reg.args(["reg", "add", TRAY_POLICY, "/v", "NoTrayItemsDisplay"])
            .args(["/t", "REG_DWORD", "/d", "1", "/f"]);
    } else {
        reg.args([
            "reg",
            "delete",
            TRAY_POLICY,
            "/v",
            "NoTrayItemsDisplay",
            "/f",
        ]);
    }
    reg.env("WINEPREFIX", wineprefix)
        .envs(envs.iter().copied())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let status = reg.status().context("running wine reg")?;
    anyhow::ensure!(status.success(), "wine reg failed: {status}");

    // Our boot's desktop must exit.
    let mut wait = std::process::Command::new(wine.with_file_name("wineserver"));
    wait.arg("-w")
        .env("WINEPREFIX", wineprefix)
        .envs(envs.iter().copied())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    if let Ok(mut child) = wait.spawn() {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while child.try_wait().ok().flatten().is_none() {
            if std::time::Instant::now() >= deadline {
                // Prefix busy; applies next boot.
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn steam() -> (tempfile::TempDir, Paths) {
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        std::fs::create_dir_all(p.helper.parent().unwrap()).unwrap();
        std::fs::create_dir_all(p.page.parent().unwrap()).unwrap();
        std::fs::write(&p.helper, b"MZvalve-helper").unwrap();
        std::fs::write(&p.crash_cfg, b"[Config]").unwrap();
        std::fs::write(&p.page, b"<html>valve</html>").unwrap();
        (dir, p)
    }

    fn page() -> String {
        format!(
            "{}<p>stand-in</p>",
            crate::core::headless_steam::PAGE_MARKER
        )
    }

    #[test]
    #[cfg(embedded_cef_wrapper)]
    fn enable_then_disable_restores_valve_files() {
        let (dir, p) = steam();
        enable(dir.path(), &page()).unwrap();
        assert_eq!(std::fs::read(&p.helper).unwrap(), WRAPPER.unwrap());
        assert_eq!(std::fs::read(&p.real).unwrap(), b"MZvalve-helper");
        assert!(p.flags.is_file() && !p.crash_cfg.exists());
        // Re-enabling keeps the true original.
        enable(dir.path(), &page()).unwrap();
        assert_eq!(std::fs::read(&p.real).unwrap(), b"MZvalve-helper");
        disable(dir.path()).unwrap();
        assert_eq!(std::fs::read(&p.helper).unwrap(), b"MZvalve-helper");
        assert_eq!(std::fs::read(&p.page).unwrap(), b"<html>valve</html>");
        assert_eq!(std::fs::read(&p.crash_cfg).unwrap(), b"[Config]");
        assert!(!p.real.exists() && !p.flags.exists() && !p.page_orig.exists());
    }

    #[test]
    #[cfg(embedded_cef_wrapper)]
    fn steam_update_while_enabled_wins() {
        let (dir, p) = steam();
        enable(dir.path(), &page()).unwrap();
        // Steam replaced both files.
        std::fs::write(&p.helper, b"MZvalve-helper-v2").unwrap();
        std::fs::write(&p.page, b"<html>valve v2</html>").unwrap();
        disable(dir.path()).unwrap();
        assert_eq!(std::fs::read(&p.helper).unwrap(), b"MZvalve-helper-v2");
        assert_eq!(std::fs::read(&p.page).unwrap(), b"<html>valve v2</html>");
        assert!(!p.real.exists() && !p.page_orig.exists());
    }

    #[test]
    fn reads_tray_policy_from_user_reg() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!tray_hidden(dir.path()));
        std::fs::write(
            dir.path().join("user.reg"),
            "WINE REGISTRY Version 2\n\n[Software\\\\Microsoft\\\\Windows\\\\CurrentVersion\\\\Policies\\\\Explorer] 1793343370\n\
             #time=1dc\n\"NoTrayItemsDisplay\"=dword:00000001\n\n[Software\\\\Wine] 1\n",
        )
        .unwrap();
        assert!(tray_hidden(dir.path()));
    }

    #[test]
    fn disable_without_enable_is_a_no_op() {
        let (dir, p) = steam();
        disable(dir.path()).unwrap();
        assert_eq!(std::fs::read(&p.helper).unwrap(), b"MZvalve-helper");
        assert_eq!(std::fs::read(&p.page).unwrap(), b"<html>valve</html>");
    }
}
