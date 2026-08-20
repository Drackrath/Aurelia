<img src="assets/aurelia_logo_v3.png" alt="Aurelia, the command-line Steam client and launcher written in Rust" title="Aurelia" align="left" height="80" />

# Aurelia: a command-line Steam client for Linux and Windows

**A fast, lightweight, headless Steam launcher and library manager written in Rust. Install, update and play your Steam games entirely from the terminal.**

[![License: GPL-3.0](https://img.shields.io/badge/License-GPL%203.0-blue.svg)](LICENSE)
[![Built with Rust](https://img.shields.io/badge/built%20with-Rust-orange.svg)](https://www.rust-lang.org/)
[![Status: Beta](https://img.shields.io/badge/status-experimental%20beta-yellow.svg)](#project-status)
[![AUR](https://img.shields.io/badge/AUR-aurelia-1793d1.svg)](https://aur.archlinux.org/packages/aurelia)

<br clear="left" />

> [!WARNING]
> **Disclaimer: read before use.**
> Aurelia is an **independent, unofficial project** and is **not affiliated with, authorized, sponsored, or endorsed by Valve or Steam** in any way. "Steam" and "Valve" are trademarks of Valve Corporation.
>
> - **It modifies Steam's files directly.** Doing so may corrupt or damage your Steam installation, potentially forcing a full reinstallation. Back up your data first.
> - **No warranty for games launched outside the official Steam launcher.** Titles started through Aurelia bypass the normal Steam client and may not behave as expected.
>
> Use Aurelia entirely **at your own risk**. The authors accept no liability for damage to your Steam installation, lost data, or banned or suspended accounts.

Aurelia is a pure command-line Steam launcher and library manager. No CEF, no WebViews,
no GUI. It talks to Steam's real network protocols through
[`steam-vent`](https://codeberg.org/steam-vent/steam-vent), so you can log in, manage your
library, install and update games, sync Steam Cloud saves, manage Steam Workshop content,
see your friends and chat with them, and launch titles (natively or through Proton or Wine)
entirely from a terminal or a script.

Because it never needs a desktop session, it runs where the official Steam client can't:
over SSH, on a headless Linux server, in a container, on a low-memory box, or as one step
inside a shell script. Every command speaks `--json`, so it is equally at home as a backend
for another launcher.

It is the modern successor to **OpenSteamClient**, rebuilt in Rust for a smaller footprint,
memory safety, and a scriptable, headless-friendly workflow. It is also a full-fledged
alternative to **SteamCMD**, one that can *launch* the games it installs.

```bash
aurelia login
aurelia list --installed
aurelia install 1245620
aurelia play 1245620
```

---

## Contents

- [Why Aurelia?](#why-aurelia)
- [How it compares](#how-it-compares)
- [Project status](#project-status)
- [Install](#install)
- [Build from source](#build-from-source)
- [Usage](#usage)
- [Configuration](#configuration)
- [Documentation](#documentation)
- [FAQ](#faq)
- [Contributing](#contributing)
- [Acknowledgments](#acknowledgments)
- [License](#license)

---

## Why Aurelia?

- **No web technology.** No Electron, CEF, or embedded browser. Idle memory stays under
  ~50 MB instead of the official Steam app's hundreds.
- **Fast and scriptable.** A pure Rust CLI: instant startup, easy to automate, and every
  command speaks `--json` for machine-readable output.
- **Linux first.** 64-bit clean, with first-class Proton and Wine management, and it runs on
  Windows too. No X11 or Wayland session required for library and download work, so it works
  over SSH and on headless servers.
- **Deep Steam integration.** PICS metadata, the content CDN, Steam Cloud, app ownership
  tickets, depot browsing, DLC management, and Steam Workshop, all built on open, documented
  protocols.
- **Open source.** GPL-3.0 licensed, with no dependency on opaque 32-bit legacy Steam binaries.

### How it compares

| Feature | Official Steam | OpenSteamClient | SteamCMD | **Aurelia** |
|---|---|---|---|---|
| **Architecture** | Electron + C++ | C++ with Qt | C++ (proprietary) | Pure Rust |
| **Idle RAM** | ~400-800 MB | ~100-200 MB | ~50 MB (per run) | < 50 MB |
| **Interface** | Desktop GUI | Desktop GUI | CLI (scriptable) | CLI (scriptable) |
| **Scope** | Everything | Library + launch | Install, update, Workshop | Library, install, launch, Cloud, Workshop, DLC, friends, chat |
| **Download engine** | CDN + P2P LAN | Standard CDN | Standard CDN | Multi-threaded CDN |
| **Authentication** | Full | Core | Full (+ anonymous) | Full (tokens, mobile app, Guard) |
| **Steam integration** | Native | Partial | Content only | Deep (PICS, CDN, Cloud, tickets) |
| **Platforms** | Windows, Linux, macOS | Windows, Linux | Windows, Linux, macOS | Linux (first), Windows |
| **Open source** | No | Yes | No | Yes (GPL-3.0) |

**vs. SteamCMD.** [SteamCMD](https://developer.valvesoftware.com/wiki/SteamCMD) is Valve's
official command-line tool and the closest analog to Aurelia, but it is **content-only**: it
downloads and updates app and Workshop files (often anonymously, for dedicated servers) and
little else. Aurelia is a full launcher and library manager. On top of installing and
updating, it lists and searches your library, **launches** games (natively or via
Proton or Wine), syncs Steam Cloud saves, manages DLC and Workshop subscriptions, reads
achievements, and does friends & chat, with every command scriptable via `--json`. SteamCMD is
proprietary and ships only as a prebuilt binary. Aurelia is open source (GPL-3.0).

---

## Project status

Aurelia is in **experimental beta**. The core is highly functional: authentication,
library management, installs and updates, integrity verification, DLC handling, Steam Cloud
sync, and Proton or Wine launching all work today.

| Area | Status | What works |
|---|---|---|
| **Authentication** | ✅ | Password, Steam Guard (email and device codes), Mobile App confirmation, refresh-token session restore |
| **Library** | ✅ | Fetch owned games, scan local installs, search & filter, Family Sharing |
| **Install & updates** | ✅ | 4-phase download pipeline (manifest → security → chunks), updates, uninstall, integrity verification, and moving installs between library folders, with Steam's `appmanifest` and `libraryfolders.vdf` kept in sync. Installs run in the background daemon and can be listed and cancelled (`install list`, `install stop`) |
| **Version pinning and downgrade** | ✅ | Install & pin a specific depot manifest (`downgrade`, `manifests`, `pin`, `unpin`), holding a game at an older build |
| **Localized metadata** | ✅ | Store text (`info`) and achievement names and descriptions follow a `--lang` flag or the `config language` default, used by the Heroic Steam integration |
| **DLC** | ✅ | Install, enable or disable, and per-DLC ownership and install status |
| **Steam Cloud** | ✅ | Enumerate, download, upload save data |
| **Proton and Wine** | ✅ | Runtime discovery, a download manager (official Valve Proton, GE builds, and **Proton-CachyOS** with AVX2 or `x86_64_v3` microarch selection), automatic **modern unified-layout** discovery (Proton 11+, GE, CachyOS, WOW64-aware) with strict bitness filtering, per-game version pinning, and launch integration. Depot-aware executable selection (native vs Proton), running-game tracking, and graceful or forced stop (`running`, `stop --force`) |
| **Self-contained Windows Steam runtime** | ✅ | Install or repair a master Wine Steam prefix (`steam-runtime install`, `repair`, `stop`, `uninstall`, `status`) to satisfy Steamworks and DRM handshakes without a host Steam client. `play --steam` falls back to it automatically when no host Steam is installed (`config steam-runtime-policy auto\|on\|off`) |
| **Optional launch plugins** | ✅ | Linux, opt-in, never bundled: **luxtorpeda** native engines, and **umu-launcher** (Proton via `umu-run`). Both are downloaded on demand and routed per-game |
| **Steam integration (opt-in)** | ✅ | Launch with real Steam integration (`play --steam`): the host Steam client bridged in, started silently if needed, or the in-Wine Steam runtime when no host Steam exists, for Steamworks and DRM. Auto-enabled on Family-Shared games |
| **Depot browser** | ✅ | List depots, inspect manifest trees, download single files |
| **Workshop** | ✅ | Browse and search, install and uninstall, subscribe, collections, rate, and read or post comments |
| **Friends & chat** | ✅ | Friends roster with live persona status and current game, resolve a SteamID from a profile or vanity URL, send or cancel friend requests, plus direct messaging (send, history, and an interactive live session). Presence is configurable, defaulting to invisible |
| **Inventory & market** | ✅ Read-only | View your inventory, look up item prices, search the Community Market, and see your wallet and listings. Buying & selling are planned |
| **Collections and categorization** | ✅ | Create, rename and delete library collections, add and remove games, a `list` COLLECTIONS column and `--collection` filter, and on-demand pull, push and sync with Steam's cloud collections |

---

## Install

### Arch Linux (AUR)

```bash
yay -S aurelia          # or: paru -S aurelia
```

### Nix and NixOS (flake)

```bash
nix run github:Drackrath/Aurelia          # run it once
nix profile install github:Drackrath/Aurelia   # install it
```

### Debian and Ubuntu (.deb)

Grab the `.deb` for your architecture from the
[latest release](https://github.com/Drackrath/Aurelia/releases/latest):

```bash
sudo apt install ./aurelia_*_amd64.deb
```

### Prebuilt binaries (Linux, Windows, macOS)

Every release ships static-ish binaries for `linux_x86_64`, `linux_arm64`,
`windows_x86_64`, `windows_arm64`, `macOS_x86_64` and `macOS_arm64`.
See the [releases page](https://github.com/Drackrath/Aurelia/releases/latest).

```bash
curl -LO https://github.com/Drackrath/Aurelia/releases/latest/download/aurelia_linux_x86_64
chmod +x aurelia_linux_x86_64 && sudo mv aurelia_linux_x86_64 /usr/local/bin/aurelia
aurelia --help
```

---

## Build from source

### Prerequisites

You'll need a [Rust toolchain](https://rustup.rs/) (edition 2024).

On Linux, install the system dependencies first (Ubuntu 24.04 example):

```bash
sudo apt-get update
sudo apt-get install build-essential pkg-config libssl-dev libx11-dev libxi-dev \
  libxrandr-dev libxinerama-dev libxcursor-dev libxkbcommon-dev libasound2-dev \
  libudev-dev libwayland-dev libgtk-3-dev libpulse-dev libdbus-1-dev \
  libegl1-mesa-dev libgles2-mesa-dev liblzma-dev
```

Windows and macOS need only the Rust toolchain.

### Build

```bash
git clone https://github.com/Drackrath/Aurelia.git
cd Aurelia
cargo build --release
```

The binary is produced at `target/release/aurelia` (`aurelia.exe` on Windows).

---

## Usage

Aurelia is driven entirely from the command line. Run `aurelia --help` for the full list
of subcommands, or `aurelia <command> --help` for a specific one.

```bash
# Account
aurelia login                        # authenticate (prompts for credentials and Steam Guard)
aurelia logout                       # clear the stored session
aurelia account                      # show account details

# Library
aurelia list                         # list your library
aurelia list --installed             # only installed games
aurelia list --search elden          # filter by name
aurelia list --online                # add an ONLINE column (needs-connection heuristic)
aurelia info 690830                  # game details (description, release, reviews, DLC)
aurelia info 690830 --extended       # + requirements, Metacritic, tags, genres, categories
aurelia info 690830 --lang german    # localize store text (falls back to config, then English)
aurelia dlc 690830                   # list a game's DLC with ownership and install status
aurelia achievements 620             # your achievements for a game (unlock state + rarity)
aurelia achievements 620 --lang german  # localize achievement names and descriptions
aurelia image 1245620                # fetch cover art to the cache (prints the path)
aurelia image 1245620 -o cover.jpg   # save artwork to a specific file

# Install & maintain
aurelia install 1245620              # download & install a game by app id
aurelia install 1245620 --library D:\SteamLibrary  # install onto a specific drive or library
aurelia libraries                    # list Steam library folders (one per drive) + free space
aurelia install list                 # show installs running in the daemon (with progress)
aurelia install stop 1245620         # cancel a running install
aurelia update 1245620               # download the latest manifest
aurelia verify 1245620               # verify installed files
aurelia uninstall 1245620            # remove a game (--delete-prefix wipes its prefix)
aurelia move 1245620 D:\SteamLibrary # move an install to another library (updates Steam data)
aurelia relink 1245620 D:\SteamLibrary  # re-point Steam at already-moved files (no copy)
aurelia import 1245620 D:\SteamLibrary  # register existing on-disk files with Steam
aurelia available 1245620            # is it installed and present on disk?
aurelia duplicates                   # games installed in several libraries at once
                                     # (a duplicate makes a game re-report updates forever.
                                     #  prints the copies to delete, removes nothing itself)

# Downgrade & version pinning
aurelia manifests 1245620            # each depot's current manifest id per branch
aurelia downgrade 1245620 --depot 1245621 --manifest 8593343465227540543  # install an older build & pin it
aurelia pin 1245620                  # lock the current install (block Aurelia updates)
aurelia unpin 1245620                # release the pin
# Older manifest ids aren't exposed by Steam. Find them on SteamDB:
#   https://steamdb.info/depot/<depot_id>/manifests/

# DLC
aurelia enable 2001                  # enable an installed DLC for its base game
aurelia disable 2001                 # disable a DLC

# Branches & depots
aurelia branches 1245620             # list beta branches
aurelia set-branch 1245620 beta      # switch branch
aurelia depots 1245620               # list depots
aurelia launch-options 1245620       # list Steam launch configs (exe, args, platform)

# Launch
aurelia play 1245620                 # launch a game and wait for it to exit
aurelia play 1245620 --proton experimental   # Linux: force a specific Proton or Wine runner
aurelia play 1245620 --steam         # run with Steam online features (Family Sharing, DRM)
aurelia running                      # list games Aurelia is currently running
aurelia stop 1245620                 # stop a running game (--force to kill a hung one)

# Steam Cloud
aurelia cloud sync 1245620           # sync saves (down then up)
aurelia cloud list 1245620           # list a game's Cloud files

# Steam Workshop
aurelia workshop browse 1245620            # discover items (search, sort, paginate)
aurelia workshop info 1234567890           # item or collection metadata
aurelia workshop install 1234567890        # download an item (collections expand to members)
aurelia workshop subscribe 1234567890 --install  # subscribe, then download
aurelia workshop status 1245620            # installed vs subscribed (+ update detection)
aurelia workshop rate 1234567890 up        # thumbs-up (or: down) an item
aurelia workshop comments 1234567890       # read an item's comments
aurelia workshop comment 1234567890 "Nice mod!"  # post a comment

# Friends & chat
aurelia friends                              # list friends (name, status, current game)
aurelia friends search gabelogannewell       # resolve a SteamID (id, profile URL or vanity)
aurelia friends add 76561197960287930        # send a friend request (accepts a URL too)
aurelia friends remove 76561197960287930     # remove a friend or cancel a request
aurelia chat send 76561198042323314 "hi!"    # send a direct message to a friend
aurelia chat history 76561198042323314       # show recent messages with a friend
aurelia chat open 76561198042323314          # interactive live chat (type to send, Ctrl-D quits)

# Inventory & market
aurelia inventory 753 --context 6            # your Steam cards, gems and backgrounds
aurelia market price 440 "Mann Co. Supply Crate Key"   # item price (no login needed)
aurelia market search "Sticker" --app-id 730 # search the Community Market
aurelia market listings                      # your active listings & buy orders
aurelia wallet                               # Steam Wallet balance

# Configuration
aurelia config show                  # print launcher configuration
aurelia config protons               # list detected Proton and Wine runtimes
aurelia config presence online       # appear online for chat (default: invisible)
aurelia config language german       # default language for info and achievements text
aurelia config game 1245620 --proton GE-Proton9-20  # pin a Proton version for one game

# Proton and Wine runtimes (download manager)
aurelia proton list                  # installable runtimes (Valve + GE + CachyOS) and what's installed
aurelia proton install GE-Proton9-20 # download a GE build (or "Proton 9.0" via Steam)
aurelia proton install Proton-CachyOS # CachyOS build (auto-picks x86_64_v3 or AVX2 when supported)
aurelia proton default GE-Proton9-20 # set the global default (used when a game has none set)
aurelia proton uninstall GE-Proton9-19  # delete an installed GE build

# Windows Steam runtime (self-contained Steamworks and DRM handshake, no host Steam client)
aurelia config steam-runtime-runner GE-Proton9-20  # select the Wine or Proton runner (required first)
aurelia steam-runtime status          # resolved master prefix, layout, steam.exe presence
aurelia steam-runtime install         # install Steam into the master Wine prefix (sign in here)
aurelia steam-runtime install --reinstall  # delete the prefix first, then install fresh
                                           # (for a corrupted install, no .bak is kept)
aurelia steam-runtime login           # re-open the in-Wine Steam to sign in again
aurelia steam-runtime repair          # back up the prefix (keep one) and reinstall
aurelia steam-runtime stop            # shut down the in-Wine Steam, keeping the prefix
aurelia steam-runtime uninstall       # remove the master prefix entirely (incl. any .bak)
aurelia config steam-runtime-policy on   # make `play --steam` always use the in-Wine runtime
                                         # (default `auto`: host Steam if present, else in-Wine)

# Collections (library categories): edit locally offline, sync to Steam on demand
aurelia collections list                     # all collections + game counts
aurelia collections create "RPGs"            # new (static) collection
aurelia collections add "RPGs" 570 730       # add games by app id
aurelia collections remove "RPGs" 730        # drop a game
aurelia collections show "RPGs"              # list a collection's games
aurelia list --collection "RPGs"             # filter the library to one collection
aurelia collections pull                     # fetch Steam's collections and merge them in
aurelia collections push --yes               # upload local collections to your Steam account
aurelia collections sync --yes               # pull then push (reconcile both sides)

# umu-launcher plugin (Linux only, optional: Proton via umu-run, downloaded on demand)
aurelia umu enable                    # turn the plugin on (off by default)
aurelia umu install                   # download umu-run on demand (not bundled)
aurelia umu path ~/umu                # use an existing install instead (skips the download)
aurelia umu status                    # show enabled state + installed version
aurelia config game 1245620 --umu     # route one game through umu (Proton via umu-run)
aurelia play 1245620 --umu            # one-off launch via umu
aurelia umu uninstall                 # remove the downloaded payload

# Luxtorpeda native-engine plugin (Linux only, optional)
aurelia luxtorpeda enable             # turn the plugin on (off by default)
aurelia luxtorpeda install            # download the client on demand (not bundled)
aurelia luxtorpeda path ~/luxtorpeda  # use an existing install instead (skips the download)
aurelia luxtorpeda status             # show enabled state + installed version
aurelia config game 2270 --native-engine   # route one game through a native engine
aurelia play 2270 --native-engine     # one-off launch via luxtorpeda
aurelia luxtorpeda uninstall          # remove the downloaded payload

# Per-game launch scripts (wrap the resolved launch command with your own script)
aurelia scripts new 2270              # scaffold <script_dir>/2270.sh (2270.bat on Windows)
aurelia scripts list                  # app ids with a script + resolved paths
aurelia scripts show 2270             # print the resolved script + its contents
aurelia play 2270                     # runs through the script (e.g. gamemoderun, mangohud)
aurelia config game 2270 --launch-script ~/my/wrap.sh   # pin a specific script per game
aurelia play 2270 --script ~/other.sh # one-off override for a single launch
aurelia play 2270 --no-script         # bypass all scripts for this launch
aurelia scripts remove 2270           # delete the dir-based script
```

> [!NOTE]
> **Luxtorpeda** is an optional plugin that runs supported games on native Linux engines
> (GZDoom, OpenMW, …) instead of Proton or Wine. It is **never bundled**. Aurelia downloads it
> on the fly into `~/.config/Aurelia/plugins/luxtorpeda` only when you enable the feature and
> opt a game in, so the binary stays lean. Linux only. Games run outside Steam's runtime
> container, so if an engine can't find system libraries, prefer Proton for that title.

<!-- -->

> [!NOTE]
> **umu-launcher** is an optional plugin that runs Windows games through Proton **outside**
> Steam (applying the Steam Linux Runtime and per-game protonfixes), wrapping the launch with
> `umu-run` instead of replacing the runtime. Like luxtorpeda it is **never bundled**.
> Aurelia downloads it on the fly into `~/.config/Aurelia/plugins/umu` only when you enable the
> feature and opt a game in, so the binary stays lean. Linux only. It **wraps Proton** rather
> than replacing it, so `--umu` combines with `--proton` to pick the Proton build it runs.

<!-- -->

> [!NOTE]
> **Per-game launch scripts** let you wrap the fully-resolved launch command with your own
> shell script (`<script_dir>/<appid>.sh`, or `.bat` on Windows). Aurelia runs the script with
> the resolved command as its arguments (`"$@"`) and exports `AURELIA_*` env vars, so a script
> that is just `exec "$@"` is a passthrough while a custom one can prepend `gamemoderun`,
> `mangohud` or `gamescope`. It works uniformly for native, Proton, luxtorpeda and umu launches.
> Resolution precedence: `play --script <path>` beats `config game --launch-script <path>`,
> which beats the auto-detected `<script_dir>/<appid>.sh`. `play --no-script` bypasses all of them.

Add `--json` to any command for machine-readable output (errors included). It's a global
flag, so `aurelia --json <command>` and `aurelia <command> --json` are equivalent.

📖 **See [USAGE.md](USAGE.md) for complete documentation of every command and option.**

---

## Configuration

Aurelia stores its configuration and local data under `~/.config/Aurelia`
(`%USERPROFILE%\.config\Aurelia` on Windows). Set **`AURELIA_CONFIG_DIR`** to relocate it.
That is useful for an embedding driver (e.g. Heroic) that needs Aurelia's state isolated from
a standalone install.

- **Library path.** Aurelia auto-detects your existing Steam installation. Inspect the
  resolved configuration with `aurelia config show`.
- **Session.** Refresh tokens are persisted in `session.json` so subsequent invocations
  log in automatically.
- **Unified download pipeline.** Installs, updates, and verifications all run through a
  single, robust engine for reliability and speed.

---

## Documentation

| Document | Contents |
|---|---|
| [USAGE.md](USAGE.md) | Full reference for every command and flag |
| [WINDOWS_STEAM_RUNTIME.md](WINDOWS_STEAM_RUNTIME.md) | The self-contained Wine Steam prefix (`steam-runtime`) |
| [CONTRIBUTING.md](CONTRIBUTING.md) | How to contribute |
| [SECURITY.md](SECURITY.md) | Reporting a vulnerability |
| [FILES_REVIEWED.md](FILES_REVIEWED.md) | Manual review checklist for the latest code-review changes |

---

## FAQ

### Is there a Steam client that works without a GUI?

Yes. That is exactly what Aurelia is. It is a pure CLI Steam client: no Electron, no CEF, no
embedded browser, no desktop session required. You can log in, install games, update them,
sync Cloud saves and launch titles from a terminal or a shell script.

### Can I install and play Steam games over SSH or on a headless server?

Yes. Library management, downloads, updates, verification, Cloud sync and Workshop all work
headlessly over SSH. Launching a game still needs somewhere for it to render (a display,
a virtual X server, or a remote-play setup), but everything up to the launch does not.

### How is Aurelia different from SteamCMD?

[SteamCMD](https://developer.valvesoftware.com/wiki/SteamCMD) is Valve's official CLI tool,
but it is content-only: it downloads and updates app and Workshop files and little else.
Aurelia is a full launcher and library manager. It lists and searches your library, launches
games natively or through Proton and Wine, syncs Steam Cloud saves, manages DLC and Workshop
subscriptions, reads achievements, and does friends & chat. SteamCMD is proprietary and ships
as a prebuilt binary. Aurelia is open source under GPL-3.0.

### Is Aurelia an OpenSteamClient alternative?

It is its spiritual successor. OpenSteamClient is a C++ and Qt desktop GUI. Aurelia rebuilds
the same idea in Rust as a scriptable CLI, without the 32-bit legacy Steam binaries, at a
fraction of the memory footprint.

### Does it run Windows games on Linux?

Yes, through Proton and Wine. Aurelia discovers installed runtimes, downloads new ones
(official Valve Proton, GE-Proton, and Proton-CachyOS with AVX2 or `x86_64_v3` selection),
pins a Proton version per game, and can route launches through
[umu-launcher](https://github.com/Open-Wine-Components/umu-launcher) or native engines via
[luxtorpeda](https://github.com/luxtorpeda-dev/luxtorpeda).

### Do I need the official Steam client installed?

No. Aurelia talks to Steam's real network protocols directly. For titles that insist on a
Steamworks or DRM handshake, it can either bridge to a host Steam client if you have one, or
install a self-contained Windows Steam runtime inside its own Wine prefix
(`aurelia steam-runtime install`).

### How much RAM does it use?

Under ~50 MB idle, against roughly 400 to 800 MB for the official Steam desktop app.

### Which platforms are supported?

Linux is the primary target (x86_64 and arm64), Windows is supported, and macOS binaries are
built for each release. See [Install](#install).

### Can I use it to script my Steam library?

Yes. Every command accepts `--json` for machine-readable output, including errors, so
Aurelia works as a backend for other launchers and for automation. `AURELIA_CONFIG_DIR`
relocates its state so an embedding driver can keep it isolated.

### Is it safe to use? Can I get VAC banned?

Aurelia is unofficial and unaffiliated with Valve, it modifies Steam's files directly, and
third-party tools that interact with Steam carry a risk of action against your account.
Read the disclaimer at the top of this README before using it. Use at your own risk.

---

## Contributing

Contributions are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines. By
participating, you agree to abide by our [Code of Conduct](CODE_OF_CONDUCT.md).

## Acknowledgments

Aurelia grew directly out of **[SteamFlow](https://github.com/weter11/SteamFlow)**, the
earlier project it is derived from and the foundation this work is built on. Our deepest
thanks to its author: SteamFlow did the hard groundwork that made Aurelia possible.

It stands, in turn, on **[steam-vent](https://codeberg.org/steam-vent/steam-vent)** and
**[steam-vent-chat](https://codeberg.org/steam-vent/chat)**, whose reverse-engineering and
protocol work let Aurelia speak Steam's real network protocols, and on a vendored, modified
`steam-cdn` (plus the `zip` crate) for the content pipeline.

### Credits

- [SteamFlow](https://github.com/weter11/SteamFlow): the project Aurelia is derived from. Its groundwork is the base everything here is built on. Thank you!
- [steam-vent](https://codeberg.org/steam-vent/steam-vent): Steam network protocol implementation
- [steam-vent-chat](https://codeberg.org/steam-vent/chat): Steam Chat protocol implementation
- [steam-cdn](https://crates.io/crates/steam-cdn): content-delivery and depot download engine (vendored & modified)
- [SteamKit2](https://github.com/SteamRE/SteamKit): Steam .NET research code
- [SteamHelper-rs](https://github.com/saskenuba/SteamHelper-rs): SteamKit Rust port

---

## Star History

<a href="https://www.star-history.com/?repos=Drackrath%2FAurelia&type=date&legend=top-left">
 <picture>
   <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/chart?repos=Drackrath/Aurelia&type=date&theme=dark&legend=top-left&sealed_token=zXdjs_drcHTMkMNbox0hJui2MeqHrik6ffskwkRLfX2hsH8W9nwd9pZ-yEhuBwUuC1WC1pfLVnL7hPyZ5DLdGtDMCxSJh7Kddu-w0eZI-5wM64y56opayjFg_zN1x0rFeHGE6RJh4rG56LXWpv6beBZ42a7cIM0IlxfTeegy72K2xgtbewqQAccsI8b2" />
   <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/chart?repos=Drackrath/Aurelia&type=date&legend=top-left&sealed_token=zXdjs_drcHTMkMNbox0hJui2MeqHrik6ffskwkRLfX2hsH8W9nwd9pZ-yEhuBwUuC1WC1pfLVnL7hPyZ5DLdGtDMCxSJh7Kddu-w0eZI-5wM64y56opayjFg_zN1x0rFeHGE6RJh4rG56LXWpv6beBZ42a7cIM0IlxfTeegy72K2xgtbewqQAccsI8b2" />
   <img alt="Star History Chart" src="https://api.star-history.com/chart?repos=Drackrath/Aurelia&type=date&legend=top-left&sealed_token=zXdjs_drcHTMkMNbox0hJui2MeqHrik6ffskwkRLfX2hsH8W9nwd9pZ-yEhuBwUuC1WC1pfLVnL7hPyZ5DLdGtDMCxSJh7Kddu-w0eZI-5wM64y56opayjFg_zN1x0rFeHGE6RJh4rG56LXWpv6beBZ42a7cIM0IlxfTeegy72K2xgtbewqQAccsI8b2" />
 </picture>
</a>

---

## Related projects

- [SteamCMD](https://developer.valvesoftware.com/wiki/SteamCMD): Valve's official,
  content-only command-line tool
- [OpenSteamClient](https://github.com/OpenSteamClient/OpenSteamClient): open-source C++ and
  Qt Steam client. Aurelia is its Rust CLI successor
- [SteamFlow](https://github.com/weter11/SteamFlow): the project Aurelia is derived from
- [steam-vent](https://codeberg.org/steam-vent/steam-vent): the Steam network protocol
  implementation Aurelia is built on
- [Heroic Games Launcher](https://github.com/Heroic-Games-Launcher/HeroicGamesLauncher):
  GUI launcher that can drive Aurelia for its Steam integration
- [umu-launcher](https://github.com/Open-Wine-Components/umu-launcher): optional launch plugin
  that runs Proton outside Steam
- [luxtorpeda](https://github.com/luxtorpeda-dev/luxtorpeda): optional launch plugin for
  native Linux engines

*Topics: steam client cli, headless steam, steam launcher linux, steamcmd alternative,
open source steam client, terminal steam, install steam games from command line,
proton launcher cli, steam library manager, rust steam client.*

---

## License

Aurelia is released under the [GPL-3.0 License](LICENSE).
