//! Cloud path resolution and local save discovery.

use super::*;

/// Translates Steam Cloud filenames to real on-disk paths.
///
/// Steam Auto-Cloud filenames embed the real save location as a leading root token,
/// e.g. `%WinAppDataLocalLow%SadSocket/9Kings/save.json`. That token must be mapped
/// to the actual OS directory the game reads/writes — otherwise saves land in a
/// phantom folder under `userdata/.../<appid>/` and Steam always reports a
/// cloud/local mismatch. Classic ISteamRemoteStorage filenames carry no token and
/// live under `remote_root` (`userdata/<accountid>/<appid>/remote`).
///
/// A Windows game played through Proton writes its saves inside the wine prefix, so
/// its `%Win*%` roots resolve under `wine_prefix/drive_c/users/steamuser` rather than
/// to anything in the Linux home. Without a prefix those tokens can't be placed at
/// all and the file is skipped.
pub struct CloudPathResolver {
    pub(super) remote_root: PathBuf,
    pub(super) install_dir: Option<PathBuf>,
    pub(super) wine_prefix: Option<PathBuf>,
    /// Overrides for the target OS only.
    pub(super) overrides: Vec<UfsRootOverride>,
    pub(super) steam_id: Option<u64>,
}

impl CloudPathResolver {
    pub fn new(remote_root: PathBuf, install_dir: Option<PathBuf>) -> Self {
        Self {
            remote_root,
            install_dir,
            wine_prefix: None,
            overrides: Vec::new(),
            steam_id: None,
        }
    }

    /// Keep overrides matching `target_os`.
    #[must_use]
    pub fn with_root_overrides(mut self, overrides: &[UfsRootOverride], target_os: &str) -> Self {
        self.overrides = overrides
            .iter()
            .filter(|o| o.applies_to(target_os))
            .cloned()
            .collect();
        self
    }

    /// Steam id for path placeholders.
    #[must_use]
    pub fn with_steam_id(mut self, steam_id: u64) -> Self {
        self.steam_id = Some(steam_id);
        self
    }

    pub(super) fn override_for(&self, token: &str) -> Option<&UfsRootOverride> {
        self.overrides
            .iter()
            .find(|o| o.root.eq_ignore_ascii_case(token))
    }

    /// Apply the token's path transforms.
    pub fn transform_path(&self, token: &str, path: &str) -> String {
        let mut out = path.to_string();
        if let Some(o) = self.override_for(token) {
            for (find, replace) in &o.path_transforms {
                if out.contains(find.as_str()) {
                    out = out.replacen(find.as_str(), replace, 1);
                }
            }
        }
        out
    }

    /// Fill `{64BitSteamID}` and `{Steam3AccountID}`.
    pub fn substitute_ids(&self, path: &str) -> String {
        let Some(id) = self.steam_id else {
            return path.to_string();
        };
        path.replace("{64BitSteamID}", &id.to_string())
            .replace("{Steam3AccountID}", &(id as u32).to_string())
    }

    /// Point `%Win*%` root tokens at a Proton/wine prefix (the `pfx` directory that
    /// contains `drive_c`). Without this they resolve to nothing on Linux.
    #[must_use]
    pub fn with_wine_prefix(mut self, wine_prefix: Option<PathBuf>) -> Self {
        self.wine_prefix = wine_prefix;
        self
    }

    /// Map a cloud filename to its real local path, resolving any `%RootToken%`
    /// prefix. Errors only when a token is present but unknown on this platform.
    pub fn resolve(&self, filename: &str) -> Result<PathBuf> {
        match split_root_token(filename) {
            Some((token, rest)) => {
                let base = self.resolve_root(token).with_context(|| {
                    format!("unsupported Steam Cloud root token '%{token}%' on this platform")
                })?;
                Ok(join_relative(&base, &self.transform_path(token, rest)))
            }
            None => Ok(join_relative(&self.remote_root, filename)),
        }
    }

    /// Resolve a bare root token (e.g. `WinAppDataLocalLow`) to its OS directory, or
    /// `None` if it isn't applicable on this platform (e.g. a `%Linux*%` root on
    /// Windows, or a `%Win*%` root with no wine prefix). Used by UFS save discovery.
    pub fn resolve_root(&self, root_token: &str) -> Option<PathBuf> {
        let lookup = |token: &str| {
            resolve_root_token(
                RootToken::from_name(token)?,
                self.install_dir.as_deref(),
                self.wine_prefix.as_deref(),
            )
        };
        match self.override_for(root_token) {
            Some(o) => {
                let target = if o.use_instead.is_empty() {
                    o.root.as_str()
                } else {
                    o.use_instead.as_str()
                };
                let base = lookup(target)?;
                Some(join_relative(&base, &self.substitute_ids(&o.add_path)))
            }
            None => lookup(root_token),
        }
    }
}

/// Discover local save files matching the app's UFS `savefiles` rules and pair each
/// with the cloud filename Steam would store it under (`%Root%<sub-path>`). Lets
/// `sync_up` upload brand-new saves that aren't in the cloud yet. Rules whose root
/// doesn't apply on this platform are skipped.
pub(super) fn discover_local_saves(
    specs: &[UfsSaveSpec],
    resolver: &CloudPathResolver,
) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    for spec in specs {
        let Some(base) = resolver.resolve_root(&spec.root) else {
            continue;
        };
        // Cloud names keep the untransformed path.
        let spec_path = normalize_rel(&resolver.substitute_ids(&spec.path));
        let search_root = join_relative(&base, &resolver.transform_path(&spec.root, &spec_path));
        if !search_root.is_dir() {
            continue;
        }

        let walker = walkdir::WalkDir::new(&search_root).min_depth(1);
        let walker = if spec.recursive {
            walker
        } else {
            walker.max_depth(1)
        };
        for entry in walker
            .into_iter()
            .filter_map(std::result::Result::ok)
            .filter(|e| e.file_type().is_file())
        {
            let name = entry.file_name().to_string_lossy();
            if !glob_matches(&spec.pattern, &name) {
                continue;
            }
            let Ok(rel) = entry.path().strip_prefix(&search_root) else {
                continue;
            };
            let rel = rel.to_string_lossy().replace('\\', "/");
            let cloud_name = if spec_path.is_empty() {
                format!("%{}%{rel}", spec.root)
            } else {
                format!("%{}%{spec_path}/{rel}", spec.root)
            };
            out.push((cloud_name, entry.path().to_path_buf()));
        }
    }
    out
}

/// Match a Steam UFS filename glob (`*`, `?`) against a filename, case-insensitively
/// (Windows save files are matched case-insensitively). An empty/`*` pattern matches
/// everything; an invalid pattern is treated as a match rather than dropping files.
pub(super) fn glob_matches(pattern: &str, name: &str) -> bool {
    if pattern.is_empty() || pattern == "*" {
        return true;
    }
    let mut re = String::from("(?i)^");
    for ch in pattern.chars() {
        match ch {
            '*' => re.push_str(".*"),
            '?' => re.push('.'),
            c => re.push_str(&regex::escape(&c.to_string())),
        }
    }
    re.push('$');
    regex::Regex::new(&re)
        .map(|r| r.is_match(name))
        .unwrap_or(true)
}

/// Split a leading `%Token%` off an Auto-Cloud filename, returning `(token, rest)`.
/// Returns `None` for classic (token-less) remote-storage filenames.
pub(super) fn split_root_token(filename: &str) -> Option<(&str, &str)> {
    let after = filename.strip_prefix('%')?;
    let end = after.find('%')?;
    Some((&after[..end], &after[end + 1..]))
}

/// Clean relative path, `/`-separated.
pub(super) fn normalize_rel(rel: &str) -> String {
    rel.split(['/', '\\'])
        .filter(|c| !c.is_empty() && *c != "." && *c != "..")
        .collect::<Vec<_>>()
        .join("/")
}

/// Join a `/`- or `\`-separated cloud-relative path onto `base`, skipping empty,
/// `.` and `..` components so a crafted filename can't escape the base directory.
pub(super) fn join_relative(base: &Path, rel: &str) -> PathBuf {
    let mut out = base.to_path_buf();
    for comp in rel.split(['/', '\\']) {
        if !comp.is_empty() && comp != "." && comp != ".." {
            out.push(comp);
        }
    }
    out
}

/// A Steam Cloud root token, named as Steam's client names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RootToken {
    GameInstall,
    WinMyDocuments,
    WinAppDataLocal,
    WinAppDataRoaming,
    SteamUserBaseStorage,
    MacHome,
    MacAppSupport,
    MacDocuments,
    WinSavedGames,
    WinProgramData,
    SteamCloudDocuments,
    WinAppDataLocalLow,
    MacCaches,
    LinuxHome,
    LinuxXdgDataHome,
    LinuxXdgConfigHome,
    AndroidSteamPackageRoot,
    WindowsHome,
    AndroidExternalData,
    AndroidInternalData,
    /// Aurelia's own alias for `WinMyDocuments`, not a Steam name.
    WinDocuments,
}

impl RootToken {
    /// Every token.
    pub(super) const ALL: [Self; 21] = [
        Self::GameInstall,
        Self::WinMyDocuments,
        Self::WinAppDataLocal,
        Self::WinAppDataRoaming,
        Self::SteamUserBaseStorage,
        Self::MacHome,
        Self::MacAppSupport,
        Self::MacDocuments,
        Self::WinSavedGames,
        Self::WinProgramData,
        Self::SteamCloudDocuments,
        Self::WinAppDataLocalLow,
        Self::MacCaches,
        Self::LinuxHome,
        Self::LinuxXdgDataHome,
        Self::LinuxXdgConfigHome,
        Self::AndroidSteamPackageRoot,
        Self::WindowsHome,
        Self::AndroidExternalData,
        Self::AndroidInternalData,
        Self::WinDocuments,
    ];

    /// The token's name between the `%` signs.
    fn as_str(self) -> &'static str {
        match self {
            Self::GameInstall => "GameInstall",
            Self::WinMyDocuments => "WinMyDocuments",
            Self::WinAppDataLocal => "WinAppDataLocal",
            Self::WinAppDataRoaming => "WinAppDataRoaming",
            Self::SteamUserBaseStorage => "SteamUserBaseStorage",
            Self::MacHome => "MacHome",
            Self::MacAppSupport => "MacAppSupport",
            Self::MacDocuments => "MacDocuments",
            Self::WinSavedGames => "WinSavedGames",
            Self::WinProgramData => "WinProgramData",
            Self::SteamCloudDocuments => "SteamCloudDocuments",
            Self::WinAppDataLocalLow => "WinAppDataLocalLow",
            Self::MacCaches => "MacCaches",
            Self::LinuxHome => "LinuxHome",
            Self::LinuxXdgDataHome => "LinuxXdgDataHome",
            Self::LinuxXdgConfigHome => "LinuxXdgConfigHome",
            Self::AndroidSteamPackageRoot => "AndroidSteamPackageRoot",
            Self::WindowsHome => "WindowsHome",
            Self::AndroidExternalData => "AndroidExternalData",
            Self::AndroidInternalData => "AndroidInternalData",
            Self::WinDocuments => "WinDocuments",
        }
    }

    /// The token [`as_str`](Self::as_str) names.
    fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.as_str() == name)
    }
}

/// Resolve a Steam Cloud root token to an OS directory.
/// `%GameInstall%` needs the game's install directory; the rest derive from the
/// user's home / known folders.
#[cfg(windows)]
pub(super) fn resolve_root_token(
    token: RootToken,
    install_dir: Option<&Path>,
    _wine_prefix: Option<&Path>,
) -> Option<PathBuf> {
    let user = || std::env::var_os("USERPROFILE").map(PathBuf::from);
    match token {
        RootToken::GameInstall => install_dir.map(Path::to_path_buf),
        RootToken::WinMyDocuments | RootToken::WinDocuments => user().map(|u| u.join("Documents")),
        RootToken::WinAppDataLocal => std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .or_else(|| user().map(|u| u.join("AppData").join("Local"))),
        RootToken::WinAppDataLocalLow => user().map(|u| u.join("AppData").join("LocalLow")),
        RootToken::WinAppDataRoaming => std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .or_else(|| user().map(|u| u.join("AppData").join("Roaming"))),
        RootToken::WinSavedGames => user().map(|u| u.join("Saved Games")),
        _ => None,
    }
}

/// `%Win*%` tokens belong to a game running under Proton, so they
/// resolve inside that game's wine prefix (`drive_c/users/steamuser/...`) — the path
/// the Windows build actually writes to. With no prefix known they resolve to `None`
/// and the file is skipped with a warning.
#[cfg(not(windows))]
pub(super) fn resolve_root_token(
    token: RootToken,
    install_dir: Option<&Path>,
    wine_prefix: Option<&Path>,
) -> Option<PathBuf> {
    // Proton runs the game as the fixed `steamuser` account inside the prefix.
    let win_user = || wine_prefix.map(|p| p.join("drive_c/users/steamuser"));
    match token {
        RootToken::GameInstall => install_dir.map(Path::to_path_buf),
        RootToken::WinMyDocuments | RootToken::WinDocuments => {
            win_user().map(|u| u.join("Documents"))
        }
        RootToken::WinAppDataLocal => win_user().map(|u| u.join("AppData/Local")),
        RootToken::WinAppDataLocalLow => win_user().map(|u| u.join("AppData/LocalLow")),
        RootToken::WinAppDataRoaming => win_user().map(|u| u.join("AppData/Roaming")),
        RootToken::WinSavedGames => win_user().map(|u| u.join("Saved Games")),
        _ => resolve_native_root_token(token),
    }
}

#[cfg(target_os = "macos")]
fn resolve_native_root_token(token: RootToken) -> Option<PathBuf> {
    let home = || std::env::var_os("HOME").map(PathBuf::from);
    match token {
        RootToken::MacHome => home(),
        RootToken::MacAppSupport => home().map(|h| h.join("Library/Application Support")),
        RootToken::MacDocuments => home().map(|h| h.join("Documents")),
        RootToken::MacCaches => home().map(|h| h.join("Library/Caches")),
        _ => None,
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
fn resolve_native_root_token(token: RootToken) -> Option<PathBuf> {
    let home = || std::env::var_os("HOME").map(PathBuf::from);
    match token {
        RootToken::LinuxHome => home(),
        RootToken::LinuxXdgDataHome => std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| home().map(|h| h.join(".local/share"))),
        RootToken::LinuxXdgConfigHome => std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| home().map(|h| h.join(".config"))),
        _ => None,
    }
}

pub fn default_cloud_root(steam_id: u64, appid: u32) -> Result<PathBuf> {
    let home = crate::core::config::home_dir()?;
    let account_id = steam_id as u32;
    let userdata = if cfg!(target_os = "macos") {
        "Library/Application Support/Steam/userdata"
    } else {
        ".local/share/Steam/userdata"
    };
    Ok(home
        .join(userdata)
        .join(account_id.to_string())
        .join(appid.to_string()))
}
