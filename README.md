<img src="assets/aurelia_logo.png" alt="Aurelia logo" title="Aurelia" align="left" height="80" />

# Aurelia

**A fast, lightweight, command-line Steam launcher and library manager written in Rust.**

[![License: GPL-3.0](https://img.shields.io/badge/License-GPL%203.0-blue.svg)](LICENSE)
[![Built with Rust](https://img.shields.io/badge/built%20with-Rust-orange.svg)](https://www.rust-lang.org/)
[![Status: Alpha](https://img.shields.io/badge/status-active%20alpha-yellow.svg)](#project-status)
[![AUR](https://img.shields.io/badge/AUR-aurelia-1793d1.svg)](https://aur.archlinux.org/packages/aurelia)

<br clear="left" />

> [!WARNING]
> **Disclaimer — read before use.**
> Aurelia is an **independent, unofficial project** and is **not affiliated with, authorized, sponsored, or endorsed by Valve or Steam** in any way. "Steam" and "Valve" are trademarks of Valve Corporation.
>
> - **It modifies Steam's files directly.** Doing so may corrupt or damage your Steam installation, potentially forcing a full reinstallation. Back up your data first.
> - **No warranty for games launched outside the official Steam launcher.** Titles started through Aurelia bypass the normal Steam client and may not behave as expected.
> - **Risk of VAC bans.** Use of third-party tools that interact with Steam may cause VAC (Valve Anti-Cheat) to flag any user account associated with Aurelia. **Accounts used with Aurelia may be banned.**
>
> Use Aurelia entirely **at your own risk**. The authors accept no liability for damage to your Steam installation, lost data, or banned/suspended accounts.

<!-- -->

> [!NOTE]
> Manual review checklist for the latest code-review changes: [FILES_REVIEWED.md](FILES_REVIEWED.md).


Aurelia is a pure command-line Steam launcher and library manager — no CEF, no WebViews,
no GUI. It talks to Steam's real network protocols through
[`steam-vent`](https://codeberg.org/steam-vent/steam-vent), so you can log in, manage your
library, install and update games, sync Steam Cloud saves, manage Steam Workshop content,
see your friends and chat with them, and launch titles (natively or through Proton/Wine)
entirely from a terminal or a script.

It is the modern successor to **OpenSteamClient**, rebuilt in Rust for a smaller footprint,
memory safety, and a scriptable, headless-friendly workflow.

```bash
aurelia login
aurelia list --installed
aurelia install 1245620
aurelia play 1245620
```

---

## Why Aurelia?

- **No web technology.** No Electron, CEF, or embedded browser — idle memory stays under
  ~50 MB instead of the official Steam app's hundreds.
- **Fast and scriptable.** A pure Rust CLI: instant startup, easy to automate, and every
  command speaks `--json` for machine-readable output.
- **Linux first.** 64-bit clean, with first-class Proton/Wine management — and it runs on
  Windows too.
- **Deep Steam integration.** PICS metadata, the content CDN, Steam Cloud, app ownership
  tickets, depot browsing, DLC management, and Steam Workshop — built on open, documented
  protocols.
- **Open source.** GPL-3.0 licensed, with no dependency on opaque 32-bit legacy Steam binaries.

### How it compares

| Feature | Official Steam | OpenSteamClient | SteamCMD | **Aurelia** |
|---|---|---|---|---|
| **Architecture** | Electron + C++ | C++ / Qt | C++ (proprietary) | Pure Rust |
| **Idle RAM** | ~400–800 MB | ~100–200 MB | ~50 MB (per run) | < 50 MB |
| **Interface** | Desktop GUI | Desktop GUI | CLI (scriptable) | CLI (scriptable) |
| **Scope** | Everything | Library + launch | Install/update + Workshop | Library, install, launch, Cloud, Workshop, DLC, friends/chat |
| **Download engine** | CDN + P2P LAN | Standard CDN | Standard CDN | Multi-threaded CDN |
| **Authentication** | Full | Core | Full (+ anonymous) | Full (tokens, mobile app, Guard) |
| **Steam integration** | Native | Partial | Content only | Deep (PICS, CDN, Cloud, tickets) |
| **Platforms** | Windows, Linux, macOS | Windows, Linux | Windows, Linux, macOS | Linux (first), Windows |
| **Open source** | No | Yes | No | Yes (GPL-3.0) |

**vs. SteamCMD.** [SteamCMD](https://developer.valvesoftware.com/wiki/SteamCMD) is Valve's
official command-line tool and the closest analog to Aurelia, but it is **content-only**: it
downloads and updates app and Workshop files (often anonymously, for dedicated servers) and
little else. Aurelia is a full launcher and library manager — on top of installing and
updating, it lists and searches your library, **launches** games (natively or via
Proton/Wine), syncs Steam Cloud saves, manages DLC and Workshop subscriptions, reads
achievements, and does friends & chat — every command scriptable with `--json`. SteamCMD is
proprietary and ships only as a prebuilt binary; Aurelia is open source (GPL-3.0).

---

## Project status

Aurelia is in **active alpha**. The core is highly functional: authentication,
library management, installs/updates, integrity verification, DLC handling, Steam Cloud
sync, and Proton/Wine launching all work today.

- [x] **Authentication** — password, Steam Guard (email/device codes), Mobile App
      confirmation, refresh-token session restore
- [x] **Library** — fetch owned games, scan local installs, search & filter, Family Sharing
- [x] **Install & updates** — 4-phase download pipeline (manifest → security → chunks),
      updates, uninstall, integrity verification, and moving installs between library
      folders (with Steam's `appmanifest`/`libraryfolders.vdf` kept in sync); installs run
      in the background daemon and can be listed and cancelled (`install list` / `install stop`)
- [x] **Version pinning / downgrade** — install & pin a specific depot manifest
      (`downgrade` / `manifests` / `pin` / `unpin`), holding a game at an older build
- [x] **Localized metadata** — store text (`info`) and achievement names/descriptions follow
      a `--lang` flag or the `config language` default (used by the Heroic Steam integration)
- [x] **DLC** — install, enable/disable, and per-DLC ownership/install status
- [x] **Steam Cloud** — enumerate, download, upload save data
- [x] **Proton/Wine** — runtime discovery, a download manager (official Valve Proton, GE
      builds, and **Proton-CachyOS** with AVX2/`x86_64_v3` microarch selection), automatic
      **modern unified-layout** discovery (Proton 11+/GE/CachyOS, WOW64-aware) with strict
      bitness filtering, per-game version pinning, and launch integration; depot-aware
      executable selection (native vs Proton), running-game tracking, and graceful/forced
      stop (`running` / `stop --force`)
- [x] **Self-contained Windows Steam runtime** — install/repair a master Wine Steam prefix
      (`steam-runtime install` / `repair` / `status`) to satisfy Steamworks/DRM handshakes
      without bridging the host Steam client
- [x] **Optional launch plugins (Linux, opt-in, never bundled)** — **luxtorpeda** native
      engines, and **umu-launcher** (Proton via `umu-run`); both downloaded on demand and
      routed per-game
- [x] **Steam integration (opt-in)** — launch with the host Steam client bridged in
      (`play --steam`, Steam started silently if needed) for Steamworks online features;
      required for and auto-enabled on Family-Shared games
- [x] **Depot browser** — list depots, inspect manifest trees, download single files
- [x] **Workshop** — browse/search, install/uninstall, subscribe, collections, rate, and
      read/post comments
- [x] **Friends & chat** — friends roster with live persona status and current game,
      resolve a SteamID from a profile/vanity URL and send/cancel friend requests, plus
      direct messaging (send, history, and an interactive live session); presence is
      configurable (defaults to invisible)
- [x] **Inventory & market (read-only)** — view your inventory, look up item prices, search
      the Community Market, and see your wallet and listings (buying & selling are planned —
      see [docs/community-market-plan.md](docs/community-market-plan.md))
- [x] **Collections / categorization** — create/rename/delete library collections, add/remove
      games, a `list` COLLECTIONS column and `--collection` filter, and on-demand pull/push/sync
      with Steam's cloud collections

---

## Getting started

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
aurelia login                        # authenticate (prompts for credentials / Steam Guard)
aurelia logout                       # clear the stored session
aurelia account                      # show account details

# Library
aurelia list                         # list your library
aurelia list --installed             # only installed games
aurelia list --search elden          # filter by name
aurelia list --online                # add an ONLINE column (needs-connection heuristic)
aurelia info 690830                  # game details (description, release, reviews, DLC)
aurelia info 690830 --extended       # + requirements, Metacritic, tags, genres, categories
aurelia info 690830 --lang german    # localize store text (falls back to config/English)
aurelia dlc 690830                   # list a game's DLC with ownership/install status
aurelia achievements 620             # your achievements for a game (unlock state + rarity)
aurelia achievements 620 --lang german  # localize achievement names/descriptions
aurelia image 1245620                # fetch cover art to the cache (prints the path)
aurelia image 1245620 -o cover.jpg   # save artwork to a specific file

# Install & maintain
aurelia install 1245620              # download & install a game by app id
aurelia install 1245620 --library D:\SteamLibrary  # install onto a specific drive/library
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

# Downgrade & version pinning
aurelia manifests 1245620            # each depot's current manifest id per branch
aurelia downgrade 1245620 --depot 1245621 --manifest 8593343465227540543  # install an older build & pin it
aurelia pin 1245620                  # lock the current install (block Aurelia updates)
aurelia unpin 1245620                # release the pin
# Older manifest ids aren't exposed by Steam — find them on SteamDB:
#   https://steamdb.info/depot/<depot_id>/manifests/

# DLC
aurelia enable 2001                  # enable an installed DLC for its base game
aurelia disable 2001                 # disable a DLC

# Branches & depots
aurelia branches 1245620             # list beta branches
aurelia set-branch 1245620 beta      # switch branch
aurelia depots 1245620               # list depots
aurelia launch-options 1245620       # list Steam launch configs (exe/args/platform)

# Launch
aurelia play 1245620                 # launch a game and wait for it to exit
aurelia play 1245620 --proton experimental   # Linux: force a specific Proton/Wine runner
aurelia play 1245620 --steam         # run with Steam online features (Family Sharing / DRM)
aurelia running                      # list games Aurelia is currently running
aurelia stop 1245620                 # stop a running game (--force to kill a hung one)

# Steam Cloud
aurelia cloud sync 1245620           # sync saves (down then up)
aurelia cloud list 1245620           # list a game's Cloud files

# Steam Workshop
aurelia workshop browse 1245620            # discover items (search / sort / paginate)
aurelia workshop info 1234567890           # item or collection metadata
aurelia workshop install 1234567890        # download an item (collections expand to members)
aurelia workshop subscribe 1234567890 --install  # subscribe, then download
aurelia workshop status 1245620            # installed vs subscribed (+ update detection)
aurelia workshop rate 1234567890 up        # thumbs-up (or: down) an item
aurelia workshop comments 1234567890       # read an item's comments
aurelia workshop comment 1234567890 "Nice mod!"  # post a comment

# Friends & chat
aurelia friends                              # list friends (name, status, current game)
aurelia friends search gabelogannewell       # resolve a SteamID (id / profile URL / vanity)
aurelia friends add 76561197960287930        # send a friend request (accepts a URL too)
aurelia friends remove 76561197960287930     # remove a friend / cancel a request
aurelia chat send 76561198042323314 "hi!"    # send a direct message to a friend
aurelia chat history 76561198042323314       # show recent messages with a friend
aurelia chat open 76561198042323314          # interactive live chat (type to send; Ctrl-D quits)

# Inventory & market
aurelia inventory 753 --context 6            # your Steam cards / gems / backgrounds
aurelia market price 440 "Mann Co. Supply Crate Key"   # item price (no login needed)
aurelia market search "Sticker" --app-id 730 # search the Community Market
aurelia market listings                      # your active listings & buy orders
aurelia wallet                               # Steam Wallet balance

# Configuration
aurelia config show                  # print launcher configuration
aurelia config protons               # list detected Proton/Wine runtimes
aurelia config presence online       # appear online for chat (default: offline/invisible)
aurelia config language german       # default language for info/achievements text
aurelia config game 1245620 --proton GE-Proton9-20  # pin a Proton version for one game

# Proton / Wine runtimes (download manager)
aurelia proton list                  # installable runtimes (Valve + GE + CachyOS) and what's installed
aurelia proton install GE-Proton9-20 # download a GE build (or "Proton 9.0" via Steam)
aurelia proton install Proton-CachyOS # CachyOS build (auto-picks x86_64_v3/AVX2 when supported)
aurelia proton default GE-Proton9-20 # set the global default (used when a game has none set)
aurelia proton uninstall GE-Proton9-19  # delete an installed GE build

# Windows Steam runtime (self-contained Steamworks/DRM handshake, no host Steam client)
aurelia steam-runtime status          # resolved master prefix, layout, steam.exe presence
aurelia steam-runtime install         # install Steam into the master Wine prefix
aurelia steam-runtime repair          # back up the prefix (keep one) and reinstall

# Collections (library categories) — edit locally offline, sync to Steam on demand
aurelia collections list                     # all collections + game counts
aurelia collections create "RPGs"            # new (static) collection
aurelia collections add "RPGs" 570 730       # add games by app id
aurelia collections remove "RPGs" 730        # drop a game
aurelia collections show "RPGs"              # list a collection's games
aurelia list --collection "RPGs"             # filter the library to one collection
aurelia collections pull                     # fetch Steam's collections and merge them in
aurelia collections push --yes               # upload local collections to your Steam account
aurelia collections sync --yes               # pull then push (reconcile both sides)

# umu-launcher plugin (Linux only, optional — Proton via umu-run, downloaded on demand)
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
aurelia play 2270                     # runs through the script (e.g. gamemoderun/mangohud/gamescope)
aurelia config game 2270 --launch-script ~/my/wrap.sh   # pin a specific script per game
aurelia play 2270 --script ~/other.sh # one-off override for a single launch
aurelia play 2270 --no-script         # bypass all scripts for this launch
aurelia scripts remove 2270           # delete the dir-based script
```

> [!NOTE]
> **Luxtorpeda** is an optional plugin that runs supported games on native Linux engines
> (GZDoom, OpenMW, …) instead of Proton/Wine. It is **never bundled** — Aurelia downloads it
> on the fly into `~/.config/Aurelia/plugins/luxtorpeda` only when you enable the feature and
> opt a game in, so the binary stays lean. Linux only. Games run outside Steam's runtime
> container; if an engine can't find system libraries, prefer Proton for that title.

<!-- -->

> [!NOTE]
> **umu-launcher** is an optional plugin that runs Windows games through Proton **outside**
> Steam (applying the Steam Linux Runtime and per-game protonfixes), wrapping the launch with
> `umu-run` instead of replacing the runtime. Like luxtorpeda it is **never bundled** —
> Aurelia downloads it on the fly into `~/.config/Aurelia/plugins/umu` only when you enable the
> feature and opt a game in, so the binary stays lean. Linux only. It **wraps Proton** rather
> than replacing it, so `--umu` combines with `--proton` to pick the Proton build it runs.

<!-- -->

> [!NOTE]
> **Per-game launch scripts** let you wrap the fully-resolved launch command with your own
> shell script (`<script_dir>/<appid>.sh`, or `.bat` on Windows). Aurelia runs the script with
> the resolved command as its arguments (`"$@"`) and exports `AURELIA_*` env vars, so a script
> that is just `exec "$@"` is a passthrough while a custom one can prepend `gamemoderun` /
> `mangohud` / `gamescope`. It works uniformly for native, Proton, luxtorpeda and umu launches.
> Resolution precedence: `play --script <path>` > `config game --launch-script <path>` >
> the auto-detected `<script_dir>/<appid>.sh`. `play --no-script` bypasses all of them.

Add `--json` to any command for machine-readable output (errors included). It's a global
flag, so `aurelia --json <command>` and `aurelia <command> --json` are equivalent.

📖 **See [USAGE.md](USAGE.md) for complete documentation of every command and option.**

---

## Configuration

Aurelia stores its configuration and local data under `~/.config/Aurelia`
(`%USERPROFILE%\.config\Aurelia` on Windows). Set **`AURELIA_CONFIG_DIR`** to relocate it —
useful for an embedding driver (e.g. Heroic) that needs Aurelia's state isolated from a
standalone install.

- **Library path** — Aurelia auto-detects your existing Steam installation. Inspect the
  resolved configuration with `aurelia config show`.
- **Session** — refresh tokens are persisted in `session.json` so subsequent invocations
  log in automatically.
- **Unified download pipeline** — installs, updates, and verifications all run through a
  single, robust engine for reliability and speed.

---

## Documentation

| Document | Contents |
|---|---|
| [USAGE.md](USAGE.md) | Full reference for every command and flag |
| [docs/community-market-plan.md](docs/community-market-plan.md) | Design & roadmap for Steam Community Market support |
| [CONTRIBUTING.md](CONTRIBUTING.md) | How to contribute |

---

## Contributing

Contributions are welcome — see [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines. By
participating, you agree to abide by our [Code of Conduct](CODE_OF_CONDUCT.md).

## Acknowledgments

Aurelia grew directly out of **[SteamFlow](https://github.com/weter11/SteamFlow)** — the
earlier project it is derived from and the foundation this work is built on. Our deepest
thanks to its author: SteamFlow did the hard groundwork that made Aurelia possible.

It stands, in turn, on **[steam-vent](https://codeberg.org/steam-vent/steam-vent)** and
**[steam-vent-chat](https://codeberg.org/steam-vent/chat)**, whose reverse-engineering and
protocol work let Aurelia speak Steam's real network protocols — and on a vendored, modified
`steam-cdn` (plus the `zip` crate) for the content pipeline.

### Credits

- [SteamFlow](https://github.com/weter11/SteamFlow) — the project Aurelia is derived from; its groundwork is the base everything here is built on. Thank you!
- [steam-vent](https://codeberg.org/steam-vent/steam-vent) — Steam network protocol implementation
- [steam-vent-chat](https://codeberg.org/steam-vent/chat) — Steam Chat protocol implementation
- [steam-cdn](https://crates.io/crates/steam-cdn) — content-delivery / depot download engine (vendored & modified)
- [SteamKit2](https://github.com/SteamRE/SteamKit) — Steam .NET research code
- [SteamHelper-rs](https://github.com/saskenuba/SteamHelper-rs) — SteamKit Rust port

---

## License

Aurelia is released under the [GPL-3.0 License](LICENSE).
