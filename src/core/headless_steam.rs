//! Host Steam without its CEF UI.
//!
//! Bind-mounts a stand-in `steamui/index.html`.
use anyhow::Result;
#[cfg(target_os = "linux")]
use anyhow::{Context, bail};
#[cfg(target_os = "linux")]
use std::path::{Path, PathBuf};

/// Who the stand-in page signs in.
pub struct HeadlessLogin {
    pub account_name: String,
    /// Aurelia's refresh token, if any.
    pub refresh_token: Option<String>,
}

/// The stand-in page.
pub fn frontend_page(account_name: &str, token_url: Option<&str>) -> String {
    let account = serde_json::to_string(account_name).unwrap_or_else(|_| "\"\"".into());
    let token_url = serde_json::to_string(&token_url).unwrap_or_else(|_| "null".into());
    PAGE.replace("__ACCOUNT__", &account)
        .replace("__TOKEN_URL__", &token_url)
}

/// Marks our page, not Valve's.
pub const PAGE_MARKER: &str = "<!-- aurelia-headless -->";

const PAGE: &str = r#"<!doctype html><!-- aurelia-headless --><html><head><title>SharedJSContext</title><meta charset="utf-8"></head><body><script>
const log = (m) => console.log("[aurelia-headless] " + m);
const ACCOUNT = __ACCOUNT__;
const TOKEN_URL = __TOKEN_URL__;
// Prompts safe to confirm unattended.
const AUTO_CONTINUE = new Set(["ShowInterstitials", "ShowDurationControl"]);
function onUserRequest(actionId, gameId, request, details) {
  if (AUTO_CONTINUE.has(request)) {
    log(`continue ${request} for ${gameId}`);
    SteamClient.Apps.ContinueGameAction(actionId, request);
  } else {
    log(`cancel ${request} for ${gameId} (needs the Steam UI): ${details}`);
    SteamClient.Apps.CancelGameAction(actionId);
  }
}
async function signIn() {
  if (TOKEN_URL) {
    try {
      const login = await (await fetch(TOKEN_URL)).json();
      const r = await SteamClient.Auth.SetLoginToken(login.token, login.account);
      log("SetLoginToken -> " + JSON.stringify(r));
      if (r.result === 1) return;
    } catch (e) {
      log("token login failed: " + e);
    }
  }
  const r = await SteamClient.Auth.StartSignInFromCache(ACCOUNT);
  log("StartSignInFromCache -> " + JSON.stringify(r));
}
async function main() {
  SteamClient.Apps.RegisterForGameActionUserRequest(onUserRequest);
  SteamClient.Apps.RegisterForGameActionShowError((id, gameId, err, details) =>
    log(`launch error for ${gameId}: ${err} ${details}`));
  try {
    await signIn();
  } catch (e) {
    log("sign-in threw: " + e);
  }
  // Client holds some work until this.
  SteamClient.UI.NotifyAppInitialized();
}
main();
</script></body></html>
"#;

#[cfg(target_os = "linux")]
/// Chromium switches for a windowless helper.
///
/// Helper PSS: 465 to 249 MB.
const CEF_FLAGS: &[&str] = &[
    "--single-process",
    "--ozone-platform=headless",
    "--use-gl=disabled",
    "--disable-gpu",
    "--disable-gpu-compositing",
    "--disable-software-rasterizer",
    "--in-process-gpu",
    "--no-zygote",
    // Only our local page loads.
    "--no-sandbox",
    "--disable-extensions",
    "--disable-background-networking",
    "--disable-component-update",
    "--mute-audio",
    "--js-flags=--lite-mode",
];

#[cfg(target_os = "linux")]
const HELPER_EXEC: &str = r#"exec ./steamwebhelper "$@""#;

#[cfg(target_os = "linux")]
/// Valve's wrapper with our flags.
///
/// `None` if its exec line changed.
fn trimmed_wrapper(original: &str, flags: &[&str]) -> Option<String> {
    if original.lines().filter(|l| l.trim() == HELPER_EXEC).count() != 1 {
        return None;
    }
    let exec = format!("{HELPER_EXEC} {}", flags.join(" "));
    Some(
        original
            .lines()
            .map(|l| {
                if l.trim() == HELPER_EXEC {
                    exec.as_str()
                } else {
                    l
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n",
    )
}

#[cfg(target_os = "linux")]
const RT3_FLAG: &str = ".steam-enable-steamrt64-client";

#[cfg(target_os = "linux")]
/// Whether host Steam runs the steamrt3c client.
pub fn rt3_client(steam_root: &Path) -> bool {
    steam_root.join(RT3_FLAG).exists() && steam_root.join("steamrt64/steam").is_file()
}

#[cfg(target_os = "linux")]
/// Switch host Steam's steamrt3c client.
///
/// Steam's own opt-in file; `steam.sh` reads it.
pub fn set_rt3_client(steam_root: &Path, on: bool) -> Result<()> {
    let flag = steam_root.join(RT3_FLAG);
    if on {
        if !steam_root.join("steamrt64/steam").is_file() {
            bail!("this Steam install has no steamrt3c client (steamrt64/steam)");
        }
        std::fs::write(&flag, b"").with_context(|| format!("writing {}", flag.display()))?;
    } else if flag.exists() {
        std::fs::remove_file(&flag).with_context(|| format!("removing {}", flag.display()))?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
/// Whether headless mode can run here.
pub fn available(steam_root: &Path) -> bool {
    steam_root.join("steamui/index.html").is_file()
        && steam_root.join("steam.sh").is_file()
        && which("bwrap").is_some()
}

#[cfg(target_os = "linux")]
fn which(bin: &str) -> Option<PathBuf> {
    std::env::var_os("PATH")?
        .to_str()?
        .split(':')
        .map(|dir| Path::new(dir).join(bin))
        .find(|p| p.is_file())
}

#[cfg(target_os = "linux")]
/// Private dir for the page.
fn page_dir() -> Result<PathBuf> {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join("aurelia").join("headless-steam");
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(dir)
}

#[cfg(target_os = "linux")]
/// Start host Steam headless.
///
/// The token is served in the background.
pub async fn start(steam_root: &Path, login: HeadlessLogin) -> Result<()> {
    if !available(steam_root) {
        bail!("headless Steam needs bwrap and a Steam install with steamui/");
    }
    let page = page_dir()?.join("index.html");
    std::fs::write(&page, page_for(login).await?)?;

    let mut cmd = std::process::Command::new("bwrap");
    cmd.args(["--dev-bind", "/", "/", "--bind"])
        .arg(&page)
        .arg(steam_root.join("steamui/index.html"));
    // steamrt3c starts its own helper script.
    let wrap_path = if rt3_client(steam_root) {
        steam_root.join("steamrt64/steamwebhelper.sh")
    } else {
        steam_root.join("ubuntu12_64/steamwebhelper_sniper_wrap.sh")
    };
    let trimmed = std::fs::read_to_string(&wrap_path)
        .ok()
        .and_then(|w| trimmed_wrapper(&w, CEF_FLAGS));
    match trimmed {
        Some(wrapper) => {
            let ours = page.with_file_name("steamwebhelper_wrap.sh");
            std::fs::write(&ours, wrapper)?;
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&ours, std::fs::Permissions::from_mode(0o700))?;
            cmd.arg("--bind").arg(&ours).arg(&wrap_path);
        }
        None => tracing::warn!("steamwebhelper wrapper changed; running CEF untrimmed"),
    }
    cmd.arg("--")
        .arg(steam_root.join("steam.sh"))
        .arg("-silent")
        // Bound files fail verify; reinstall loops.
        .arg("-noverifyfiles")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    // Outlive this launch, like `steam -silent`.
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }
    cmd.spawn().context("starting headless Steam under bwrap")?;
    tracing::info!("Launched headless Steam client (stand-in UI)");

    Ok(())
}

/// Page for `login`; serves its token.
pub async fn page_for(login: HeadlessLogin) -> Result<String> {
    let url = match login.refresh_token {
        Some(token) => {
            let server = TokenServer::start(token, login.account_name.clone()).await?;
            let url = server.url.clone();
            tokio::spawn(server.serve_once(std::time::Duration::from_secs(120)));
            Some(url)
        }
        None => None,
    };
    Ok(frontend_page(&login.account_name, url.as_deref()))
}

/// One-shot loopback token endpoint.
struct TokenServer {
    listener: tokio::net::TcpListener,
    url: String,
    secret: String,
    body: String,
}

impl TokenServer {
    async fn start(token: String, account: String) -> Result<Self> {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
        let secret = format!("{:032x}", rand::random::<u128>());
        let url = format!("http://{}/login?s={secret}", listener.local_addr()?);
        let body = serde_json::json!({ "token": token, "account": account }).to_string();
        Ok(Self {
            listener,
            url,
            secret,
            body,
        })
    }

    /// Hand out the token once.
    async fn serve_once(self, timeout: std::time::Duration) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let accepted = tokio::time::timeout_at(deadline, self.listener.accept()).await;
            let Ok(Ok((mut stream, _))) = accepted else {
                tracing::warn!("headless Steam never fetched its login token");
                return;
            };
            let mut buf = vec![0u8; 4096];
            let n = match tokio::time::timeout(
                std::time::Duration::from_secs(5),
                stream.read(&mut buf),
            )
            .await
            {
                Ok(Ok(n)) => n,
                _ => continue,
            };
            let request = String::from_utf8_lossy(&buf[..n]);
            let (response, done) = respond(&request, &self.secret, &self.body);
            let _ = stream.write_all(response.as_bytes()).await;
            if done {
                tracing::info!("headless Steam fetched its login token");
                return;
            }
        }
    }
}

// Page origin is steamloopback.host.
const CORS: &str = "Access-Control-Allow-Origin: *\r\n\
                    Access-Control-Allow-Private-Network: true\r\n\
                    Access-Control-Allow-Methods: GET\r\n";

/// HTTP reply; true once token sent.
fn respond(request: &str, secret: &str, body: &str) -> (String, bool) {
    let line = request.lines().next().unwrap_or_default();
    if line.starts_with("OPTIONS ") {
        return (
            format!("HTTP/1.1 204 No Content\r\n{CORS}Content-Length: 0\r\n\r\n"),
            false,
        );
    }
    let wanted = format!("GET /login?s={secret} ");
    if !line.starts_with(&wanted) {
        return (
            format!("HTTP/1.1 403 Forbidden\r\n{CORS}Content-Length: 0\r\n\r\n"),
            false,
        );
    }
    (
        format!(
            "HTTP/1.1 200 OK\r\n{CORS}Content-Type: application/json\r\n\
             Cache-Control: no-store\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        ),
        true,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_os = "linux")]
    fn wrapper_gets_flags_on_exec_line() {
        let valve = "export LD_LIBRARY_PATH=.\necho hi\nexec ./steamwebhelper \"$@\"\n";
        let ours = trimmed_wrapper(valve, &["--a", "--b"]).unwrap();
        assert!(ours.contains("export LD_LIBRARY_PATH=.\necho hi\n"));
        assert!(ours.contains(r#"exec ./steamwebhelper "$@" --a --b"#));
        // Changed upstream: leave untrimmed.
        assert!(trimmed_wrapper("exec ./steamwebhelper --new \"$@\"", &["--a"]).is_none());
    }

    #[test]
    fn page_embeds_escaped_values() {
        let page = frontend_page("a\"b", Some("http://127.0.0.1:1/login?s=x"));
        assert!(page.contains(r#"const ACCOUNT = "a\"b";"#));
        assert!(page.contains(r#"const TOKEN_URL = "http://127.0.0.1:1/login?s=x";"#));
        let no_token = frontend_page("acc", None);
        assert!(no_token.contains("const TOKEN_URL = null;"));
    }

    #[test]
    fn token_needs_secret_and_goes_out_once() {
        let body = r#"{"token":"t","account":"a"}"#;
        let (r, done) = respond("GET /login?s=wrong HTTP/1.1\r\n", "right", body);
        assert!(r.starts_with("HTTP/1.1 403") && !done && !r.contains("\"t\""));
        let (r, done) = respond("OPTIONS /login?s=right HTTP/1.1\r\n", "right", body);
        assert!(r.contains("Allow-Private-Network") && !done);
        let (r, done) = respond("GET /login?s=right HTTP/1.1\r\n", "right", body);
        assert!(r.starts_with("HTTP/1.1 200") && r.ends_with(body) && done);
    }
}
