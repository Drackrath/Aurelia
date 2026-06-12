# Aurelia CLI Usage

Aurelia is a command-line Steam launcher. It authenticates against Steam, manages
your library, downloads/verifies games, and launches them — all from the terminal.

```text
aurelia <COMMAND> [OPTIONS]
```

Run `aurelia --help` for a summary, or `aurelia <COMMAND> --help` for the options
of a specific command. `--version` prints the build version.

## Contents

- [Global behavior](#global-behavior)
- [Authentication](#authentication)
  - [`login`](#login)
  - [`logout`](#logout)
- [Library](#library)
  - [`list`](#list)
  - [`account`](#account)
  - [`info`](#info)
  - [`dlc`](#dlc)
  - [`achievements`](#achievements)
  - [`image`](#image)
- [Installation & maintenance](#installation--maintenance)
  - [`install`](#install)
  - [`update`](#update)
  - [`verify`](#verify)
  - [`uninstall`](#uninstall)
  - [`move`](#move)
  - [`relink`](#relink)
  - [`import`](#import)
  - [`available`](#available)
  - [`enable` / `disable`](#enable--disable)
- [Launching](#launching)
  - [`play`](#play)
- [Depots & branches](#depots--branches)
  - [`branches`](#branches)
  - [`set-branch`](#set-branch)
  - [`depots`](#depots)
  - [`launch-options`](#launch-options)
- [Steam Cloud](#steam-cloud)
  - [`cloud sync`](#cloud-sync)
  - [`cloud list`](#cloud-list)
- [Configuration](#configuration)
  - [`config show`](#config-show)
  - [`config protons`](#config-protons)
- [Session daemon](#session-daemon)
  - [`daemon`](#daemon)
  - [`daemon list` / `daemon stop`](#daemon)
  - [`kill`](#kill)
- [Files & locations](#files--locations)
- [Exit codes & logging](#exit-codes--logging)

---

## Global behavior

- **`--json`:** A global flag accepted by **every** command (before or after the command,
  e.g. `aurelia --json list` or `aurelia list --json`). Output is emitted as JSON on
  stdout, and any error is printed as `{ "error": "..." }` with a non-zero exit code.
  Diagnostics and progress are written to stderr, so stdout stays clean for piping into
  tools like `jq`.
- **`-v, --verbose`:** A global, repeatable flag that increases log verbosity. Logs are
  written to stderr. At the default level Aurelia prints high-level progress (connecting,
  fetching owned games, …) while the chatty Steam networking stack is quieted; `-v`, `-vv`
  and `-vvv` progressively unmute it. This is the way to diagnose a command that appears to
  **hang**: the last line printed shows exactly which step is stuck (typically a Steam CM
  connection or RPC). `RUST_LOG`/`AURELIA_LOG` (standard `tracing` env-filter syntax, e.g.
  `RUST_LOG=steam_vent=trace`) override the flag entirely. See
  [docs/logging.md](docs/logging.md).
- **Session:** After `login`, a refresh token is stored so subsequent commands reuse the
  session automatically. Commands that need Steam (`account`, `install`, `play`, …) will
  error with `not logged in — run \`aurelia login\` first` if no valid session exists.
- **Library discovery:** Installed games are detected across **all** Steam libraries,
  including secondary libraries on other drives (e.g. `F:\SteamLibrary`) even if they are
  not listed in `libraryfolders.vdf`.
- **Logging:** Set `RUST_LOG` to control tracing verbosity, e.g. `RUST_LOG=debug` (to
  stderr).

`<APP_ID>` is the numeric Steam application id (visible via `aurelia list`).

---

## Authentication

### `login`

Authenticate with Steam and persist the session.

```text
aurelia login [-u <USERNAME>] [-p <PASSWORD>] [-g <GUARD_CODE>] [--code] [--qr]
aurelia login --health      # report session status (no login)
aurelia login --reconnect   # rebuild the daemon's shared session
```

| Option | Description |
| --- | --- |
| `-u, --username <USERNAME>` | Steam account name. Prompted if omitted. |
| `-p, --password <PASSWORD>` | Account password. Prompted securely if omitted. |
| `-g, --guard <GUARD_CODE>` | Steam Guard code (email or mobile authenticator), supplied up front. |
| `--code` (alias `--pin`) | Enter the Steam Guard code **interactively** when prompted, instead of approving in the Steam Mobile app. Conflicts with `-g`. |
| `--qr` | Log in by **scanning a QR code** with the Steam Mobile app — no username/password needed. Conflicts with the credential options. |
| `--health` | Report current session status **without logging in** (see below). Conflicts with all login options. |
| `--reconnect` | Rebuild the [daemon's](#daemon) shared session from the stored token. Conflicts with all login options. |

There are three ways to authenticate:

1. **Password + Steam Guard.** Provide `-u`/`-p` (or be prompted). Then, depending on your
   account: pass `-g <CODE>` up front, use `--code`/`--pin` to type the code when prompted,
   or (the default) approve the login in your Steam Mobile app.
2. **`--code` / `--pin`.** Forces interactive Steam Guard **code** entry: after you submit
   credentials, Steam asks for the code (email or authenticator) and you type it in.
3. **`--qr`.** Renders a QR code in the terminal (with a `https://s.team/…` link as a
   fallback). Scan it with the Steam Mobile app to approve; no password is entered.

A single log line — shown even without `-v` — reports which method is being awaited, e.g.
`Login method awaited: QR code — scan it with the Steam Mobile app` or
`Login method awaited: Steam Guard code`. The password may also be supplied via the
`AURELIA_PASSWORD` environment variable.

```bash
# Interactive password login (recommended)
aurelia login

# Type the Steam Guard code interactively instead of approving in the app
aurelia login --code            # or: aurelia login --pin

# Scan a QR code with the Steam Mobile app (no password)
aurelia login --qr

# Fully non-interactive
aurelia login -u myname -p 'secret' -g ABCDE
AURELIA_PASSWORD='secret' aurelia login -u myname
```

#### Session health & reconnect

These two flags inspect or refresh the **session** rather than logging in, and are aimed at a
front-end that drives the [`daemon`](#daemon):

- **`aurelia login --health`** reports whether a session is currently authenticated, without
  performing a login. When a daemon is in use it reads the daemon's **shared session state**
  (no new logon); standalone it does a one-off live restore check. Output (`--json`):
  `{ "logged_in", "account", "steam_id", "daemon" }` — `daemon` indicating whether the answer
  came from the shared daemon session. A poller can use this to decide whether `login` is
  needed.
- **`aurelia login --reconnect`** tears down the daemon's shared session and re-establishes it
  from the stored token — use it if the live connection dropped (e.g. after a network blip)
  and commands start failing with auth errors. It returns the same status object as
  `--health`. Without a running daemon it errors (there is no shared session to rebuild; start
  `aurelia daemon` first, or just re-run the failing command standalone).

```bash
aurelia login --health             # human-readable status
aurelia login --health --json      # {"logged_in":true,"account":"me","steam_id":...,"daemon":true}
aurelia login --reconnect --json   # rebuild the shared session, then report status
```

#### Non-interactive `--json` login (for tooling)

With `--json`, `login` becomes a machine-drivable handshake with **no TTY prompts** — a
driver (e.g. a GUI front-end) supplies credentials via flags/`AURELIA_PASSWORD` and exchanges
NDJSON lines on stdout/stdin:

- **Password:** `aurelia login --json -u <user> -p <pass>`. The first line emitted is
  always `{"event":"awaiting_confirmation","message":"…"}` — sent **before** the login
  attempt blocks, so the driver can immediately tell the user to approve the sign-in on
  their device (otherwise nothing prints until the attempt completes or times out). Then,
  if Steam needs a Guard code, a `{"event":"guard_required","type":"email"|"device"}` line
  follows; write the code as a single line to the process's **stdin** and login retries.
  Accounts that use mobile-app approval instead emit
  `{"event":"guard_required","type":"device_confirmation"}`.
- **QR:** `aurelia login --qr --json` streams `{"event":"qr_challenge","url":"https://s.team/…"}`
  (re-emitted whenever Steam rotates the code); render the URL as a QR and wait.
- **Result:** both end with `{"logged_in":true,"account":"<name>"}` on success, or
  `{"error":"…"}` (non-zero exit) on failure.

The complete NDJSON event sequence a driver may observe, in order:

| Event line | When | Driver action |
| --- | --- | --- |
| `{"event":"awaiting_confirmation","message":"…"}` | Immediately, on password login, before the attempt blocks. | Show the message; prompt the user to approve on their device if asked. |
| `{"event":"qr_challenge","url":"…"}` | QR login; re-emitted on each code rotation. | Render `url` as a QR code and wait. |
| `{"event":"guard_required","type":"email"\|"device"}` | A typed Steam Guard code is needed. | Read a code from the user, write it as one line to the child's **stdin**. |
| `{"event":"guard_required","type":"device_confirmation"}` | The account approves via the Steam Mobile app. | Tell the user to approve in the app; the command then completes or times out. |
| `{"logged_in":true,"account":"<name>"}` | Terminal — success. | Done; the session is persisted. |
| `{"error":"…"}` | Terminal — failure (non-zero exit). | Surface the error. |

In `--json` mode the username/password must be provided up front (no interactive prompt);
only the Guard code is exchanged over stdin.

### `logout`

Clear the stored session.

```bash
aurelia logout
```

---

## Library

### `list`

List games in your library (owned games merged with locally installed ones).

```text
aurelia list [-i] [-s <TEXT>] [--online] [--json]
```

| Option | Description |
| --- | --- |
| `-i, --installed` | Only show installed games. |
| `-s, --search <TEXT>` | Filter by case-insensitive substring of the name. |
| `--online` | Add an `ONLINE` column indicating whether each game appears to require an internet connection (see below). |
| `--json` | Emit JSON instead of a table. |

The `STATUS` column shows `installed`, `update` (installed with an update available), or
`-` (not installed). A non-default branch is shown in brackets after the name.

Steam **tooling** — Proton, the Steam Linux Runtimes, and Steamworks Common
Redistributables — is filtered out, so the list shows only real games rather than the
runtime/redistributable app ids that share the library.

With **`--online`**, an extra `ONLINE` column reports whether the game looks like it
**requires** a connection to play: `yes`, `no`, or `?` (undetermined). Steam exposes no
explicit flag for this, so it is inferred from the game's PICS store categories — a title
is treated as online-required when it advertises an online-multiplayer category (MMO,
Online PvP, Online Co-op) but **no** single-player support. This makes one PICS lookup per
listed game, so it is slower than a plain listing and needs an active session; without one
the column reads `?`. The `--json` output carries an `online_required` boolean (or `null`).

The `LICENSE` column shows whether the logged-in account holds a license for the game:

| Value | Meaning |
| --- | --- |
| `owned` | The account has a license (the game is in its owned-games list). |
| `family-shared` | Installed locally but licensed to a **different** account — borrowed via Steam Family Sharing. |
| `unlicensed` | Installed under this account with no license record (e.g. redistributables, soundtrack/DLC, or a delisted free app). |

The list includes Family-Shared games **even when they are not installed** — the full
shared library offered by your Steam Family is queried and merged in (these show
`STATUS` `-`). Family-Sharing is determined two ways, both requiring an active session:
the Steam Families shared-library list, and, for installed games, comparing the
`LastOwner` in the `appmanifest` against your SteamID. The `--json` output includes
`is_owned` and `is_family_shared` booleans per game.

```bash
aurelia list
aurelia list --installed
aurelia list --search elden
aurelia list --json > library.json
```

**Without an Aurelia session** (you haven't run `aurelia login`, or you're offline),
`list` falls back to the locally signed-in Steam client's own caches and still shows your
full library — every game is reported as `owned` with `STATUS` `-` unless installed. This
requires only that the Steam client itself is signed in; no network access is used. Running
`aurelia login` re-enables the strictly richer network path (live ownership, update status,
and not-installed Family-Shared titles). See
[docs/linux-library-discovery.md](docs/linux-library-discovery.md) for details.

### `account`

Show account details for the logged-in user. Requires an active session.

```text
aurelia account [--json]
```

```bash
aurelia account
aurelia account --json
```

Shows account name, SteamID, country, email (and validation state), authorized device
count, and VAC ban count.

### `info`

Show detailed information about one or more games. The metadata is fetched over Steam's
CM connection (the `StoreBrowse` service), not the HTTPS storefront. A session is required
**only on a cache miss** — see [Caching](#caching) below; a cached lookup works offline.

```text
aurelia info <APP_ID>... [--extended] [--no-cache] [--json]
```

| Option | Description |
| --- | --- |
| `<APP_ID>...` | One or more app ids. Multiple ids are fetched together (see below). |
| `--extended` | Also show storefront-only fields (see below). Makes additional HTTPS storefront requests. |
| `--no-cache` | Bypass the local metadata cache and fetch fresh data from Steam. |
| `--json` | Emit JSON instead of formatted text. |

By default `info` shows what the `StoreBrowse` protocol provides directly: type,
developers, publishers, franchises, release date (and Early-Access/coming-soon state),
price and discount, platforms, the Steam **review summary**, the short description,
**artwork URLs** (header/capsule/hero/background/logo), and the list of **DLC** with names
resolved. The DLC ids come from PICS appinfo and their names from a single batched
`StoreBrowse` lookup — all over the CM connection, with no per-DLC web calls.

The `--json` output includes an `assets` object with `header`, `capsule` (portrait cover),
`hero`, `background` and `logo` URLs, derived from the StoreBrowse asset block (falling back
to Steam's conventional CDN paths) so a front-end doesn't have to guess them.

A handful of fields have **no CM-protocol source**, so they are shown only with
**`--extended`**, which fetches them from the public HTTPS storefront (Steam storefront API
plus SteamSpy):

- **System requirements** — minimum and recommended.
- **Metacritic** score and **website**.
- Store **genres** and **categories** (resolved to names).
- Community **user tags** (from SteamSpy).

#### Multiple app ids (one logon per batch)

`info` accepts several app ids at once and resolves them over a **single** Steam logon
with **one batched `StoreBrowse` call**, so `aurelia info <id1> <id2> <id3>` costs one
connection — far cheaper than running `info` once per id. (DLC-name lookups still happen
per game.) An id with no store data is skipped with a warning rather than failing the whole
command; a single unknown id still errors.

#### Caching

To avoid a Steam logon on every call — Steam throttles repeated logons, and front-ends
like Heroic poll `info` often — the CM-sourced metadata (the `StoreBrowse` fields plus the
DLC list) is cached to disk per app under `info_cache/<APP_ID>.json` in the config
directory. A cache **hit** serves the result with **no network access at all** (no logon,
no `StoreBrowse`/PICS round-trip), so it also works offline.

- The cache **time-to-live defaults to 6 hours**. Override it (in seconds) with the
  `AURELIA_INFO_CACHE_TTL` environment variable; set it to `0` to disable the cache.
- Pass `--no-cache` to ignore any cached copy and refresh from Steam (the fresh result is
  then written back to the cache).
- `--extended` storefront/SteamSpy fields are **not** cached — they are always fetched live
  when `--extended` is given, though a cache hit still spares the CM logon for the base data.

#### JSON output shape

With `--json`, the extended fields (when requested) are grouped under an `"extended"` key so
the default object shape is unchanged. **One** id produces a single JSON **object** (as
before); **several** ids produce a JSON **array** of those objects, in the order requested.

```bash
aurelia info 690830                      # protocol-native fields
aurelia info 690830 --extended           # + requirements, Metacritic, tags, genres, categories
aurelia info 690830 --json               # single object
aurelia info 690830 570 730 --json       # array of three objects, one logon
aurelia info 690830 --no-cache           # force a fresh fetch
AURELIA_INFO_CACHE_TTL=0 aurelia info 690830   # bypass the cache for this run
```

### `dlc`

List a game's DLC together with its ownership and install state. Requires login
(ownership is checked against your account).

```text
aurelia dlc <APP_ID> [--json]
```

| Option | Description |
| --- | --- |
| `--json` | Emit JSON instead of formatted text. |

A focused alternative to `info` when you only want the DLC list. The DLC ids come from
PICS appinfo and their names from a single batched `StoreBrowse` lookup (both over the CM
connection — no storefront API); each entry is then annotated with:

- **owned** — your account holds a license for the DLC (an app ownership ticket is
  issued).
- **installed** — the DLC's content is present on disk (its depots are recorded in
  the base game's appmanifest).
- **disabled** — the DLC is listed in the base game's `DisabledDLC`, so Steam treats
  it as turned off.

In the text view the `STATUS` column collapses installed/disabled into
`not-installed`, `disabled`, or `enabled`. The base game must be installed for the
install/enable state to be meaningful; otherwise every DLC reads as `not-installed`.

```bash
aurelia dlc 690830
aurelia dlc 690830 --json
```

### `achievements`

Show the logged-in user's achievements for a game, with per-achievement unlock state.
Requires an active session.

```text
aurelia achievements <APP_ID> [-l <LANG>] [--json]
```

| Option | Description |
| --- | --- |
| `-l, --lang <LANG>` | Language for names/descriptions (Steam API language name, e.g. `english`, `german`). Default `english`. |
| `--json` | Emit JSON instead of a table. |

Combines the game's achievement **definitions** and **global rarity** (`Player.GetGameAchievements`)
with your **unlock state and time** (`ClientGetUserStats`) — all over the Steam CM connection.
The text view marks unlocked achievements (`✓`), the global unlock rate, and the unlock date;
hidden-and-still-locked ones are tagged `(hidden)`. The `--json` output is
`{ "app_id", "unlocked", "total", "achievements": [ { achievement_id, achievement_key, name,
description, visible, image_url_unlocked, image_url_locked, rarity, unlocked, unlock_time,
date_unlocked } ] }` (rarity is the global unlock percentage; `date_unlocked`/`unlock_time` are
`null` when locked). A game you've never launched still lists every achievement, all locked.

```bash
aurelia achievements 620
aurelia achievements 620 --lang german
aurelia achievements 620 --json
```

### `image`

Download a game's cover/header artwork from the Steam CDN to the local image cache.

```text
aurelia image <APP_ID> [-o <PATH>] [-f]
```

| Option | Description |
| --- | --- |
| `-o, --output <PATH>` | Write the image to this path instead of the cache directory. |
| `-f, --force` | Re-download even if a cached copy already exists. |

The command prints the final path of the image. It tries the library capsule, then the
header image, then the legacy capsule. No login is required (artwork is public).

```bash
aurelia image 1245620                 # cache it, print the cached path
aurelia image 1245620 -o cover.jpg    # save to a specific file
aurelia image 1245620 --force         # refresh the cached copy
```

---

## Installation & maintenance

These commands require an active session.

### `install`

Download and install a game.

```text
aurelia install <APP_ID> [-p <windows|linux>] [--dry-run]
```

| Option | Description |
| --- | --- |
| `-p, --platform <windows\|linux>` | Depot platform to install. Auto-detected if omitted. |
| `--dry-run` | Don't install — just report the estimated download and on-disk size. |

If `--platform` is omitted, the available platforms are detected and the first one is
chosen (printed as `Auto-selected platform: ...`). Progress is streamed to the terminal;
the command exits non-zero if the download fails.

With `--dry-run`, nothing is downloaded; Aurelia prints the estimated **download size**
(compressed transfer) and **disk size** (installed footprint) for the target platform,
derived from PICS depot metadata. With `--json` it emits
`{ "app_id", "platform", "download_size", "disk_size", "depot_count" }` (sizes in bytes) —
useful for an install dialog. The estimate covers the base game (DLC depots are excluded).

```bash
aurelia install 1245620 --dry-run
aurelia install 1245620 --dry-run --json
```

**DLC:** If the app id is a DLC, its content is installed into the **base game's**
directory and registered in the base game's `appmanifest` — its depots are added with the
`dlcappid` tag and the DLC is removed from the manifest's `DisabledDLC` list, so the game
recognises it as installed and **enabled**. The base game must already be installed.

```bash
aurelia install 1245620
aurelia install 1245620 --platform windows
```

### `update`

Download the latest manifest for an installed game (apply a pending update).

```bash
aurelia update 1245620
```

### `verify`

Verify the integrity of an installed game's files, re-downloading any that are missing or
corrupt. Progress is streamed.

```bash
aurelia verify 1245620
```

### `uninstall`

Uninstall a game.

```text
aurelia uninstall <APP_ID> [--delete-prefix]
```

| Option | Description |
| --- | --- |
| `--delete-prefix` | Also delete the game's Wine prefix / compatibility data (Linux). |

```bash
aurelia uninstall 1245620
aurelia uninstall 1245620 --delete-prefix
```

### `move`

Move an installed game to a different Steam library folder, updating Steam's on-disk data
so the client recognises the game at its new path instead of reporting it as missing.

```text
aurelia move <APP_ID> <LIBRARY> [--restart-steam]
```

| Option | Description |
| --- | --- |
| `<LIBRARY>` | Destination Steam **library root** (the folder containing `steamapps/`), e.g. `D:\SteamLibrary`. Must already be a Steam library. |
| `--restart-steam` | Stop Steam for the duration of the move and restart it afterward. |

The move relocates three things and reconciles Steam's bookkeeping:

- the **game files** (`steamapps/common/<installdir>`),
- the **Proton/Wine prefix** (`steamapps/compatdata/<appid>`), if the game has one,
- the **`appmanifest_<appid>.acf`** (copied to the destination, removed from the source —
  Steam derives a game's library from where its manifest lives), and
- the **`apps` index in `libraryfolders.vdf`**, so the index isn't left pointing at the old
  location (best-effort; Steam reconciles it from the manifests on next launch if the file
  can't be edited cleanly).

Progress is streamed with a `MOVING` percentage. Moves within the same drive use an instant
`rename`; moves to another drive copy with byte-level progress. The **source is deleted only
after the copy fully succeeds**, so an interrupted cross-drive move never loses the original.

Steam rewrites these files on exit, so the move **refuses to run while Steam is open**
unless you pass `--restart-steam`, which makes Aurelia stop Steam, move, then start it
again. The destination must already be a registered Steam library (add a drive via Steam →
Settings → Storage first); Aurelia warns if it isn't.

```bash
aurelia move 1245620 D:\SteamLibrary
aurelia move 1245620 /mnt/games/SteamLibrary --restart-steam
```

### `relink`

Point Steam at an install that already lives in a different library — **without copying any
files**. Use this when you moved a game's folder yourself (Aurelia only updates Steam's
records); use [`move`](#move) when the files still need to be copied.

```text
aurelia relink <APP_ID> <LIBRARY> [--restart-steam]
```

| Option | Description |
| --- | --- |
| `<LIBRARY>` | Destination Steam library root. Its `steamapps/common/<installdir>` must already contain the game's files. |
| `--restart-steam` | Stop Steam for the operation and restart it afterward. |

It moves the `appmanifest_<appid>.acf` to the destination library and updates
`libraryfolders.vdf`; the game files and Proton prefix are left untouched. Fails if the files
aren't present at the destination. Like `move`, it refuses to run while Steam is open unless
`--restart-steam` is given.

```bash
aurelia relink 1245620 D:\SteamLibrary --restart-steam
```

### `import`

Register an existing on-disk install that Steam doesn't know about — Aurelia writes its
`appmanifest_<appid>.acf` (depot manifests and build id taken from PICS, so Steam sees it as
installed and up to date) and adds it to `libraryfolders.vdf`.

```text
aurelia import <APP_ID> <LIBRARY> [-p <windows|linux>] [--restart-steam]
```

| Option | Description |
| --- | --- |
| `<LIBRARY>` | Steam library root whose `steamapps/common/<installdir>` holds the files. |
| `-p, --platform <windows\|linux>` | Depot platform whose files are present. Defaults to the current OS. |
| `--restart-steam` | Stop Steam for the operation and restart it afterward. |

The install directory name comes from PICS, so Aurelia knows where to look under
`steamapps/common`. Fails if the files aren't there (use [`install`](#install) to download) or
if an appmanifest already exists (use [`relink`](#relink) to relocate it).

```bash
aurelia import 1245620 D:\SteamLibrary
aurelia import 1245620 ~/.steam/steam --platform linux --restart-steam
```

### `available`

Report whether a game is installed **and** its files are present on disk (mirrors what a
front-end needs to decide if a title can be launched).

```text
aurelia available <APP_ID> [--json]
```

Checks that an `appmanifest` exists and that the resolved install directory is present. The
`--json` output is `{ "app_id", "available", "install_path" }` (`install_path` is `null` when
nothing is registered).

This is a **local, offline** check: it reads only on-disk Steam files and **never logs on to
Steam** (no session required), so a front-end can call it freely per game without
contributing to Steam logon rate limits.

```bash
aurelia available 1245620
aurelia available 1245620 --json
```

### `enable` / `disable`

Enable or disable an owned DLC for its base game by toggling the DLC's entry in the base
game's `appmanifest` `DisabledDLC` lists. The `<APP_ID>` is the **DLC's** app id; its base
game is resolved automatically and must be installed.

```text
aurelia enable  <APP_ID> [--restart-steam]
aurelia disable <APP_ID> [--restart-steam]
```

| Option | Description |
| --- | --- |
| `--restart-steam` | Stop Steam before applying the change and start it again afterward (Windows). |

```bash
aurelia enable 2690330                  # flip the flag (apply on next Steam start)
aurelia disable 2690330 --restart-steam # apply now by cycling Steam
```

`enable` only flips the flag — run `aurelia install <APP_ID>` if the DLC's content isn't
downloaded yet.

> **How it applies:** `DisabledDLC` lives in the base game's `appmanifest`, which the desktop
> Steam client reads only at **startup** and overwrites from memory on **exit**. So an edit
> made while Steam is running is lost when Steam closes. `--restart-steam` does the reliable
> sequence — **stop Steam → edit → start Steam** — so the change takes effect immediately.
> Without it, restart Steam yourself for the change to apply. The command tells you when a
> restart is required.

---

## Launching

### `play`

Launch a game and wait for it to exit. Requires an active session. If Steam Cloud sync is
enabled, saves are synced down before launch and up afterward.

```text
aurelia play <APP_ID> [-p <PROTON>] [-w]
```

| Option | Description |
| --- | --- |
| `-p, --proton <PROTON>` | Force a specific Proton/Wine runner (Linux only). Implies a Windows target. |
| `-w, --windows` | Run the Windows executable directly with no Proton/Wine layer. |

Platform behavior:

- **On Windows**, games always run natively — there is no Proton/Wine layer — so plain
  `aurelia play <APP_ID>` works and `--windows` is implied.
- **On Linux**, native Linux builds run directly; Windows builds run through Proton/Wine.
  Use `--proton <ver>` to pin a specific runner (see `config protons` for available names),
  or `--windows` to run the `.exe` directly with no compatibility layer.

```bash
aurelia play 1245620                 # native on Windows / auto on Linux
aurelia play 1245620 --windows       # force native Windows execution
aurelia play 1245620 --proton experimental   # Linux: pin a Proton version
```

---

## Depots & branches

These commands require an active session.

### `branches`

List the available beta branches for a game.

```bash
aurelia branches 1245620
```

### `set-branch`

Switch a game to a different branch. Run `update` afterward to apply the change.

```text
aurelia set-branch <APP_ID> <BRANCH>
```

```bash
aurelia set-branch 1245620 beta
aurelia update 1245620
```

### `depots`

List the depots for a game (depot id, size in bytes, and name).

```bash
aurelia depots 1245620
```

### `launch-options`

List a game's launch options — the executables/arguments Steam can start it with, read from
the PICS `config/launch` table.

```text
aurelia launch-options <APP_ID> [--json]
```

Each entry has an `id` (`"0"` is the default), a `description`, an `executable` and
`arguments`, an optional `working_dir`, and platform constraints `oslist`
(`windows`/`linux`/`macos`, empty = any) and `osarch` (`32`/`64`). The `--json` output is
`{ "app_id", "launch_options": [ { id, description, executable, arguments, working_dir,
oslist, osarch, type } ] }`.

```bash
aurelia launch-options 1245620
aurelia launch-options 1245620 --json
```

---

## Steam Cloud

These commands require an active session.

### `cloud sync`

Synchronise a game's Steam Cloud saves with their real on-disk locations.

```text
aurelia cloud sync <APP_ID> [--up | --down] [--path <DIR>] [--json]
```

| Option | Description |
| --- | --- |
| `--up` | Only upload local saves to Steam. Conflicts with `--down`. |
| `--down` | Only download saves from Steam. Conflicts with `--up`. |
| `--path <DIR>` | Override the base directory for **classic** (token-less) remote-storage files. Defaults to `<userdata>/<account>/<appid>/remote`. Does **not** affect Auto-Cloud files (see below). |
| `--json` | Emit a JSON result instead of text. |

With **neither** flag it performs a full sync — **down then up** — matching what `play` does
around a launch. `--down` or `--up` restrict it to one direction. The `--json` result is
`{ "app_id", "direction": "both"|"down"|"up", "remote_root", "downloaded", "uploaded" }`.

**Path mapping (important).** Steam Auto-Cloud filenames embed the real save location as a
leading root token, e.g. `%WinAppDataLocalLow%SadSocket/9Kings/save.json`. Aurelia resolves
that token to the actual OS directory the game reads and writes — it does **not** copy saves
into a folder under `userdata`. Supported tokens:

- **Windows:** `%WinMyDocuments%`, `%WinAppDataLocal%`, `%WinAppDataLocalLow%`,
  `%WinAppDataRoaming%`, `%WinSavedGames%`, `%GameInstall%`.
- **Linux:** `%LinuxHome%`, `%LinuxXdgDataHome%`, `%LinuxXdgConfigHome%`, `%GameInstall%`.
  (`%Win*%` tokens belong to a game's Proton prefix, which this layer doesn't track yet, so
  they're skipped on Linux.)

Token-less filenames are classic `ISteamRemoteStorage` files and live under the `--path`
directory (default `<appid>/remote`). `%GameInstall%` resolves against the game's install
directory when it is installed.

**Direction logic.**

- **down** — a cloud file is written to its mapped local path when it is newer than (or
  missing) the local copy; the file is then stamped with the cloud's modification time so a
  later sync doesn't see it as locally changed.
- **up** — a save is uploaded when it is newer than, or differs in size from, its cloud copy.
  The candidate set is the union of (a) files already in the cloud and (b) local files matched
  by the app's UFS `savefiles` rules (read from appinfo), so a **brand-new** save that has
  never been in the cloud still gets its first upload.

```bash
aurelia cloud sync 1245620                 # download then upload
aurelia cloud sync 1245620 --down          # pull saves from Steam only
aurelia cloud sync 1245620 --up            # push local saves to Steam
aurelia cloud sync 1245620 --json
```

> Not yet handled: per-OS `ufs/rootoverrides` remapping, and `%Win*%` tokens on Linux/Proton.

### `cloud list`

List a game's Steam Cloud files with size and last-modified time.

```text
aurelia cloud list <APP_ID> [--json]
```

The `--json` output is `{ "app_id", "files": [{ "filename", "size", "timestamp", "sha_hash" }] }`
(`size` in bytes, `timestamp` a Unix time).

```bash
aurelia cloud list 1245620
aurelia cloud list 1245620 --json
```

---

## Configuration

### `config show`

Print the current launcher configuration as JSON (library path, default Proton version,
cloud-sync setting, per-game overrides, …).

```bash
aurelia config show
```

### `config protons`

List detected Proton/Wine runtimes — both Steam-managed runtimes and custom ones under
`compatibilitytools.d`.

```bash
aurelia config protons
```

---

## Session daemon

### `daemon`

Run a background process that logs in to Steam **once** and serves every other `aurelia`
command over a local socket — so a whole session's worth of commands shares **one** Steam
connection instead of re-authenticating on each call.

```text
aurelia daemon [--socket <PATH>]
aurelia daemon list [--json]
aurelia daemon stop [PID] [--json]
```

| Option | Description |
| --- | --- |
| `--socket <PATH>` | Override the socket/pipe path (also settable via `AURELIA_DAEMON_SOCKET`). |

`aurelia daemon list` shows running daemons with their PID and command line; `aurelia daemon
stop` terminates the daemon(s), or just one when given a `PID` from the list. These run
locally and never forward to (or auto-spawn) the daemon they manage. See also
[`kill`](#kill), which terminates **every** aurelia process. JSON shapes:
`{ "daemons": [{ "pid", "command" }] }` and `{ "killed", "pids" }`.

**Why:** Aurelia is otherwise a per-invocation CLI — each command opens a fresh Steam CM
connection and re-authenticates with the stored refresh token. Steam throttles repeated
logons aggressively (surfacing as `RateLimitExceeded`, or even a transient
`invalid credentials` lockout), which a front-end that polls Aurelia (e.g. Heroic) trips
easily. The daemon collapses that to a **single logon per daemon lifetime**.

**How it works:**

- **One server.** Start `aurelia daemon` once (e.g. at Heroic startup). It restores the
  saved session in the background and listens on a per-user endpoint — a Unix domain socket
  (`$XDG_RUNTIME_DIR/aurelia-<uid>.sock`) on Linux/macOS, or a named pipe
  (`\\.\pipe\aurelia-<user>`) on Windows.
- **Transparent forwarding.** Every other `aurelia <cmd>` automatically connects to the
  daemon and runs there against the shared session, relaying stdin, stdout, stderr and the
  exit code — so the command behaves exactly as if run directly. If no daemon is running, an
  invocation **auto-spawns** one and then connects.
- **Opt out.** Set `AURELIA_NO_DAEMON=1` to force a command to run standalone (its own
  one-off logon), bypassing the daemon entirely.
- **Login/logout** performed through the daemon update its shared session in place, so
  subsequent commands immediately use the new (or cleared) credentials.

```bash
aurelia daemon                       # start the shared-session server (run once)
aurelia daemon --socket /tmp/a.sock  # custom endpoint
aurelia info 730 --json              # auto-connects to the daemon (or spawns one)
AURELIA_NO_DAEMON=1 aurelia info 730 # bypass the daemon, run standalone
```

**Staying healthy.** The daemon keeps its session alive with Steam's connection heartbeat,
and self-heals if it drops: a background liveness probe re-establishes the shared session if
the connection dies, and a failed session restore is retried (after a short backoff) on a
later command rather than wedging the daemon. `aurelia login --reconnect` forces an immediate
re-establish. If the saved token is invalid or absent, commands needing auth still return a
clean `not logged in` error — run `aurelia login` (which the daemon picks up) to establish the
shared session.

### `kill`

Terminate **every** running aurelia process, including the session daemon. Useful after
deploying a new binary (the long-lived daemon keeps running the old code until restarted).

```text
aurelia kill [--json]
```

The invoking process is excluded, so the command lives long enough to report its result.
To stop only daemons, use [`daemon stop`](#daemon) instead. The `--json` result is
`{ "found", "killed", "pids" }`.

```bash
aurelia kill
```

---

## Files & locations

Aurelia stores its data under the user config directory:

- **Linux:** `~/.config/Aurelia`
- **Windows:** `%USERPROFILE%\.config\Aurelia`

| Path | Contents |
| --- | --- |
| `session.json` | Persisted login session (refresh token). |
| `images/` | Cached cover/header artwork (`<APP_ID>_library.jpg`). |
| `info_cache/` | Cached `info` metadata (`<APP_ID>.json`); TTL via `AURELIA_INFO_CACHE_TTL` (default 6h). |
| `logs/` | Per-launch event logs. |

Game installs live in your Steam libraries (`steamapps/common/...`), which Aurelia
discovers automatically across all connected drives.

---

## Exit codes & logging

- Commands return a non-zero exit code on failure, with an `Error:` message (and a
  `Caused by:` chain) on stderr.
- Increase verbosity with the `RUST_LOG` environment variable, e.g.:

  ```bash
  RUST_LOG=debug aurelia play 1245620
  ```
