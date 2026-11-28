//! `config` subcommands and game config args.

use std::path::PathBuf;

use clap::Subcommand;

use super::*;

#[derive(Subcommand)]
pub(crate) enum ConfigCommand {
    /// Print the current launcher configuration as JSON.
    Show,
    /// List detected Proton/Wine runtimes.
    Protons,
    /// View or set the Steam presence the daemon announces for friends/chat.
    Presence {
        /// `online`, or `offline` (invisible: you appear offline but still sync
        /// friends and receive chat). Omit to print the current setting.
        mode: Option<ChatPresenceArg>,
    },
    /// View or set the default Steam API language.
    #[command(alias = "lang")]
    Language {
        /// The default Steam API language name (e.g. `german`, `french`,
        /// `schinese`) used by `aurelia achievements` when `--lang` is not
        /// given. Omit the value to print the current setting.
        lang: Option<String>,
    },
    /// View or set `user` status wait.
    PersonaWait {
        /// Seconds (1-300); omit to print.
        #[arg(value_parser = clap::value_parser!(u64).range(1..=300))]
        seconds: Option<u64>,
    },
    /// View or set DLC cache seconds.
    DlcCache {
        /// DLC list seconds (default 86400).
        #[arg(long, value_parser = clap::value_parser!(u64).range(0..=2_592_000))]
        list: Option<u64>,
        /// Ownership seconds (default 3600).
        #[arg(long, value_parser = clap::value_parser!(u64).range(0..=2_592_000))]
        owned: Option<u64>,
    },
    /// View or set the price country.
    #[command(alias = "cc")]
    Country {
        /// Two-letter ISO code; empty clears, omitted prints.
        country: Option<String>,
    },
    /// View or set the experimental-features gate.
    ///
    /// When enabled, unlocks experimental commands — currently `login --openid`
    /// (browser identity check) and `login --web-token` (web-surface auth). These
    /// prove identity or enable web-only commands (inventory/wallet/market) but
    /// cannot create the full client session that library/install/launch need.
    /// The `AURELIA_EXPERIMENTAL=1` environment variable enables them for a single
    /// run without persisting this setting. Omit the value to print it.
    Experimental {
        /// `true` or `false`. Omit to print the current setting.
        enabled: Option<bool>,
    },
    /// View or set the Wine/Proton runner that hosts the Windows Steam runtime.
    ///
    /// Used by `steam-runtime install`/`repair` to drive `SteamSetup.exe` and the
    /// background Steam client under a bare Wine. Accepts an installed runtime name
    /// (see `aurelia proton list`) such as `GE-Proton9-20` or `experimental`, or an
    /// absolute path to a Wine build. A Proton tree is fine — its bundled bare Wine
    /// is used automatically. Pass an empty value to clear it.
    SteamRuntimeRunner {
        /// Installed runtime name or absolute Wine path. Omit to print the current
        /// setting; pass `""` to clear it.
        runner: Option<String>,
    },
    /// View or set the global default Steam-integration policy for launches.
    ///
    /// Applied when a game's own policy (`config game <id> --steam-runtime …`) is
    /// `auto`. Controls how `aurelia play --steam` provides Steam DRM / Steamworks:
    /// `auto` prefers the host Steam client and falls back to the in-Wine Steam
    /// runtime when no host Steam is installed; `on` always uses the in-Wine runtime;
    /// `off` never does (host Steam only). Omit the value to print the current setting.
    SteamRuntimePolicy {
        /// `auto`, `on`, or `off`. Omit to print the current setting.
        policy: Option<SteamRuntimeArg>,
    },
    /// View or set whether the daemon resumes interrupted installs when it starts.
    /// `manual` (default) waits for `install resume`; `auto` resumes `interrupted`
    /// backlog entries, never paused or stopped ones.
    InstallResume {
        /// `manual` or `auto`. Omit to print the current setting.
        mode: Option<InstallResumeArg>,
    },
    /// View or set the global download limit.
    ///
    /// Caps all game downloads together, in kbit/s (8000 = 1 MB/s). `0` or `off`
    /// removes the limit. Running downloads pick up a change within seconds.
    DownloadLimit {
        /// kbit/s, or `off`. Omit to print.
        #[arg(value_parser = parse_download_limit)]
        kbit: Option<u64>,
    },
    /// View or set Vulkan shader pre-caching before `play` (Linux). `auto` (default)
    /// replays the game's Steam pipeline cache with `fossilize_replay` and points the
    /// driver caches at `steamapps/shadercache/<appid>`, like Steam; `off` leaves
    /// shaders to the game.
    ShaderPrecache {
        /// `auto` or `off`. Omit to print the current setting.
        mode: Option<ShaderPrecacheArg>,
    },
    /// View or set whether `launch-options` hides entries whose executable isn't
    /// installed, for installed games. `on` (default) lists only entries `play` can
    /// start; `off` lists every entry, with `installed` in `--json`.
    HideUninstalledLaunchOptions {
        /// `on` or `off`. Omit to print the current setting.
        mode: Option<OnOffArg>,
    },
    /// Steam Cloud sync around `play`.
    CloudSync {
        /// `on` or `off`; omit to print.
        mode: Option<OnOffArg>,
    },
    /// View or set whether `play` auto-installs a missing Proton anti-cheat
    /// runtime (EasyAntiCheat / BattlEye) for a game that ships one. `on`
    /// (default) downloads it before the Proton launch, like Steam; `off` never
    /// downloads (install manually with `aurelia proton anticheat`).
    AnticheatAutoInstall {
        /// `on` or `off`. Omit to print the current setting.
        mode: Option<OnOffArg>,
    },
    /// View or set the network proxy used for all HTTP(S) communication.
    ///
    /// Applies to the Steam web endpoints, depot downloads, and Proton/plugin
    /// release lookups. Takes effect on the next command (and requires restarting
    /// the session daemon). An explicit `HTTP(S)_PROXY`/`ALL_PROXY` environment
    /// variable still overrides this. The Steam CM connection is not proxied.
    Proxy {
        /// Proxy URL, e.g. `http://host:8080`, `http://user:pass@host:8080`, or
        /// `socks5://host:1080`. Omit to print the current setting.
        url: Option<String>,
        /// Comma-separated hosts/domains that bypass the proxy (`NO_PROXY`), e.g.
        /// `localhost,127.0.0.1,.internal`.
        #[arg(short = 'n', long, value_name = "LIST")]
        no_proxy: Option<String>,
        /// Clear the configured proxy (revert to a direct connection).
        #[arg(short = 'c', long, conflicts_with_all = ["url", "no_proxy"])]
        clear: bool,
    },
    /// View or set per-game launch settings (Proton version, platform).
    Game {
        #[command(flatten)]
        args: GameConfigArgs,
    },
    /// Reset the per-game settings of ALL games to defaults (both stores).
    ClearGames {
        /// Skip the confirmation prompt.
        #[arg(short = 'y', long)]
        yes: bool,
    },
}

/// Grouped `config game` flags.
#[derive(clap::Args)]
pub(crate) struct GameConfigArgs {
    pub(crate) app_id: u32,
    /// Set the Proton/Wine version this game launches with. Use a name from
    /// `aurelia proton list` (installed). Overrides the global default.
    #[arg(short = 'p', long)]
    pub(crate) proton: Option<String>,
    /// Clear the per-game Proton version (fall back to the global default).
    #[arg(short = 'P', long, conflicts_with = "proton")]
    pub(crate) clear_proton: bool,
    /// Force the game's platform target (`windows` runs through Proton on Linux).
    /// On Linux, `play` then runs only that build: a build Steam swapped for the
    /// other platform is installed again first (not with `--no-update`), and a
    /// missing one fails. `play --platform` overrides this for one launch.
    #[arg(short = 't', long)]
    pub(crate) platform: Option<PlatformArg>,
    /// Clear the platform preference (back to auto-detection).
    #[arg(short = 'T', long, conflicts_with = "platform")]
    pub(crate) no_platform: bool,
    /// Reset ALL of this game's per-game settings to defaults (both stores).
    #[arg(long, conflicts_with_all = ["proton", "clear_proton", "platform", "no_platform",
        "native_engine", "no_native_engine", "umu", "no_umu", "launch_script",
        "no_launch_script", "steam_runtime", "steam_prefix_mode", "launch_options",
        "no_launch_options", "cloud_sync", "prefix", "no_prefix"])]
    pub(crate) clear: bool,
    /// Route this game through the luxtorpeda native-engine plugin (Linux only;
    /// requires `aurelia luxtorpeda enable`).
    #[arg(short = 'n', long)]
    pub(crate) native_engine: bool,
    /// Clear the luxtorpeda routing (back to Aurelia's normal native/Proton selection).
    #[arg(short = 'N', long, conflicts_with = "native_engine")]
    pub(crate) no_native_engine: bool,
    /// Route this game through the umu-launcher plugin (Proton via umu; Linux only;
    /// requires `aurelia umu enable`).
    #[arg(short = 'u', long, conflicts_with_all = ["native_engine", "no_native_engine"])]
    pub(crate) umu: bool,
    /// Clear the umu routing (back to Aurelia's normal native/Proton selection).
    #[arg(short = 'U', long, conflicts_with = "umu")]
    pub(crate) no_umu: bool,
    /// Set a per-game launch script that wraps the resolved launch command. See
    /// `aurelia scripts`. Overrides the auto-detected `<script_dir>/<app_id>.sh`.
    #[arg(short = 's', long, value_name = "PATH")]
    pub(crate) launch_script: Option<PathBuf>,
    /// Clear the per-game launch script (falls back to the auto-detected script).
    #[arg(short = 'S', long, conflicts_with = "launch_script")]
    pub(crate) no_launch_script: bool,
    /// Use the self-contained Windows Steam runtime for this game: `on` starts the
    /// master Steam client in Wine to satisfy Steamworks/DRM handshakes without the
    /// host Steam client. Requires `aurelia config steam-runtime-runner` and
    /// `aurelia steam-runtime install`. `auto` is the default (off).
    #[arg(short = 'r', long, value_name = "auto|on|off")]
    pub(crate) steam_runtime: Option<SteamRuntimeArg>,
    /// How the master Steam prefix backs this game: `shared` runs it in the master
    /// prefix directly; `per-game` copies Steam into the game's own prefix.
    #[arg(short = 'm', long, value_name = "shared|per-game")]
    pub(crate) steam_prefix_mode: Option<SteamPrefixModeArg>,
    /// Steam-style launch options for this game, e.g.
    /// `"PROTON_LOG=1 nice -n 5 %command% -windowed"`: leading `VAR=value` set the
    /// environment, words before `%command%` wrap the game, words after it (or
    /// all words without it) are game arguments.
    #[arg(long, value_name = "LINE", allow_hyphen_values = true)]
    pub(crate) launch_options: Option<String>,
    /// Clear this game's launch options.
    #[arg(long, conflicts_with = "launch_options")]
    pub(crate) no_launch_options: bool,
    /// Per-game Steam Cloud sync on `play`.
    #[arg(long, value_name = "auto|on|off")]
    pub(crate) cloud_sync: Option<CloudSyncArg>,
    /// Use DIR instead of Steam's compatdata folder for this game
    /// (`STEAM_COMPAT_DATA_PATH`; the Wine prefix is `DIR/pfx`). Launches and
    /// `cloud sync` both use it.
    #[arg(long, value_name = "DIR")]
    pub(crate) prefix: Option<PathBuf>,
    /// Go back to Steam's compatdata folder.
    #[arg(long, conflicts_with = "prefix")]
    pub(crate) no_prefix: bool,
}
