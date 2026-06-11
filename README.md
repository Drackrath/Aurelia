<img src="assets/aurelia_logo.png" alt="Aurelia logo" title="Aurelia" align="left" height="80" />

# Aurelia

**A fast, lightweight, command-line Steam launcher and library manager written in Rust.**

[![License: GPL-3.0](https://img.shields.io/badge/License-GPL%203.0-blue.svg)](LICENSE)
[![Built with Rust](https://img.shields.io/badge/built%20with-Rust-orange.svg)](https://www.rust-lang.org/)
[![Status: Alpha](https://img.shields.io/badge/status-active%20alpha-yellow.svg)](#project-status)

<br clear="left" />

Aurelia is a pure command-line Steam launcher and library manager — no CEF, no WebViews,
no GUI. It talks to Steam's real network protocols through
[`steam-vent`](https://github.com/n00b67/steam-vent), so you can log in, manage your
library, install and update games, sync Steam Cloud saves, and launch titles (natively or
through Proton/Wine) entirely from a terminal or a script.

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
  tickets, depot browsing, and DLC management — built on open, documented protocols.
- **Open source.** GPL-3.0 licensed, with no dependency on opaque 32-bit legacy Steam binaries.

### How it compares

| Feature | Official Steam | OpenSteamClient | **Aurelia** |
|---|---|---|---|
| **Architecture** | Electron + C++ | C++ / Qt | Pure Rust |
| **Idle RAM** | ~400–800 MB | ~100–200 MB | < 50 MB |
| **Interface** | Desktop GUI | Desktop GUI | CLI (scriptable) |
| **Download engine** | CDN + P2P LAN | Standard CDN | Multi-threaded CDN |
| **Authentication** | Full | Core | Full (tokens, mobile app, Guard) |
| **Steam integration** | Native | Partial | Deep (PICS, CDN, Cloud, tickets) |
| **Platforms** | Windows, Linux, macOS | Windows, Linux | Linux (first), Windows |
| **Open source** | No | Yes | Yes (GPL-3.0) |

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
      folders (with Steam's `appmanifest`/`libraryfolders.vdf` kept in sync)
- [x] **DLC** — install, enable/disable, and per-DLC ownership/install status
- [x] **Steam Cloud** — enumerate, download, upload save data
- [x] **Proton/Wine** — runtime discovery and launch integration
- [x] **Depot browser** — list depots, inspect manifest trees, download single files
- [ ] Collections / categorization
- [ ] Friends list & chat
- [ ] Workshop management

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
aurelia dlc 690830                   # list a game's DLC with ownership/install status
aurelia image 1245620                # fetch cover art to the cache (prints the path)
aurelia image 1245620 -o cover.jpg   # save artwork to a specific file

# Install & maintain
aurelia install 1245620              # download & install a game by app id
aurelia update 1245620               # download the latest manifest
aurelia verify 1245620               # verify installed files
aurelia uninstall 1245620            # remove a game (--delete-prefix wipes its prefix)
aurelia move 1245620 D:\SteamLibrary # move an install to another library (updates Steam data)

# DLC
aurelia enable 2001                  # enable an installed DLC for its base game
aurelia disable 2001                 # disable a DLC

# Branches & depots
aurelia branches 1245620             # list beta branches
aurelia set-branch 1245620 beta      # switch branch
aurelia depots 1245620               # list depots

# Launch
aurelia play 1245620                 # launch a game and wait for it to exit
aurelia play 1245620 --windows       # run the Windows .exe directly (default on Windows)
aurelia play 1245620 --proton experimental   # force a specific Proton/Wine runner

# Configuration
aurelia config show                  # print launcher configuration
aurelia config protons               # list detected Proton/Wine runtimes
```

Add `--json` to any command for machine-readable output (errors included). It's a global
flag, so `aurelia --json <command>` and `aurelia <command> --json` are equivalent.

📖 **See [USAGE.md](USAGE.md) for complete documentation of every command and option.**

---

## Configuration

Aurelia stores its configuration and local data under `~/.config/Aurelia`
(`%USERPROFILE%\.config\Aurelia` on Windows).

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
| [CONTRIBUTING.md](CONTRIBUTING.md) | How to contribute |

---

## Contributing

Contributions are welcome — see [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

## Acknowledgments

Special thanks to the developers of **OpenSteamClient** and **steam-vent** for their
invaluable reverse-engineering and protocol documentation. Aurelia is powered by a
vendored, modified `steam-cdn` and the `zip` crate.

### Credits

- [steam-vent](https://codeberg.org/steam-vent/steam-vent) — Steam protocol implementation

---

## License

Aurelia is released under the [GPL-3.0 License](LICENSE).
