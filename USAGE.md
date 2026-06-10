# Aurelia CLI Usage

Aurelia is a command-line Steam launcher. It authenticates against Steam, manages
your library, downloads/verifies games, and launches them — all from the terminal.

```
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
  - [`image`](#image)
- [Installation & maintenance](#installation--maintenance)
  - [`install`](#install)
  - [`update`](#update)
  - [`verify`](#verify)
  - [`uninstall`](#uninstall)
  - [`enable` / `disable`](#enable--disable)
- [Launching](#launching)
  - [`play`](#play)
- [Depots & branches](#depots--branches)
  - [`branches`](#branches)
  - [`set-branch`](#set-branch)
  - [`depots`](#depots)
- [Configuration](#configuration)
  - [`config show`](#config-show)
  - [`config protons`](#config-protons)
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

```
aurelia login [-u <USERNAME>] [-p <PASSWORD>] [-g <GUARD_CODE>] [--code] [--qr]
```

| Option | Description |
| --- | --- |
| `-u, --username <USERNAME>` | Steam account name. Prompted if omitted. |
| `-p, --password <PASSWORD>` | Account password. Prompted securely if omitted. |
| `-g, --guard <GUARD_CODE>` | Steam Guard code (email or mobile authenticator), supplied up front. |
| `--code` (alias `--pin`) | Enter the Steam Guard code **interactively** when prompted, instead of approving in the Steam Mobile app. Conflicts with `-g`. |
| `--qr` | Log in by **scanning a QR code** with the Steam Mobile app — no username/password needed. Conflicts with the credential options. |

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

### `logout`

Clear the stored session.

```bash
aurelia logout
```

---

## Library

### `list`

List games in your library (owned games merged with locally installed ones).

```
aurelia list [-i] [-s <TEXT>] [--json]
```

| Option | Description |
| --- | --- |
| `-i, --installed` | Only show installed games. |
| `-s, --search <TEXT>` | Filter by case-insensitive substring of the name. |
| `--json` | Emit JSON instead of a table. |

The `STATUS` column shows `installed`, `update` (installed with an update available), or
`-` (not installed). A non-default branch is shown in brackets after the name.

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

```
aurelia account [--json]
```

```bash
aurelia account
aurelia account --json
```

Shows account name, SteamID, country, email (and validation state), authorized device
count, and VAC ban count.

### `info`

Show detailed information about a game. No login required (public store data).

```
aurelia info <APP_ID> [--json]
```

| Option | Description |
| --- | --- |
| `--json` | Emit JSON instead of formatted text. |

Displays basic data (type, developers, publishers, release date, price, platforms,
Metacritic, website), the short description, community **tags**, **genres**,
**categories**, minimum and recommended **hardware requirements**, and the list of
**DLC** (with names resolved). Data comes from the Steam storefront API, with user tags
from SteamSpy.

```bash
aurelia info 690830
aurelia info 690830 --json
```

### `dlc`

List a game's DLC together with its ownership and install state. Requires login
(ownership is checked against your account).

```
aurelia dlc <APP_ID> [--json]
```

| Option | Description |
| --- | --- |
| `--json` | Emit JSON instead of formatted text. |

A focused alternative to `info` when you only want the DLC list. The DLC ids and
names come from the Steam storefront API (capped at 50 entries); each entry is then
annotated with:

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

### `image`

Download a game's cover/header artwork from the Steam CDN to the local image cache.

```
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

```
aurelia install <APP_ID> [-p <windows|linux>]
```

| Option | Description |
| --- | --- |
| `-p, --platform <windows\|linux>` | Depot platform to install. Auto-detected if omitted. |

If `--platform` is omitted, the available platforms are detected and the first one is
chosen (printed as `Auto-selected platform: ...`). Progress is streamed to the terminal;
the command exits non-zero if the download fails.

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

```
aurelia uninstall <APP_ID> [--delete-prefix]
```

| Option | Description |
| --- | --- |
| `--delete-prefix` | Also delete the game's Wine prefix / compatibility data (Linux). |

```bash
aurelia uninstall 1245620
aurelia uninstall 1245620 --delete-prefix
```

### `enable` / `disable`

Enable or disable an owned DLC for its base game by toggling the DLC's entry in the base
game's `appmanifest` `DisabledDLC` lists. The `<APP_ID>` is the **DLC's** app id; its base
game is resolved automatically and must be installed.

```
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

```
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

```
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

## Files & locations

Aurelia stores its data under the user config directory:

- **Linux:** `~/.config/Aurelia`
- **Windows:** `%USERPROFILE%\.config\Aurelia`

| Path | Contents |
| --- | --- |
| `session.json` | Persisted login session (refresh token). |
| `images/` | Cached cover/header artwork (`<APP_ID>_library.jpg`). |
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
