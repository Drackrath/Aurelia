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
- [Collections](#collections)
  - [`collections list`](#collections-list)
  - [`collections show`](#collections-show)
  - [`collections create` / `rename` / `delete`](#collections-create--rename--delete)
  - [`collections add` / `remove`](#collections-add--remove)
  - [`collections pull`](#collections-pull)
  - [`collections push` / `sync`](#collections-push--sync)
- [Installation & maintenance](#installation--maintenance)
  - [`install`](#install)
  - [`install list` / `install stop`](#install-list--install-stop)
  - [`libraries`](#libraries)
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
  - [`running`](#running)
  - [`stop`](#stop)
- [Depots & branches](#depots--branches)
  - [`branches`](#branches)
  - [`set-branch`](#set-branch)
  - [`depots`](#depots)
  - [`launch-options`](#launch-options)
- [Downgrade & version pinning](#downgrade--version-pinning)
  - [`manifests`](#manifests)
  - [`downgrade`](#downgrade)
  - [`pin` / `unpin`](#pin--unpin)
- [Steam Cloud](#steam-cloud)
  - [`cloud sync`](#cloud-sync)
  - [`cloud list`](#cloud-list)
- [Steam Workshop](#steam-workshop)
  - [`workshop browse`](#workshop-browse)
  - [`workshop info`](#workshop-info)
  - [`workshop list`](#workshop-list)
  - [`workshop install`](#workshop-install)
  - [`workshop uninstall`](#workshop-uninstall)
  - [`workshop subscribe` / `unsubscribe`](#workshop-subscribe--unsubscribe)
  - [`workshop status`](#workshop-status)
  - [`workshop rate`](#workshop-rate)
  - [`workshop comments`](#workshop-comments)
  - [`workshop comment`](#workshop-comment)
- [Friends & chat](#friends--chat)
  - [`friends`](#friends)
  - [`friends search`](#friends-search)
  - [`friends add` / `remove`](#friends-add--remove)
  - [`chat send`](#chat-send)
  - [`chat history`](#chat-history)
  - [`chat open`](#chat-open)
- [Inventory, wallet & market](#inventory-wallet--market)
  - [`inventory`](#inventory)
  - [`wallet`](#wallet)
  - [`market price`](#market-price)
  - [`market search`](#market-search)
  - [`market listings`](#market-listings)
- [Configuration](#configuration)
  - [`config show`](#config-show)
  - [`config protons`](#config-protons)
  - [`config presence`](#config-presence)
  - [`config language`](#config-language)
  - [`config game`](#config-game)
- [Proton & Wine runtimes](#proton--wine-runtimes)
  - [`proton list`](#proton-list)
  - [`proton install`](#proton-install)
  - [`proton uninstall`](#proton-uninstall)
  - [`proton default`](#proton-default)
- [Windows Steam runtime](#windows-steam-runtime)
  - [Steam integration policy](#steam-integration-policy)
  - [`steam-runtime install` / `repair` / `status`](#windows-steam-runtime)
- [Luxtorpeda native-engine plugin](#luxtorpeda-native-engine-plugin-linux-only)
- [umu-launcher plugin](#umu-launcher-plugin-linux-only)
- [Launch scripts](#launch-scripts)
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
- **Config location:** Aurelia stores its session, config, caches and launch logs under
  `~/.config/Aurelia` by default. Set **`AURELIA_CONFIG_DIR`** to relocate them — useful for
  an embedding driver (e.g. Heroic) that needs Aurelia's state isolated from a user's
  standalone install.
- **`AURELIA_DIAGNOSE_INSTALL=1`:** opt-in diagnostic mode for the
  [Windows Steam runtime](#windows-steam-runtime) install/repair flow — runs the installer
  with verbose `WINEDEBUG` and captures output to a timestamped log under
  `~/.config/Aurelia/logs/`. No effect on normal game launches.

`<APP_ID>` is the numeric Steam application id (visible via `aurelia list`).

---

## Authentication

### `login`

Authenticate with Steam and persist the session.

```text
aurelia login [-u <USERNAME>] [-p <PASSWORD>] [-g <GUARD_CODE>] [--code] [--qr] [--openid]
aurelia login --web-token [JSON]   # store a browser web token (web-only access)
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
| `--openid` | Verify your identity **in the browser on the official Steam sign-in page** (OpenID). Identity-only — see below. Conflicts with the credential options. |
| `--web-token [JSON]` | Store a **browser web token** enabling the web-surface commands (inventory, wallet, market listings) without a client login. Pass the `clientjstoken` JSON as the value, or omit it to be prompted. See [Web-only access](#web-only-access---web-token). Conflicts with the other login options. |
| `--health` | Report current session status **without logging in** (see below). Conflicts with all login options. |
| `--reconnect` | Rebuild the [daemon's](#daemon) shared session from the stored token. Conflicts with all login options. |

There are three ways to authenticate (plus a browser-based identity check):

1. **Password + Steam Guard.** Provide `-u`/`-p` (or be prompted). Then, depending on your
   account: pass `-g <CODE>` up front, use `--code`/`--pin` to type the code when prompted,
   or (the default) approve the login in your Steam Mobile app.
2. **`--code` / `--pin`.** Forces interactive Steam Guard **code** entry: after you submit
   credentials, Steam asks for the code (email or authenticator) and you type it in.
3. **`--qr`.** Renders a QR code in the terminal (with a `https://s.team/…` link as a
   fallback). Scan it with the Steam Mobile app to approve; no password is entered.
4. **`--openid` (identity check only).** Opens the **official Steam sign-in page**
   (`steamcommunity.com`) in your browser; after you sign in there, Steam redirects back to a
   localhost-only callback with a signed OpenID assertion, which Aurelia verifies directly
   with Steam. Your password is only ever typed on Steam's own page — never in Aurelia.

> **Why `--openid` can't be a full login.** Steam's browser sign-in for third parties is
> OpenID 2.0 (Valve offers no OAuth2/OpenID Connect endpoint), and it attests **identity
> only** — Steam never issues a client session/refresh token through it. So `--openid`
> proves *who you are* (your SteamID64, cross-checked against the stored session's account)
> but cannot create a session; commands that need one still require a `login` via password
> or `--qr`. For keeping the password out of Aurelia entirely *and* getting a session, `--qr`
> is the recommended flow. The browser left signed in by `--openid` can, however, hand over
> a **web token** — see [`--web-token`](#web-only-access---web-token) below.

#### Web-only access (`--web-token`)

While Valve issues no *client* tokens to browsers, a signed-in browser session does carry a
short-lived **web-audience token**: `https://steamcommunity.com/chat/clientjstoken` returns
`{"logged_in":true,"steamid":…,"account_name":…,"token":…}` for whoever is signed in on
`steamcommunity.com`. `aurelia login --web-token` stores that token (pasted as the flag's
value, or at a prompt), after which the **web-surface commands — [`inventory`](#inventory),
[`wallet`](#wallet), [`market listings`](#market-listings) — work with no client login**.
The OpenID success page links to the `clientjstoken` URL directly, so
`login --openid` → copy JSON → `login --web-token` is a complete browser-only setup.

Limits, so there are no surprises:

- **Web surface only.** CM-backed commands (library, install, launch, friends, cloud) are
  unaffected — they still need a full `login`/`--qr` session.
- **Short-lived (~24 h), no refresh token.** When it expires, reload the `clientjstoken`
  page in the (still signed-in) browser and re-run `login --web-token`. The command prints
  the expiry when the token carries one; Steam issues both JWT-format tokens (readable
  expiry) and opaque ones (expiry known only to Steam — shown as `unknown`).
- **Accepted pastes:** the full `clientjstoken` JSON (recommended), a bare token value, or
  a `steamLoginSecure` cookie value (`<steamid>||<token>`, `%7C%7C` accepted). A bare
  *opaque* (non-JWT) token carries no identity of its own, so it binds to the stored
  session's account — with no stored session, paste the full JSON instead.
- **Account-safe.** The paste's identity is cross-checked (JSON/cookie `steamid` vs. a JWT
  token's `sub` claim) and refused if it belongs to a different account than the stored
  session.
- A full `login` supersedes the stored web token (a CM session mints fresh web tokens
  itself), and `logout` deletes it along with the rest of the session.

For a GUI driver (e.g. Heroic): render `login --openid` in your own webview; once signed in,
fetch `clientjstoken` with the webview's session and feed the JSON to
`aurelia login --web-token --json` (see the NDJSON table below). Keeping the webview profile
persistent lets you silently re-fetch a fresh token whenever the stored one expires.

```bash
aurelia login --web-token                      # prompts for the pasted JSON
aurelia login --web-token '{"logged_in":true,"steamid":"…","account_name":"…","token":"…"}'
aurelia wallet                                 # now works without a client login
```

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

# Verify your identity on the official Steam page in the browser (no session)
aurelia login --openid

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
  `{ "logged_in", "account", "steam_id", "web_token", "daemon" }` — `daemon` indicating
  whether the answer came from the shared daemon session, `web_token` whether a browser web
  token is stored ([`--web-token`](#web-only-access---web-token)). `account`/`steam_id` are
  reported from the persisted session even when `logged_in` is false — e.g. a web-token-only
  sign-in — so a driver can show who is signed in. A poller can use this to decide whether
  `login` is needed.
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
- **OpenID:** `aurelia login --openid --json` emits `{"event":"openid_challenge","url":"https://steamcommunity.com/openid/login?…"}`;
  the driver opens the URL in a browser (Aurelia does not auto-open one in `--json` mode)
  and waits while the user signs in on the Steam page.
- **Web token:** `aurelia login --web-token --json` (no value) first checks the
  `AURELIA_WEB_TOKEN` environment variable for the `clientjstoken` JSON — the recommended
  driver channel, immune to shell/argv quoting of the embedded JSON quotes. Without it, the
  command emits
  `{"event":"web_token_required","url":"https://steamcommunity.com/chat/clientjstoken"}`
  and reads the JSON as **one line from stdin**; passing the JSON as the flag's value also
  works where quoting is under control. `--web-token` always runs locally (never through
  the daemon), so the env var is reliably visible.
- **Result:** password and QR logins end with `{"logged_in":true,"account":"<name>"}` on
  success. `--openid` ends with
  `{"openid_verified":true,"steam_id":…,"matches_stored_session":true|false|null,"logged_in":false}` —
  `logged_in` is always `false` because the OpenID flow verifies identity without creating a
  session (`matches_stored_session` is `null` when no session is stored). Failures always end
  with `{"error":"…"}` (non-zero exit).

The complete NDJSON event sequence a driver may observe, in order:

| Event line | When | Driver action |
| --- | --- | --- |
| `{"event":"awaiting_confirmation","message":"…"}` | Immediately, on password login, before the attempt blocks. | Show the message; prompt the user to approve on their device if asked. |
| `{"event":"qr_challenge","url":"…"}` | QR login; re-emitted on each code rotation. | Render `url` as a QR code and wait. |
| `{"event":"openid_challenge","url":"…"}` | OpenID identity check; once, at the start. | Open `url` in a browser; the user signs in on the Steam page. |
| `{"event":"web_token_required","url":"…"}` | `--web-token` without a value. | Fetch `url` with the signed-in browser/webview session, write the JSON as one line to the child's **stdin**. |
| `{"event":"guard_required","type":"email"\|"device"}` | A typed Steam Guard code is needed. | Read a code from the user, write it as one line to the child's **stdin**. |
| `{"event":"guard_required","type":"device_confirmation"}` | The account approves via the Steam Mobile app. | Tell the user to approve in the app; the command then completes or times out. |
| `{"logged_in":true,"account":"<name>"}` | Terminal — success (password/QR). | Done; the session is persisted. |
| `{"openid_verified":true,"steam_id":…,"logged_in":false}` | Terminal — success (`--openid`). | Identity verified; **no session was created**. |
| `{"web_token_saved":true,"steam_id":…,"expires_at":…,"logged_in":…}` | Terminal — success (`--web-token`). | Web commands now work until `expires_at`; `logged_in` reflects whether a full client session also exists. |
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
aurelia list [-i] [-s <TEXT>] [--collection <NAME>] [--online] [--json]
```

| Option | Description |
| --- | --- |
| `-i, --installed` | Only show installed games. |
| `-s, --search <TEXT>` | Filter by case-insensitive substring of the name. |
| `--collection <NAME>` | Only show games in the named [collection](#collections) (by name or id). Static collections only. |
| `--online` | Add an `ONLINE` column indicating whether each game appears to require an internet connection (see below). |
| `--json` | Emit JSON instead of a table. |

The `STATUS` column shows `installed`, `update` (installed with an update available), or
`-` (not installed). A non-default branch is shown in brackets after the name.

The `COLLECTIONS` column lists the [collections](#collections) each game belongs to
(comma-joined names of the **static** collections whose membership includes it; dynamic
collections are skipped since they can't be resolved offline). It is empty (`-`) when a game is
in no collection. The `--collection <NAME>` filter narrows the listing to a single collection's
members — resolve by name (case-insensitive) or id; an unknown name is an error, and dynamic
collections can't be used offline. The `--json` output includes a `collections` array per game.

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

For **installed** games the `--json` output also carries a `platform` field
(`"windows"`, `"linux"` or `"macos"`) — the platform of the depot actually on disk,
detected from the installed files. It tells a driver whether the game runs natively or
through Proton without re-deriving it (`null` when not installed or undetermined).

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
aurelia info <APP_ID>... [--extended] [--no-cache] [--lang <LANGUAGE>] [--json]
```

| Option | Description |
| --- | --- |
| `<APP_ID>...` | One or more app ids. Multiple ids are fetched together (see below). |
| `--extended` | Also show storefront-only fields (see below). Makes additional HTTPS storefront requests. |
| `--no-cache` | Bypass the local metadata cache and fetch fresh data from Steam. |
| `-l`, `--lang <LANGUAGE>` | Steam API language name for store text (descriptions, requirements, etc.), e.g. `german`, `french`, `schinese`. Defaults to the `aurelia config language` setting, then English. |
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
DLC list) is cached to disk per app and language under `info_cache/<APP_ID>.<LANGUAGE>.json`
in the config directory, so requests in different languages never clobber each other. A cache **hit** serves the result with **no network access at all** (no logon,
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
aurelia info 690830 --lang german        # store text in German (falls back to config/English)
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
| `-l, --lang <LANG>` | Language for names/descriptions (Steam API language name, e.g. `english`, `german`). When omitted, falls back to the [`config language`](#config-language) setting, or `english`. |
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

## Collections

Steam **library collections** are the named categories you group games into in the Steam
client (e.g. "RPGs", "Finished", plus the built-in **Favorites** and **Hidden**). Aurelia keeps
a **local working copy** in `~/.config/Aurelia/collections.json` that you edit **offline** —
create/rename/delete collections and add/remove games with no login. Those edits reach your
Steam account only when you explicitly [`pull`](#collections-pull),
[`push`](#collections-push--sync), or [`sync`](#collections-push--sync).

Collections come in two kinds:

- **Static** — an explicit list of games. These are the ones you create and edit here.
- **Dynamic** — membership is a saved filter (tags, platforms, …) that Steam evaluates. Aurelia
  round-trips these **verbatim** and never edits them, so a sync won't clobber them. `add`,
  `remove`, and `delete` refuse to touch a dynamic collection.

The built-in **`favorite`** and **`hidden`** collections can have games added/removed but
**cannot be deleted**.

> [!CAUTION]
> `push` and `sync` **write to your real Steam cloud account** and change the collections you
> see in the Steam client on every device. They ask for confirmation first; pass `--yes` to
> skip the prompt. In `--json` mode there is no prompt, so `--yes` is **required**. `pull` and
> all local edits are safe. If a `push` is rejected because your local copy is stale, run
> `aurelia collections pull` first, then push again.

Every subcommand honors the global `--json` flag.

### `collections list`

List every collection with its kind and static game count.

```bash
aurelia collections list
aurelia collections list --json
```

### `collections show`

Show a collection's member app ids. Accepts a **name** (case-insensitive) or an **id**. Dynamic
collections can't be listed offline (Steam computes their members).

```bash
aurelia collections show "RPGs"
aurelia collections show uc-1a2b3c4d --json
```

### `collections create` / `rename` / `delete`

```bash
aurelia collections create "RPGs"              # new static collection
aurelia collections rename "RPGs" "Role-Playing"
aurelia collections delete "Role-Playing"      # marked deleted locally; removed from Steam on push
```

`delete` marks the collection for deletion locally; it is tombstoned in Steam on the next
`push`/`sync`. Built-in `favorite`/`hidden` can't be deleted.

### `collections add` / `remove`

Add or remove one or more games (by app id) to/from a collection. `remove` records the game as
excluded even if it was previously added.

```bash
aurelia collections add "RPGs" 570 730 1245620
aurelia collections remove "RPGs" 730
```

### `collections pull`

Download your collections from Steam and **merge** them into the local store: memberships are
unioned per collection, brand-new remote collections are added, and remote deletions are
applied. Requires a logged-in session (`aurelia login`). Safe — it never writes to Steam.

```bash
aurelia collections pull
aurelia collections pull --json
```

### `collections push` / `sync`

`push` uploads every local collection to your Steam account. `sync` does a `pull` (to merge in
any remote changes) followed by a `push`. Both **mutate your real Steam library** and require
confirmation or `--yes`.

```bash
aurelia collections push            # prompts: "About to upload N collection(s)… Continue? [y/N]"
aurelia collections push --yes      # skip the prompt
aurelia collections sync --yes      # reconcile both sides
aurelia collections push --yes --json
```

The `list` command also integrates collections: it shows a **COLLECTIONS** column (the
comma-joined names of the static collections each game belongs to), and `aurelia list
--collection "<name>"` filters the library to a single collection's members. See
[`list`](#list).

---

## Installation & maintenance

These commands require an active session.

### `install`

Download and install a game.

```text
aurelia install <APP_ID> [-p <windows|linux>] [--library <PATH>] [--dry-run] [--restart-steam]
```

| Option | Description |
| --- | --- |
| `-p, --platform <windows\|linux>` | Depot platform to install. Auto-detected if omitted. |
| `--library <PATH>` | Steam library folder (drive/location) to install into — a library root containing a `steamapps` directory, as listed by [`aurelia libraries`](#libraries). Defaults to the configured `steam_library_path`. |
| `--dry-run` | Don't install — just report the estimated download and on-disk size. |
| `--restart-steam` | When installing a DLC, stop the Steam client and restart it afterward so the running client picks up the change (Windows). Without it, it only warns. |

If `--platform` is omitted, the available platforms are detected and the first one is
chosen (printed as `Auto-selected platform: ...`). Progress is streamed to the terminal;
the command exits non-zero if the download fails.

By default the game is installed into Aurelia's configured Steam library
(`steam_library_path`); pass `--library <PATH>` to install onto a specific drive/location
instead. List the available library folders — one per drive — with
[`aurelia libraries`](#libraries).

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

Installs run inside the [session daemon](#session-daemon), so they continue in the
background even if the foreground command is interrupted (Ctrl+C detaches the client; it
does **not** cancel the download). Use [`install stop`](#install-list--install-stop) to
cancel, or [`install list`](#install-list--install-stop) to see what's running.

### `install list` / `install stop`

Manage in-flight installs running in the daemon.

```text
aurelia install list [--json]
aurelia install stop <APP_ID> [--json]
```

`install list` shows each install in progress with its app id, byte progress, percentage
and status (`No installs in progress.` when idle). With `--json` it emits an array of
`{ "app_id", "name", "downloaded_bytes", "total_bytes", "percent", "status", "is_downloading" }`.

`install stop <APP_ID>` signals the running install for that app to abort. With `--json` it
emits `{ "event": "stopping", "app_id" }`, or `{ "event": "not_found", "app_id" }` if no
matching install is active (e.g. the daemon isn't running or the download already finished).

```bash
aurelia install list
aurelia install list --json
aurelia install stop 1245620
```

> Stopping reaches a running install only when it shares the daemon with the `stop`
> command — the normal path, since commands auto-forward to the daemon. A foreground
> install started in a separate process (e.g. with the daemon disabled) can't be stopped
> from another process.

### `libraries`

List the Steam library folders games can be installed into — one per drive/location —
each with its free space. These are the roots accepted by [`install --library`](#install)
(and `move`/`relink`/`import`). Only roots that actually contain a `steamapps` directory
are reported. No session required (a local, offline check).

```text
aurelia libraries [--json]
```

The text view prints one library per line with its free space, e.g.
`C:\Program Files (x86)\Steam  (123.4 GiB free)`. The `--json` output is
`{ "libraries": [ { "path", "free_bytes" } ] }` (`path` is the library root; `free_bytes`
is the free space on that drive in bytes, or `null` when it couldn't be determined) —
useful for populating an install-drive picker.

```bash
aurelia libraries
aurelia libraries --json
```

### `update`

Download the latest manifest for an installed game (apply a pending update). With no
app id it lists every installed game that has an update available.

```bash
aurelia update 1245620
```

A **pinned** game (see [Downgrade & version pinning](#downgrade--version-pinning)) is not
upgraded: `aurelia update <id>` refuses with a message pointing at `aurelia unpin <id>`.
Pass `--force` to update it anyway (the pin is left in place). `aurelia update` with no id
lists pinned games separately, never as "update available".

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
aurelia play <APP_ID> [-p <PROTON>] [-w] [--steam]
```

| Option | Description |
| --- | --- |
| `-p, --proton <PROTON>` | Force a specific Proton/Wine runner (Linux only). Implies a Windows target. |
| `-w, --windows` | Run the Windows executable directly with no Proton/Wine layer. **Windows hosts only** — rejected on Linux, where a Windows `.exe` can't be run natively. |
| `--steam` | Launch with real Steam integration so Steamworks/DRM work. Prefers the host Steam client (Proton's `lsteamclient` bridge, started silently if not running); when no host Steam is installed it falls back to the [Windows Steam runtime](#windows-steam-runtime) inside Wine. Without it, the game runs standalone. Implied for Family-Shared games, which require Steam to authorise the borrowed licence. The host-vs-in-Wine choice is configurable — see [`config steam-runtime-policy`](#windows-steam-runtime). |
| `--native-engine` | Route this launch through the [luxtorpeda](#luxtorpeda-native-engine-plugin-linux-only) native-engine plugin (Linux only). Installs the plugin on first use. Conflicts with `--proton`/`--windows`. |

Platform behavior:

- **On Windows**, games always run natively — there is no Proton/Wine layer — so plain
  `aurelia play <APP_ID>` works and `--windows` is implied.
- **On Linux**, native Linux builds run directly; Windows builds run through Proton/Wine.
  Use `--proton <ver>` to pin a specific runner (see `config protons` for available names).
  The launch entry is chosen by the **installed depot**: Aurelia picks the executable that
  actually exists on disk, so a game installed as a native Linux build runs natively even if
  `--proton` is passed.

Steam integration (`--steam`):

- **Standalone (default):** the game runs without a Steam client. Works for owned games with
  lenient DRM; Steam DRM and Steamworks online features are unavailable.
- **With `--steam`:** Aurelia provides a real Steam client so DRM/Steamworks work. It resolves
  the source from the [`steam-runtime-policy`](#windows-steam-runtime) (global default, or the
  per-game policy set with `config game <id> --steam-runtime`):
  - **`auto` (default):** use the **host** Steam client when one is installed (starting it
    silently), otherwise fall back to the **Windows Steam runtime inside Wine** (see below).
  - **`on`:** always use the in-Wine Steam runtime, even if a host Steam exists.
  - **`off`:** host Steam only; never the in-Wine runtime.

  The in-Wine fallback requires the runtime to be installed first
  (`aurelia steam-runtime install`). If neither a host Steam nor the in-Wine runtime is
  available, the launch runs standalone and DRM-protected games will not start.

Required for Steamworks online features and for **Family-Shared** games (where `--steam` is
forced on).

```bash
aurelia play 1245620                 # native on Windows / auto on Linux, standalone
aurelia play 1245620 --windows       # Windows host: force native execution
aurelia play 1245620 --proton experimental   # Linux: pin a Proton version
aurelia play 1245620 --steam         # real Steam integration (host, else in-Wine runtime)
```

---

### `running`

List the games Aurelia is currently running (launched via `aurelia play`). Records whose
process has already exited are pruned, so the list reflects what's actually live.

```text
aurelia running [--json]
```

```bash
aurelia running
aurelia running --json
```

### `stop`

Stop a running game previously launched with `aurelia play`. Omitting the app id lists the
running games (same as [`running`](#running)).

```text
aurelia stop [<APP_ID>] [--force]
```

| Option | Description |
| --- | --- |
| `--force` | Kill the game immediately (SIGKILL) instead of asking it to exit gracefully first. Use when a game is hung and ignores a normal stop. |

A normal stop asks the game to exit cleanly (so it can save); `--force` terminates it at once.
Either way Aurelia sweeps the game's whole process tree — including Proton's re-parented
`steam.exe`/`wineserver`/game processes (matched by `STEAM_COMPAT_APP_ID`) — so nothing is
left behind.

```bash
aurelia stop                # list running games
aurelia stop 1245620        # graceful stop
aurelia stop 1245620 --force   # force-kill a hung game
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

## Downgrade & version pinning

Steam's client only ever installs the *current* manifest for a branch. These commands let
you install an **older build** of a game (a specific depot manifest) and keep it from being
updated back. This modifies real game files and Steam metadata — respect the at-your-own-risk
stance and test on a game you can re-verify.

### `manifests`

List, per depot, the **current** manifest id on every branch Steam advertises (version
discovery). Steam does **not** expose historical/older ids — for those, use the printed
[SteamDB](https://steamdb.info) depot pages.

```text
aurelia manifests <APP_ID> [--depot <DEPOT_ID>] [--json]
```

| Option | Description |
| --- | --- |
| `--depot <DEPOT_ID>` | Only show this depot. |
| `--json` | Emit `[ { depot_id, depot_name, branch, manifest_id, size } ]`. |

```bash
aurelia manifests 1245620
aurelia manifests 1245620 --depot 1245621
```

Text output is a `DEPOT · BRANCH · MANIFEST_ID · SIZE · NAME` table followed by the SteamDB
manifest-history link for each depot (`https://steamdb.info/depot/<depot_id>/manifests/`),
where older manifest ids can be found.

### `downgrade`

Install specific (usually older) depot manifests and, by default, **pin** them. Downgrading
requires an authenticated, **owning** session (an anonymous login can't fetch request codes /
depot keys for owned content). Progress streams like `install`.

```text
aurelia downgrade <APP_ID> --depot <DEPOT_ID> --manifest <MANIFEST_ID>  # repeatable pair
aurelia downgrade <APP_ID> --manifest <DEPOT_ID>:<MANIFEST_ID> ...       # combined form
                  [--branch <name>] [--branch-password <p>]
                  [--library <path>] [--verify] [--no-pin] [--json]
```

| Option | Description |
| --- | --- |
| `--depot <DEPOT_ID>` | Target depot (repeatable). Pairs by position with a bare `--manifest`. |
| `--manifest <ID>` | Target manifest id: a bare id (paired with `--depot`) or `<depot>:<manifest>`. |
| `--branch <name>` | Branch whose build id is recorded in the appmanifest (default `public`). |
| `--branch-password <p>` | Password for a protected branch (recorded for reference only). |
| `--library <path>` | Steam library folder to install into. |
| `--verify` | Run an integrity pass after downloading (against the downgraded manifests). |
| `--no-pin` | Don't pin afterward (Aurelia's update commands may then re-upgrade it). |

`--depot` and `--manifest` are **parallel repeatable lists** paired by index and must be
equal in length (unequal counts are rejected). Depots you don't name keep their current
manifest, so you can pin just the one depot that matters. Only the named depots are
downgraded; a full downgrade of a multi-depot build may need several depots at matching ids.

```bash
# Find the depot, get an old manifest id from SteamDB, then:
aurelia downgrade 1245620 --depot 1245621 --manifest 8593343465227540543
# Two depots at once (combined form):
aurelia downgrade 1245620 --manifest 1245621:8593343465227540543 --manifest 1245622:1234567890
```

If Steam declines a manifest request code (typical for very old manifests, HTTP 401), the
command fails with a clear message — the manifest may be too old, or you may need to own the
game with a non-anonymous login.

### `pin` / `unpin`

Manage Aurelia's update lock directly. `pin` records a game's currently-installed manifests
and blocks Aurelia's `update` / `check-updates` from upgrading it; `unpin` releases it.

```text
aurelia pin <APP_ID>      [--json]
aurelia unpin <APP_ID>    [--json]
```

```bash
aurelia pin 1245620
aurelia update 1245620      # -> refuses: "app is pinned — run `aurelia unpin 1245620` first"
aurelia unpin 1245620
```

> **The pin is soft.** It writes `AutoUpdateBehavior "1"` (update only on launch) into the
> `.acf` and records the pin in Aurelia's config, and it is authoritative for **Aurelia's own
> commands**. The **official Steam client** can still re-queue an update — for example when
> you launch the game through Steam. The reliable way to keep a downgraded build is to launch
> it via Aurelia and avoid updating it in the Steam client.

---

## Steam Cloud

These commands require an active session.

### `cloud sync`

Synchronise a game's Steam Cloud saves with their real on-disk locations.

```text
aurelia cloud sync <APP_ID> [--up | --down] [--path <DIR>] [--resolve <cloud|local>] [--json]
```

| Option | Description |
| --- | --- |
| `--up` | Only upload local saves to Steam. Conflicts with `--down`. |
| `--down` | Only download saves from Steam. Conflicts with `--up`. |
| `--path <DIR>` | Override the base directory for **classic** (token-less) remote-storage files. Defaults to `<userdata>/<account>/<appid>/remote`. Does **not** affect Auto-Cloud files (see below). |
| `--resolve <cloud\|local>` | How to resolve a **diverged** save (see [Conflicts](#conflicts) below). `cloud` overwrites local with the Steam copy; `local` overwrites the Steam copy with the on-disk one. Omit to only **detect** conflicts and leave both copies untouched. |
| `--json` | Emit a JSON result instead of text. |

With **neither** flag it performs a full sync — **down then up** — matching what `play` does
around a launch. `--down` or `--up` restrict it to one direction. The `--json` result is
`{ "app_id", "direction": "both"|"down"|"up", "remote_root", "status": "ok"|"conflicts",
"downloaded": [..], "uploaded": [..], "conflicts": [..] }` (`downloaded`/`uploaded` are the
filenames moved; `conflicts` is described below).

#### Conflicts

Aurelia never decides a save by timestamp alone — a clock skew or a restored file can make
the wrong copy "newer" and silently destroy progress. Instead it compares the **content
hash** of each save against a small per-app baseline (the last state at which cloud and local
agreed, stored under `cloud_sync/<APP_ID>.json` in the config dir):

- identical content → nothing is moved;
- only one side changed since the baseline → it's moved the obvious way (a newer local save
  uploads; a newer cloud save downloads);
- **both** sides changed independently (or there's no baseline yet and the two copies differ)
  → a **conflict**: the copies have diverged and neither is touched. Each conflicting file is
  reported in the `conflicts` array as `{ filename, local_path, local_hash, local_size,
  local_timestamp, cloud_hash, cloud_size, cloud_timestamp }`, and `status` is `"conflicts"`.

Re-run with `--resolve cloud` (keep Steam's copy) or `--resolve local` (keep the on-disk copy)
to apply a choice; resolving records a fresh baseline so later syncs are clean. This is the
contract a front-end (e.g. Heroic) drives to show a **Take Cloud / Take Local** prompt — it
runs a plain sync, and if `status` is `conflicts` it asks the user, then re-runs with
`--resolve`. The same conflict-safe logic guards the automatic sync around
[`play`](#play): a diverged save is left untouched (logged as a warning) rather than
overwritten.

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

**Direction logic.** Direction filters only the *automatic* (non-conflicting) transfers;
conflicts are detected in every direction.

- **down** — applies cloud-side changes: a cloud file is written to its mapped local path
  (then stamped with the cloud's modification time so a later sync doesn't see it as locally
  changed). A save that also changed locally is reported as a conflict, not overwritten.
- **up** — applies local-side changes: a changed local save is uploaded. The candidate set is
  the union of (a) files already in the cloud and (b) local files matched by the app's UFS
  `savefiles` rules (read from appinfo), so a **brand-new** save that has never been in the
  cloud still gets its first upload.

```bash
aurelia cloud sync 1245620                 # down then up; report any conflicts
aurelia cloud sync 1245620 --down          # pull cloud changes only
aurelia cloud sync 1245620 --up            # push local changes only
aurelia cloud sync 1245620 --resolve cloud # on conflict, keep Steam's copy
aurelia cloud sync 1245620 --resolve local # on conflict, keep the on-disk copy
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

## Steam Workshop

Manage Steam Workshop items (**published files**) for a game. Except for the local
portion of `status`, these commands require an active session.

An `<ID>` is a Workshop **published-file id** — the numeric id in a Workshop page's URL
(`…/sharedfiles/filedetails/?id=1234567890`). To **find** ids in the first place, use
[`workshop browse`](#workshop-browse). Wherever ids are accepted, a **collection** id may
be given in its place: by default a collection is **expanded to its member items**
(recursively, with cycles and duplicates removed), and each member is acted on. Pass
`--no-recurse` to act on the listed id itself without expanding.

**What gets stored.** Installing downloads an item's content into

```text
<library>/steamapps/workshop/content/<APP_ID>/<ID>/
```

and records it in `<library>/steamapps/workshop/appworkshop_<APP_ID>.acf`, so the Steam
client itself recognises the item as installed. The library is Aurelia's configured Steam
library (see [`config show`](#config-show)).

**How content is retrieved.** A Workshop item carries an `hcontent_file` manifest id on the
game's workshop depot; Aurelia downloads it through the **same** content-server → manifest →
CDN-chunk pipeline used by [`install`](#install) (decrypting and decompressing each chunk
with the depot key). This v1 supports **SteamPipe** items (the modern norm); **legacy
`file_url` UGC** is not yet supported and is rejected with a clear error.

### `workshop browse`

Search/browse a game's Workshop to **discover** items (and their ids) to subscribe to or
install — `PublishedFile.QueryFiles` under the hood. This is the entry point when you don't
already know an item's id.

```text
aurelia workshop browse <APP_ID> [-s <TEXT>] [--sort <ORDER>] [--count <N>] [--cursor <C>] [--tag <TAG>]... [--json]
```

| Option | Description |
| --- | --- |
| `-s, --search <TEXT>` | Free-text match on title/description. |
| `--sort <ORDER>` | Result ordering: `trend` (default), `popular`, `recent`, `updated`, `subscriptions`, `text`. Use `text` with `--search` for best relevance. |
| `--count <N>` | Results per page, 1–100 (default 20). |
| `--cursor <C>` | Pagination cursor; pass a previous page's `next_cursor`. `*` is the first page (default). |
| `--tag <TAG>` | Restrict to items carrying this tag (repeatable; all must match). |

The text view is an `ID / SIZE / TITLE` table, followed by the match total and — when more
results remain — the `--cursor` value to pass for the next page. The `--json` output is
`{ "app_id", "total", "next_cursor", "items": [ <item object>... ] }`, where each item has
the same shape as [`workshop info`](#workshop-info). Paging is **cursor-based**: start at
`*`, then feed each response's `next_cursor` back via `--cursor` until it stops advancing.

```bash
aurelia workshop browse 1245620                          # trending items
aurelia workshop browse 1245620 --search "hd textures"   # search
aurelia workshop browse 1245620 --sort subscriptions --count 50
aurelia workshop browse 1245620 --tag Gameplay --tag Mod --json
aurelia workshop browse 1245620 --cursor "AoJw0Yzg..."   # next page
```

### `workshop info`

Show metadata for one or more items/collections, fetched in a single batched
`PublishedFile.GetDetails` call.

```text
aurelia workshop info <ID>... [--json]
```

The text view prints id, title, owning app, type (`item`/`collection`), and — for items —
size and content manifest id; for collections, the member count. The `--json` output is an
array of objects (a single id still yields a one-element array): `{ "id", "app_id",
"title", "hcontent_file", "file_url", "file_size", "time_updated", "kind"
("Item"|"Collection"), "children": [<id>...] }` (`children` is populated for collections,
empty otherwise; `hcontent_file` is `0` for legacy/collection entries).

```bash
aurelia workshop info 1234567890
aurelia workshop info 1234567890 2345678901 --json
```

### `workshop list`

List the Workshop items you're subscribed to for a game (your subscriptions are enumerated,
then resolved to metadata).

```text
aurelia workshop list <APP_ID> [--json]
```

The text view is an `ID / SIZE / TITLE` table; `--json` emits the same array of item objects
as [`workshop info`](#workshop-info).

```bash
aurelia workshop list 1245620
aurelia workshop list 1245620 --json
```

### `workshop install`

Download one or more items/collections and register them in the workshop manifest. Progress
is streamed to the terminal, and as NDJSON with `--json` — the same event stream as
[`install`](#install) (`queued` → `progress` …), followed by one result line per item.

```text
aurelia workshop install <ID>... [--no-recurse] [--json]
```

| Option | Description |
| --- | --- |
| `--no-recurse` | Install only the given ids; don't expand a collection to its members. |

The per-item `--json` result line is
`{ "event": "result", "id", "app_id", "status": "installed" }`. Installing a collection id
(without `--no-recurse`) installs every member.

```bash
aurelia workshop install 1234567890
aurelia workshop install 5000000000              # a collection — installs all its items
aurelia workshop install 9000000000 --no-recurse # the collection entry only, no members
```

### `workshop uninstall`

Remove installed items/collections — deletes each item's content directory and its entry in
`appworkshop_<APP_ID>.acf`. The owning app of each id is resolved via `GetDetails` (so this
needs a session).

```text
aurelia workshop uninstall <ID>... [--no-recurse] [--json]
```

| Option | Description |
| --- | --- |
| `--no-recurse` | Uninstall only the given ids; don't expand a collection to its members. |

The `--json` output is `{ "uninstalled": [<id>...] }`.

```bash
aurelia workshop uninstall 1234567890
aurelia workshop uninstall 5000000000 --json
```

### `workshop subscribe` / `unsubscribe`

Subscribe to or unsubscribe from items/collections. By default `subscribe` only **registers**
the subscription; pass `--install` to also download the content immediately (streaming
progress, as `install` does).

```text
aurelia workshop subscribe   <ID>... [--install] [--no-recurse] [--json]
aurelia workshop unsubscribe <ID>... [--no-recurse] [--json]
```

| Option | Description |
| --- | --- |
| `--install` | (`subscribe` only) Download the content after subscribing. |
| `--no-recurse` | Act on the given ids only; don't expand a collection to its members. |

`subscribe` `--json`: `{ "subscribed": [<id>...], "installed": <bool> }`.
`unsubscribe` `--json`: `{ "unsubscribed": [<id>...] }`.

```bash
aurelia workshop subscribe 1234567890 --install
aurelia workshop subscribe 5000000000            # subscribe to a collection's items
aurelia workshop unsubscribe 1234567890
```

### `workshop status`

Report, per item, whether it's **installed**, **subscribed**, and whether an **update** is
available (the installed content manifest differs from the current `hcontent_file`). The
installed set is read **locally** from `appworkshop_<APP_ID>.acf`; subscription and
update state are **best-effort** — they need the network, and are omitted/blank when offline.

```text
aurelia workshop status <APP_ID> [--json]
```

The text view is an `ID / INSTALLED / SUBSCRIBED / UPDATE / TITLE` table over the union of
installed and subscribed items. The `--json` output is `{ "app_id", "items": [{ "id",
"title", "installed", "subscribed", "update_available" }] }`.

```bash
aurelia workshop status 1245620
aurelia workshop status 1245620 --json
```

### `workshop rate`

Rate a Workshop item **thumbs-up** or **thumbs-down** (`PublishedFile.Vote`).

```text
aurelia workshop rate <ID> <up|down> [--json]
```

The `--json` output is `{ "id", "vote": "up"|"down", "status": "rated" }`.

```bash
aurelia workshop rate 1234567890 up
aurelia workshop rate 1234567890 down --json
```

### `workshop comments`

Read the comments on a Workshop item's public comment thread
(`Community.GetCommentThread`).

```text
aurelia workshop comments <ID> [--count <N>] [--start <N>] [--json]
```

| Option | Description |
| --- | --- |
| `--count <N>` | How many comments to fetch, 1–100 (default 20). |
| `--start <N>` | Index of the first comment to fetch, for paging (default 0). |

The text view prints each comment's timestamp, author SteamID, upvote count, and body. The
`--json` output is `{ "id", "comments": [{ "id", "author", "timestamp", "text", "upvotes"
}] }` (`author` is a SteamID64; `id` is the comment's `gidcomment`).

```bash
aurelia workshop comments 1234567890
aurelia workshop comments 1234567890 --count 50 --start 50
aurelia workshop comments 1234567890 --json
```

### `workshop comment`

Post a comment to a Workshop item's comment thread (`Community.PostCommentToThread`).

```text
aurelia workshop comment <ID> <TEXT> [--json]
```

Quote the text if it contains spaces. The `--json` output is
`{ "id", "comment_id", "status": "posted" }` (`comment_id` is the new comment's
`gidcomment`).

```bash
aurelia workshop comment 1234567890 "Great mod, thanks!"
aurelia workshop comment 1234567890 "Works perfectly" --json
```

---

## Friends & chat

Manage your Steam friends and exchange direct (friend-to-friend) messages. These commands
require an active session — **except** [`friends search`](#friends-search), which only reads
public profile data and needs no login.

To receive friend data and incoming messages, the session must announce a Steam
**presence** — Steam treats a refresh-token logon as *offline* and withholds friend persona
state and chat until a presence is declared. The [`daemon`](#daemon) does this automatically
on connect using the configured [`config presence`](#config-presence); a standalone session
announces it on demand. Presence **defaults to offline**, which is an *invisible* presence:
you appear offline to friends but still sync your friends list and receive messages. Change
it with [`config presence`](#config-presence).

A `<STEAMID>` below is a friend's **SteamID64** — the 17-digit id shown by [`friends`](#friends).

### `friends`

List your Steam friends with display name, online status, and current game.

```text
aurelia friends [--json]
```

The text view is a `STATUS / STEAMID / NAME / GAME` table, where `STATUS` is the persona
state (`online`, `offline`, `busy`, `away`, `snooze`, `looking to trade`, `looking to
play`). The `--json` output is an array of `{ "steam_id", "relationship", "persona_name",
"persona_state", "game_app_id", "game_name" }` (only accepted friends — `relationship` 3 —
are listed; `persona_state` is the raw EPersonaState integer).

Through the [`daemon`](#daemon) the roster is served from a background watcher that keeps it
live as Steam pushes friend/persona updates, so a call **right after the daemon starts** may
be briefly empty until the initial list arrives — run it again a moment later. Standalone it
does a short best-effort collection over a fresh connection.

```bash
aurelia friends
aurelia friends --json
```

`aurelia friends` is shorthand for `aurelia friends list`.

### `friends search`

Resolve a Steam account to its **SteamID64** from a flexible identifier. **No login
required** — it reads the public Steam Community profile data (`?xml=1`), so it works as a
standalone lookup. Steam exposes no free-text people search over the protocol, so this
resolves a specific identifier rather than searching by display name.

```text
aurelia friends search <QUERY> [--json]
```

`<QUERY>` may be any of:

- a 17-digit **SteamID64** (validated and echoed back, with the name looked up),
- a **profile URL** — `https://steamcommunity.com/profiles/<id>`,
- a **custom (vanity) URL** — `https://steamcommunity.com/id/<name>`, or
- a bare **vanity name** (the custom-URL slug).

The text view prints the SteamID, display name, and profile URL. The `--json` output is
`{ "steam_id", "persona_name", "profile_url" }` (`persona_name` is `null` if the profile
hides it). Feed the resulting SteamID to [`friends add`](#friends-add--remove) or the
[`chat`](#chat-send) commands. Resolution fails with a clear error if no such profile or
custom URL exists.

```bash
aurelia friends search gabelogannewell
aurelia friends search https://steamcommunity.com/id/gabelogannewell
aurelia friends search 76561197960287930 --json
```

### `friends add` / `remove`

Send a friend request, or remove a friend / cancel a pending request. Both require an active
session.

```text
aurelia friends add <QUERY> [--json]
aurelia friends remove <STEAMID> [--json]
```

`add` accepts the **same flexible `<QUERY>`** as [`friends search`](#friends-search) (a
SteamID64, profile URL, custom URL, or vanity name); it is resolved to a SteamID and a
request is sent. Aurelia waits for Steam's confirmation and reports the target (and its name,
when Steam returns one); a non-OK result is surfaced as a clear error (e.g. friend-list full,
rate-limited, or access denied). The `--json` output is
`{ "steam_id", "persona_name", "status": "request_sent" }`.

`remove` takes a **SteamID64** (as listed by [`friends`](#friends)) and removes that friend
or withdraws a request you sent. It is fire-and-forget — Steam returns no acknowledgement —
so it reports success once the message is sent. The `--json` output is
`{ "steam_id", "status": "removed" }`.

```bash
aurelia friends add 76561197960287930
aurelia friends add https://steamcommunity.com/id/someone   # resolve, then request
aurelia friends remove 76561197960287930
```

> Friend requests are visible to the recipient. Sending many in a short time can trip Steam's
> rate limits (surfaced as an `EResult 84` error).

### `chat send`

Send a direct message to a friend.

```text
aurelia chat send <STEAMID> <MESSAGE>...
```

All words after the SteamID form the message (quote it to be safe). The `--json` output is
`{ "steamid", "sent", "server_timestamp", "modified_message" }` (`modified_message` is set
only when Steam rewrites the text, e.g. applying bbcode).

```bash
aurelia chat send 76561198042323314 "on my way"
aurelia chat send 76561198042323314 "ggwp" --json
```

### `chat history`

Show recent messages exchanged with a friend, most recent first.

```text
aurelia chat history <STEAMID> [--count <N>] [--json]
```

| Option | Description |
| --- | --- |
| `--count <N>` | How many recent messages to fetch (default 20). |

The text view prints `[<timestamp>]  me/them: <text>`. The `--json` output is an array of
`{ "sender", "from_self", "message", "timestamp" }` (`sender` a SteamID64, `timestamp` a
Unix time).

```bash
aurelia chat history 76561198042323314
aurelia chat history 76561198042323314 --count 50 --json
```

### `chat open`

Open an **interactive live chat** with a friend: incoming messages stream to stdout while
each line you type on stdin is sent. The session ends on stdin EOF — **Ctrl-D** (or
**Ctrl-Z** then Enter on Windows) — or if the connection closes.

```text
aurelia chat open <STEAMID> [--json]
```

In plain mode it prints `them: <text>` for incoming messages, `me (sent elsewhere): <text>`
for messages you sent from another device, and a typing indicator. With `--json` it becomes
an **NDJSON event stream** for front-ends, reading message text from stdin lines and
emitting one event per line:

| Event line | When |
| --- | --- |
| `{"event":"ready","with":<steamid>}` | Session established. |
| `{"event":"message","from":<steamid>,"text":"…","timestamp":<unix>}` | A message arrived from the friend. |
| `{"event":"echo","to":<steamid>,"text":"…","timestamp":<unix>}` | A message you sent from another device. |
| `{"event":"typing","from":<steamid>,"timestamp":<unix>}` | The friend is typing. |
| `{"event":"error","message":"…"}` | A send failed. |
| `{"event":"closed","with":<steamid>}` | Terminal — the session ended. |

It runs naturally over the [`daemon`](#daemon): the thin client streams your stdin and
relays the daemon's stdout in real time, so the live session reuses the shared, already-online
connection.

```bash
aurelia chat open 76561198042323314
echo "brb" | aurelia chat open 76561198042323314      # send one line, then exit on EOF
aurelia chat open 76561198042323314 --json            # NDJSON event stream
```

---

## Inventory, wallet & market

Read-only access to your Steam **inventory**, **wallet** balance, and the **Community
Market** (item prices, search, and your own listings). Item price and market search are
**public** and need no login; inventory, wallet, and your listings require an active session
— or a browser **web token** stored via
[`login --web-token`](#web-only-access---web-token), which powers exactly these commands
without a client login.

> **Market eligibility.** Wallet and market features require the account to be eligible for
> the Steam Community Market — Steam mandates the Steam Guard Mobile Authenticator enabled for
> 15+ days (and no recent new-device holds). Ineligible accounts get a clear error rather than
> data. (Buying/selling are not implemented yet — see
> [docs/community-market-plan.md](docs/community-market-plan.md).)

### `inventory`

List the logged-in account's inventory for a game.

```text
aurelia inventory <APP_ID> [--context <ID>] [--json]
```

| Option | Description |
| --- | --- |
| `--context <ID>` | Inventory context id (default `2`). Steam community items (cards, gems, backgrounds) live under **app `753`, context `6`**. |

The text view is an `AMOUNT / TRADE / MKT / NAME` table (TRADE = tradable, MKT = marketable).
The `--json` output is an array of `{ "asset_id", "class_id", "name", "market_hash_name",
"item_type", "amount", "tradable", "marketable", "icon_url" }`.

```bash
aurelia inventory 753 --context 6      # your Steam cards / gems / backgrounds
aurelia inventory 730                  # CS2 items (context 2)
aurelia inventory 753 --context 6 --json
```

### `wallet`

Show your Steam Wallet balance. Requires an active, market-eligible session.

```text
aurelia wallet [--json]
```

The `--json` output is `{ "balance_cents", "currency", "country", "formatted" }`
(`balance_cents` in the currency's minor units; `currency` is the Steam currency id).

```bash
aurelia wallet
aurelia wallet --json
```

### `market price`

Look up an item's Community Market price. **Public** — no login required.

```text
aurelia market price <APP_ID> <NAME> [--currency <ID>] [--json]
```

| Option | Description |
| --- | --- |
| `<NAME>` | The exact **market hash name** (case-sensitive), e.g. `"Mann Co. Supply Crate Key"`. Quote it. |
| `--currency <ID>` | Steam currency id (`1`=USD, `2`=GBP, `3`=EUR, …). Default `1`. |

The text view prints the lowest price, median price, and 24-hour volume. The `--json` output
is `{ "market_hash_name", "lowest_price", "median_price", "volume" }` (prices are
Steam-formatted strings; any may be `null` if Steam has no data).

```bash
aurelia market price 440 "Mann Co. Supply Crate Key"
aurelia market price 730 "AK-47 | Redline (Field-Tested)" --currency 3 --json
```

### `market search`

Search the Community Market. **Public** — no login required.

```text
aurelia market search [QUERY] [--app-id <ID>] [--count <N>] [--json]
```

| Option | Description |
| --- | --- |
| `QUERY` | Free-text query (optional). |
| `--app-id <ID>` | Restrict to one game. |
| `--count <N>` | Maximum results (default 20). |

The text view is a `PRICE / LIST / NAME` table (LIST = number of active sell listings). The
`--json` output is `{ "total_count", "results": [ { "name", "market_hash_name", "app_id",
"app_name", "sell_listings", "sell_price", "sell_price_text" } ] }` (`sell_price` in minor units).

```bash
aurelia market search "Sticker" --app-id 730 --count 10
aurelia market search --app-id 753 --json
```

### `market listings`

Show your own active market listings and open buy orders. Requires an active session.

```text
aurelia market listings [--json]
```

The `--json` output is `{ "listings": [ { "listing_id", "market_hash_name", "price" } ],
"buy_orders": [ { "buy_order_id", "market_hash_name", "price", "quantity" } ] }` (prices in
minor units).

```bash
aurelia market listings
aurelia market listings --json
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
`compatibilitytools.d`. (See also [`proton list`](#proton-list), which adds runtimes
available to **download**.)

```bash
aurelia config protons
```

### `config presence`

View or set the Steam **presence** the session daemon announces so it can sync your friends
list and receive chat (see [Friends & chat](#friends--chat)). Run with no argument to print
the current value.

```text
aurelia config presence [online|offline] [--json]
```

| Value | Meaning |
| --- | --- |
| `offline` (default) | An **invisible** presence: you appear offline to friends, but the daemon still receives your friends list and incoming messages. |
| `online` | You appear **online** to friends while the daemon is running. |

The setting is read when the daemon establishes its session, so after changing it **restart
the daemon** for it to take effect (`aurelia daemon stop`, or `aurelia kill`). The `--json`
output is `{ "chat_presence": "online"|"offline" }`.

```bash
aurelia config presence                 # print the current presence
aurelia config presence online          # appear online to friends
aurelia config presence offline --json  # back to invisible; {"chat_presence":"offline"}
```

### `config language`

View or set the default **Steam API language name** used by
[`achievements`](#achievements) when its `--lang` flag is not given. Run with no
argument to print the current value (unset means `english`); pass an empty string
to clear it.

```text
aurelia config language [<NAME>] [--json]
```

```bash
aurelia config language            # print the current default (or "english")
aurelia config language german     # set German as the default
aurelia config language ""         # clear it (back to english)
```

The `--json` output is `{ "language": "german"|null }`.

### `config game`

View or set a game's **per-game launch settings** — the Proton/Wine version it runs with
and its platform target. Run with no flags to print the current settings.

```text
aurelia config game <APP_ID> [--proton <VERSION>] [--clear-proton] [--platform <windows|linux>] [--json]
```

| Option | Description |
| --- | --- |
| `--proton <VERSION>` | Pin the Proton/Wine version for this game (a name from [`proton list`](#proton-list)). Overrides the global default at launch. |
| `--clear-proton` | Remove the per-game version, so the game falls back to the [global default](#proton-default). |
| `--platform <windows\|linux>` | Force the platform target. `windows` runs through Proton/Wine on Linux. |
| `--native-engine` | Route this game through the [luxtorpeda](#luxtorpeda-native-engine-plugin-linux-only) native-engine plugin (Linux only; requires `aurelia luxtorpeda enable`). |
| `--no-native-engine` | Clear luxtorpeda routing, back to normal native/Proton selection. |
| `--umu` | Route this game through the [umu-launcher](#umu-launcher-plugin-linux-only) plugin (Proton via umu; Linux only; requires `aurelia umu enable`). |
| `--no-umu` | Clear umu routing, back to normal native/Proton selection. |

At launch, the Proton version is resolved in this order: an explicit `play --proton` flag →
this per-game version → the global default (only when the game targets Windows). The
`--json` output is `{ "app_id", "forced_proton_version", "platform_preference", "runner" }`.

```bash
aurelia config game 1245620                          # show current settings
aurelia config game 1245620 --proton GE-Proton9-20   # pin a Proton version
aurelia config game 1245620 --clear-proton           # back to the global default
aurelia config game 1245620 --platform windows
```

---

## Proton & Wine runtimes

Download and manage the Proton/Wine runtimes games launch through. Three sources are
supported:

- **Official Valve Proton** — free Steam apps (Proton Experimental, 9.0, 8.0, …), installed
  through the normal content pipeline into `steamapps/common` (needs an active session).
- **GE community builds** — GloriousEggroll's **Proton-GE** and **Wine-GE**, downloaded from
  GitHub releases and extracted into `compatibilitytools.d` (no session needed).
- **Proton-CachyOS** — the CachyOS community build, downloaded from GitHub. Aurelia detects
  the host CPU and, when it supports AVX2, prefers the microarchitecture-optimized
  `x86_64_v3` asset (labelled *AVX2 optimized* in `proton list`), otherwise the generic
  `x86_64` build.

> Modern **unified Proton layouts** (Proton 11+, GE, and Proton-CachyOS, which nest their
> libraries under `files/lib/wine/…` with WOW64 `-unix`/`-windows` split components) are
> discovered automatically, with strict 32-/64-bit filtering so a game never loads a
> wrong-bitness DLL.

The **global default** runtime is used when a game has no per-game version
([`config game`](#config-game)) set. **Installing a runtime makes it the new default** (the
"last downloaded" rule); change it explicitly with [`proton default`](#proton-default).

### `proton list`

List installable runtimes (Valve + GE, the latter fetched from GitHub) alongside what's
already installed, with the current default marked.

```text
aurelia proton list [--installed] [--json]
```

| Option | Description |
| --- | --- |
| `--installed` | Only show runtimes installed on disk (skips the network lookup). |

The text view is a `SOURCE / NAME / SIZE / STATUS` table; `--json` emits
`{ "default", "installed": [...], "available": [...] }`. Set `GITHUB_TOKEN` to lift GitHub's
unauthenticated rate limit if you hit it.

```bash
aurelia proton list
aurelia proton list --installed
aurelia proton list --json
```

### `proton install`

Download and install a runtime by name, then set it as the global default. GE builds are
fetched/extracted directly; an official Proton name installs via Steam (with streamed
progress, like [`install`](#install)).

```text
aurelia proton install <VERSION>
```

```bash
aurelia proton install GE-Proton9-20     # GE build from GitHub
aurelia proton install "Proton 9.0"      # official Valve Proton via Steam
aurelia proton install Proton-CachyOS    # CachyOS build (auto-picks the x86_64_v3 asset on AVX2 hosts)
```

### `proton uninstall`

Delete an installed **custom (GE)** runtime from `compatibilitytools.d`. Official Valve
Proton is removed through Steam (or `aurelia uninstall <app_id>`), not here.

```text
aurelia proton uninstall <VERSION>
```

```bash
aurelia proton uninstall GE-Proton9-19
```

### `proton default`

Set the global default Proton/Wine version (used by any game without a per-game override).

```text
aurelia proton default <VERSION>
```

```bash
aurelia proton default GE-Proton9-20
```

---

## Windows Steam runtime

Some Windows games require a live Steam client for their Steamworks/DRM handshake. Aurelia
can host a self-contained **master Windows Steam prefix** — a Wine prefix with Steam
installed inside it — and start that Steam in the background purely to answer the in-prefix
handshake, while the game itself is still launched **directly** by Aurelia (never through a
`steam://run` / `-applaunch` handoff). It is the no-host-Steam path for
[`play --steam`](#play): when you launch with `--steam` and no host Steam client is
installed, Aurelia uses this in-Wine runtime for the DRM/Steamworks handshake (subject to the
[policy](#steam-integration-policy) below).

Installing/repairing the master prefix needs a Wine/Proton runner configured as
`steam_runtime_runner` — background Steam runs under bare Wine, not through Proton's
`proton run` wrapper.

**First-time setup.** `steam-runtime install` fails until a runner is selected. Pick one
from your installed runtimes and set it:

```bash
aurelia proton list                                 # see installed runtime names
aurelia config steam-runtime-runner GE-Proton9-20   # select one (or `experimental`, or a Wine path)
aurelia config steam-runtime-runner                 # verify — prints the bare Wine it resolves to
aurelia steam-runtime install                       # install Steam into the master prefix
aurelia config game <APP_ID> --steam-runtime on     # opt a game into using it at launch
```

The runner value is an **installed runtime name** (as shown by `aurelia proton list`) or an
**absolute path** to a Wine build. A Proton runtime is accepted — Aurelia uses the bare
Wine bundled inside it (`files/bin/wine64`) automatically, never `proton run`. If you have
no runtime yet, install one first with `aurelia proton install <NAME>`.

### Steam integration policy

A launch that asks for Steam integration ([`play --steam`](#play), forced on for
Family-Shared games) resolves **where** the Steam client comes from from a policy, checked
**per-game first, then the global default**:

| Policy | Behavior |
| --- | --- |
| `auto` (default) | Prefer the **host** Steam client when one is installed; otherwise fall back to the **in-Wine** Steam runtime. |
| `on` | **Always** use the in-Wine Steam runtime, even if a host Steam client exists. |
| `off` | **Host** Steam only; never the in-Wine runtime. |

```bash
aurelia config steam-runtime-policy on         # global default: always use the in-Wine runtime
aurelia config steam-runtime-policy            # view the current global default
aurelia config game <APP_ID> --steam-runtime on   # per-game override (wins over the global default)
```

A game whose own policy is `auto` inherits the global default; `on`/`off` on the game
override it. Installing the master prefix does not, by itself, route any game through it — a
launch only uses the in-Wine runtime when it both asks for Steam (`--steam`) and the resolved
policy selects it. When the policy selects the in-Wine runtime, Aurelia starts the master
Steam client in Wine (on the configured runner) and points the game at it via
`STEAM_COMPAT_CLIENT_INSTALL_PATH`, satisfying the Steamworks/DRM handshake without a host
Steam client. `--steam-prefix-mode shared|per-game` chooses whether the game runs in the
master prefix directly (`shared`, default) or gets its own copy (`per-game`).

```text
aurelia steam-runtime install [--json]
aurelia steam-runtime repair  [--json]
aurelia steam-runtime status  [--json]
aurelia config steam-runtime-runner [<NAME>]    # view/set the runner (empty string clears)
aurelia config steam-runtime-policy [auto|on|off]   # view/set the global default policy
```

| Subcommand | Description |
| --- | --- |
| `install` | Download `SteamSetup.exe` (if needed), install Steam into the master prefix, then start the Steam client. Waits for the installer and fails loudly if `steam.exe` does not appear. Requires `steam_runtime_runner` to be set. |
| `repair` | Stop Steam, back up the master prefix (keeping a single `.bak`), then reinstall — recovers a corrupted install that passes the file-exists check but crashes on start. Requires `steam_runtime_runner`. |
| `status` | Print the resolved master root, Wine prefix, layout kind, whether `steam.exe` is present, and whether a runtime runner is configured. |

```bash
aurelia steam-runtime status
aurelia steam-runtime install
aurelia steam-runtime repair
```

> **Runner:** the Windows-Steam installer and the background Steam client always run under a
> **bare wine**, never `proton run` (that wrapper derives its own prefix and expects the Steam
> Linux Runtime container). Pointing `steam_runtime_runner` at a Proton tree such as
> `GE-Proton9-20` is still fine — the wine bundled inside it (`files/bin/wine64`) is used
> automatically.

> **Diagnostics:** set `AURELIA_DIAGNOSE_INSTALL=1` before `install`/`repair` to run the
> installer with verbose `WINEDEBUG` and capture its output to a timestamped log under
> `~/.config/Aurelia/logs/` — useful for root-causing setupapi/file-copy failures. It has no
> effect on normal game launches.

---

## Luxtorpeda native-engine plugin (Linux only)

[Luxtorpeda](https://codeberg.org/luxtorpeda/luxtorpeda) is an **optional plugin** that runs
supported games on native Linux engines (GZDoom, OpenMW, devilutionX, …) instead of
Proton/Wine. It is a separate GPL-2.0 program and is **never bundled or linked into Aurelia**:
when you enable the feature and opt a game in, Aurelia downloads the client on the fly into
`~/.config/Aurelia/plugins/luxtorpeda` and invokes it over a process boundary, the same way
Steam invokes a compatibility tool. The binary therefore stays lean.

Routing is **explicit opt-in per game** — enabling the plugin never changes how an
un-pinned game launches.

```text
aurelia luxtorpeda enable | disable
aurelia luxtorpeda install | update
aurelia luxtorpeda path [<DIR>] [--clear]
aurelia luxtorpeda status [--json]
aurelia luxtorpeda uninstall [--json]
```

| Subcommand | Description |
| --- | --- |
| `enable` / `disable` | Master toggle (`luxtorpeda_enabled`). Off by default. |
| `install` / `update` | Download (or re-download) the latest luxtorpeda client. Refused when a custom path is set. |
| `path <DIR>` | Point Aurelia at an **existing** luxtorpeda install (a dir containing `toolmanifest.vdf`). This **disables the managed download** — that install is used as-is. Omit the dir to print the current value, or `--clear` to revert to the managed download. |
| `status` | Show enabled state, source (managed vs custom path), and installed version. |
| `uninstall` | Remove the **managed** payload from disk (never touches a custom-path install). |

### Custom install (skip the download)

If you already have luxtorpeda installed (e.g. in Steam's `compatibilitytools.d`), point
Aurelia at it instead of downloading a second copy. When a custom path is set, enabling the
plugin never prompts for or performs a download:

```bash
aurelia luxtorpeda path ~/.local/share/Steam/compatibilitytools.d/luxtorpeda
aurelia luxtorpeda enable        # "Using your configured install ... (no download)"
aurelia luxtorpeda path --clear  # revert to the on-the-fly managed download
```

The path is stored as `luxtorpeda_path` in the launcher config and validated on set (it
must contain a `toolmanifest.vdf`, directly or in a subdirectory).

Pin (or unpin) a game, or force a one-off launch:

```bash
aurelia luxtorpeda enable                      # off by default
aurelia config game 2270 --native-engine       # route this game through luxtorpeda
aurelia config game 2270 --no-native-engine    # back to normal native/Proton selection
aurelia play 2270 --native-engine              # one-off, regardless of per-game config
```

> **Note:** engines run outside Steam's runtime (Sniper) container, so they rely on host
> system libraries. If an engine fails to find a library for a given title, prefer Proton
> for that game. `--native-engine` conflicts with `--proton`/`--windows`.

---

## umu-launcher plugin (Linux only)

[umu-launcher](https://github.com/Open-Wine-Components/umu-launcher) is the unified launcher
that runs Windows games through Proton **outside** Steam, applying the same Steam Linux
Runtime and per-game protonfixes Steam would. Like [luxtorpeda](#luxtorpeda-native-engine-plugin-linux-only),
it is an **optional plugin** that is **never bundled**: when you enable it and opt a game in,
Aurelia downloads `umu-run` on the fly into `~/.config/Aurelia/plugins/umu` and wraps the
launch with it (setting `GAMEID` and `PROTONPATH` to the selected Proton). Unlike luxtorpeda,
umu **wraps Proton** rather than replacing it — the game still runs under the Proton build you
choose, just invoked via `umu-run`.

Routing is **explicit opt-in per game** — enabling the plugin never changes how an un-pinned
game launches.

```text
aurelia umu enable | disable
aurelia umu install | update
aurelia umu path [<DIR>] [--clear]
aurelia umu status [--json]
aurelia umu uninstall [--json]
```

| Subcommand | Description |
| --- | --- |
| `enable` / `disable` | Master toggle (`umu_enabled`). Off by default. |
| `install` / `update` | Download (or re-download) the latest `umu-run` from GitHub (`Open-Wine-Components/umu-launcher`). Refused when a custom path is set. |
| `path <DIR>` | Point Aurelia at an **existing** umu install (a directory containing `umu-run`, or the `umu-run` binary directly). This **disables the managed download**. Omit the dir to print the current value, or `--clear` to revert to the managed download. |
| `status` | Show enabled state, source (managed vs custom path), and installed version. |
| `uninstall` | Remove the **managed** payload from disk (never touches a custom-path install). |

Pin (or unpin) a game, or force a one-off launch:

```bash
aurelia umu enable                       # off by default
aurelia config game 1245620 --umu        # route this game through umu (Proton via umu-run)
aurelia config game 1245620 --no-umu     # back to normal native/Proton selection
aurelia play 1245620 --umu               # one-off, regardless of per-game config
aurelia play 1245620 --umu --proton GE-Proton9-20   # choose which Proton umu runs
```

The path is stored as `umu_path` in the launcher config. `--umu` is Linux-only and conflicts
with `--windows`/`--native-engine`; it **combines** with `--proton`, which selects the Proton
build umu runs the game through.

---

## Launch scripts

A **launch script** wraps the fully-resolved launch command with your own shell script. When a
script is active for a game, Aurelia runs the script as the program and passes the
previously-resolved program and its arguments to it as `"$@"`, so a script that is just
`exec "$@"` is a transparent passthrough, while a custom one can prepend `gamemoderun` /
`mangohud` / `gamescope` or launch its own way. This works uniformly for native, Proton
(WineTkg), luxtorpeda and umu launches, because it rewrites the final launch command just
before spawning.

**Script directory.** Scripts live in `AURELIA_SCRIPT_DIR` if that env var is set, otherwise
`~/.config/Aurelia/scripts`. A game's auto-detected script is `<script_dir>/<appid>.sh` on
unix (`<appid>.bat` on Windows). `aurelia scripts new` sets the unix executable bit (`0o755`).

**Exported environment.** Alongside the entire resolved launch environment (`WINEPREFIX`,
`WINEDLLOVERRIDES`, `STEAM_COMPAT_*`, …), Aurelia exports:

| Variable | Meaning |
| --- | --- |
| `AURELIA_APP_ID` | the Steam app id |
| `AURELIA_APP_NAME` | the game's display name |
| `AURELIA_GAME_DIR` | the game's install directory (when known) |
| `AURELIA_LAUNCH_PROGRAM` | the resolved program that would have run |
| `AURELIA_LAUNCH_ARGS` | its arguments, space-joined |

**Resolution precedence** (first match wins; `play --no-script` bypasses all):

1. `aurelia play <id> --script <PATH>` — one-off override for a single launch.
2. `aurelia config game <id> --launch-script <PATH>` — the per-game pinned script
   (stored as `launch_script` in the launcher config; clear with `--no-launch-script`).
3. the auto-detected `<script_dir>/<appid>.sh` (or `.bat`), when it exists on disk.

An explicit `--script` / `--launch-script` path that does not exist fails the launch with a
validation error rather than silently falling back.

```text
aurelia scripts dir                     # print the resolved script directory
aurelia scripts list [--json]           # app ids with a script + their resolved paths
aurelia scripts new <app_id> [--force]  # scaffold <script_dir>/<app_id>.sh (or .bat)
aurelia scripts show <app_id>           # print the resolved script path + contents
aurelia scripts remove <app_id>         # delete the dir-based script
```

| Subcommand | Description |
| --- | --- |
| `dir` | Print the resolved launch-script directory. |
| `list` | List app ids that have a script (dir-based **and** config-pinned) with resolved paths and, when known, the game name. |
| `new <app_id>` | Create the script directory if needed and write a documented template to the platform path. Errors if the file exists unless `--force`; on unix the file is made executable. |
| `show <app_id>` | Print the resolved script path and its contents. |
| `remove <app_id>` | Delete the dir-based script (reports when none exists). |

Typical use:

```bash
aurelia scripts new 2270                 # scaffold ~/.config/Aurelia/scripts/2270.sh
# edit it, e.g.:  exec gamemoderun mangohud "$@"
aurelia play 2270                        # launches through the script

aurelia config game 2270 --launch-script ~/wrappers/mygame.sh   # pin a specific script
aurelia play 2270 --script ~/other.sh    # one-off override
aurelia play 2270 --no-script            # bypass all scripts for this launch
```

Every `scripts` subcommand honors the global `--json` flag.

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
| `info_cache/` | Cached `info` metadata (`<APP_ID>.<LANGUAGE>.json`); TTL via `AURELIA_INFO_CACHE_TTL` (default 6h). |
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
