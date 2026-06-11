# Aurelia ↔ Heroic compatibility

**Goal:** let Heroic drive Steam through a bundled **Aurelia** CLI, slotting Aurelia in as a
managed *runner* exactly like Legendary (Epic), Nile (Amazon) and gogdl/comet (GOG) — and
retire Heroic's current Steam backend, which parses Steam's on-disk files
(`appmanifest_*.acf`, `licensecache`, binary `appinfo.vdf`) and delegates to the installed
Steam client via `steam://` URLs, with a separate OpenID web login.

This doc catalogues, against how Heroic's real runners work, what Aurelia must provide before
the swap. **No code changes here — it's the spec/gap list.**

> Paths like `src/backend/...` and `meta/...` are in the **Heroic** repo
> (`C:\DevelopmentTools\VSCodeProjects\HeroicGamesLauncher-1`). `src/main.rs`, `USAGE.md`,
> etc. are in this **Aurelia** repo.

---

## How Heroic runners work (the contract Aurelia must fit)

Heroic hides each store's CLI behind the `LibraryManager` / `GameManager` interfaces
(`src/common/types/game_manager.ts`) and treats the binary as interchangeable. The mechanics,
read from Legendary and Nile:

1. **Managed binary.** Each runner is one self-contained executable bundled per-OS/arch at
   `public/bin/<arch>/<platform>/<runner>[.exe]` (`archSpecificBinary()` in
   `src/backend/utils.ts` → `join(publicDir,'bin','x64'|'arm64',process.platform,name)`).
   `meta/downloadHelperBinaries.ts` fetches them from **pinned GitHub release tags**
   (`RELEASE_TAGS`), with assets named `<runner>_<platform>_<arch>` (e.g.
   `nile_linux_x86_64`, `legendary_windows_x86_64.exe`). A user can override the bundled
   binary with the `alt<Runner>Bin` global setting (`getNileBin`/`getLegendaryBin`).

2. **Invocation.** `runRunnerCommand(commandParts: string[], options)` per manager →
   `callRunner(...)` (`src/backend/launcher.ts`) spawns the binary with an **array of args**
   and the runner's `{dir,bin}`. Examples (Nile): `['install','--info','--json',appName]`,
   `['list-updates','--json']`, `['library','sync']`. Identical concurrent commands are
   de-duplicated; each gets an `abortId` + `AbortController` for cancellation. Per-runner
   config is isolated via env (Nile sets `NILE_CONFIG_PATH`) so Heroic never touches the
   user's own runner config.

3. **Two output modes:**
   - **Data commands** pass `--json`; Heroic does `JSON.parse(stdout)` (library list, update
     list, install-info/size, …).
   - **Long-running commands (install/update)** stream **human-readable progress lines** on
     stdout/stderr. `options.onOutput(data, child)` feeds each runner's
     `onInstallOrUpdateOutput`, which **regex-parses** the lines into an `InstallProgress`
     and calls `sendProgressUpdate` to drive the download-manager UI. Nile's lines, for
     reference: `Progress: 45.3 ...`, `ETA: 00:12:34`, `Downloaded: 1234 MiB`,
     `Download\t- 50 MiB`, `Disk\t- 40 MiB`.

4. **Progress shape Heroic renders** (`InstallProgress`, `src/common/types.ts`):
   `{ percent?: number, bytes: string, eta: string, downSpeed?: number, diskSpeed?: number,
   folder?: string, file?: string }`. The bar only advances once it can fill these.

5. **Return value.** `callRunner` resolves `ExecResult { stdout, stderr, error?, abort? }`;
   failure is inferred from exit code / stderr.

6. **Version diagnostic.** `<runner> --version` → raw stdout (`src/backend/utils/helperBinaries/index.ts`).

### What this means for Aurelia

The **data path already fits**: Aurelia speaks array-style subcommands and global `--json`.
The one structural mismatch is **progress**. Heroic's existing runners emit regex-parsed
text; Aurelia emits NDJSON `{event:"progress",...}`. That's *better*, but it means:

- The Steam runner needs its **own** `onInstallOrUpdateOutput` that parses Aurelia's NDJSON
  (not the legendary/nile regexes), and
- Aurelia must expose, in that stream, the fields the bar needs: **percent, bytes
  done/total, ETA, download speed, disk speed** — for *every* long-running op (install,
  update, verify, move), not just install.

---

## Already covered by today's Aurelia CLI

From the updated `README.md` / `src/main.rs`, these exist and map cleanly to runner commands
(all support global `--json`; errors → `{ "error": "..." }`):

`login`, `logout`, `account`, `list` (`--installed/--search/--online`), `info <id>`
(`--extended`), `dlc <id>`, `image <id>`, `install <id> -p <platform>` (streams NDJSON
`progress`), `update <id>`, `verify <id>`, `uninstall <id> [--delete-prefix]`,
`move <id> <library> [--restart-steam]`, `play <id> [-w|-p <ver>]` (blocks),
`stop [<id>]`, `enable/disable <dlc_id>`, `branches`, `set-branch`, `depots`,
`config show|protons`.

These satisfy: library refresh (`list`), DLC list/state (`dlc`), metadata
(`info --extended`), install-with-progress, uninstall, update, verify, move, launch, stop,
DLC enable/disable, and basic auth.

---

## Gaps to close before the swap (→ Aurelia issues)

1. **Streaming progress for `update`, `verify`, and `move` — ✅ done.** All four ops
   (`install`/`update`/`verify`/`move`) already stream the NDJSON `progress` event through
   `drive_progress`. The events now also carry **`speed_bps`** and **`eta_seconds`** (computed
   from the byte-rate between ticks), so Heroic's `InstallProgress {percent, bytes, eta,
   downSpeed, diskSpeed}` can be fully populated. *Heroic call sites:* `update`, `repair`
   (verify), `moveInstall` in `src/backend/storeManagers/steam/games.ts` — each needs a
   Steam `onInstallOrUpdateOutput` that maps Aurelia's NDJSON fields onto `InstallProgress`.

2. **Non-interactive (JSON) Steam Guard / QR login — ✅ done.** `aurelia login --json` is now
   a machine-drivable handshake (no TTY prompts). Credentials come from `-u`/`-p` (or
   `AURELIA_PASSWORD`); the protocol over stdout/stdin is:
   - `login --json` → on a Guard challenge emits `{event:"guard_required",type:"email"|"device"}`,
     then reads the code as **one line on stdin** and retries. Mobile-app accounts emit
     `{event:"guard_required",type:"device_confirmation"}`.
   - `login --qr --json` → streams `{event:"qr_challenge",url}` (re-emitted on rotation); the
     driver renders the QR and waits for the result.
   - Both finish with `{logged_in:true,account}` or `{error}`.

   *Heroic side (the swap):* drive this from `SteamUser.login` + the login form — spawn
   `aurelia login --json …`, parse the NDJSON, push the code to the child's stdin (or render
   the QR), await the result line. *Known limitation:* a mobile-app-approval account blocks on
   the first attempt without a pre-event (the approval handler waits); code-based accounts get
   the full event/stdin round-trip.

3. **Pre-install size estimate.** Heroic's install dialog / `getInstallInfo` needs download
   and on-disk size *before* installing — cf. Nile's `['install','--info','--json',appName]`
   → `{ download_size, disk_size }`. Aurelia exposes neither. *Fix:* `install --dry-run
   --json` (or `download_size`/`disk_size` in `info --json`). *Call site:*
   `SteamLibraryManager.getInstallInfo`.

4. **Steam Cloud save sync.** Aurelia has a `cloud_sync` module but no CLI surface, so
   Heroic's `syncSaves` stays a no-op. *Fix:* `aurelia cloud sync <id> [--up|--down] --json`.
   *Call site:* `SteamGameManager.syncSaves`.

5. **Launch-options list.** `getLaunchOptions` has no Aurelia source (Steam's per-app launch
   configs). *Fix:* `aurelia launch-options <id> --json`. *Call site:*
   `SteamLibraryManager.getLaunchOptions`.

6. **Art URLs from `info`.** `info` returns no cover/hero/logo URLs (only `image` downloads
   bytes); Heroic otherwise guesses CDN URLs. *Fix:* include `header`/`background`/`logo`
   asset URLs in `info --json` (the StoreBrowse `StoreItem.assets` block carries these
   natively). *Call site:* `getExtraInfo` / `steamToUnifiedInfo`.

7. **`--version` for diagnostics — ✅ already satisfied.** `aurelia --version` exists (clap
   `version` attribute) and prints `aurelia 0.1.0`. Heroic's getter can return stdout like
   Nile's (or `stdout.split(' ')[1]` for the bare version). No Aurelia change needed; the
   Heroic side just needs a `getAureliaVersion()` once the runner is wired.

8. **Release binaries for bundling — handled by the maintainer.** Producing tagged GitHub
   releases with per-arch/platform assets (named `aurelia_<os>_<arch>`, `.exe` on Windows) is
   the maintainer's responsibility — the process is documented in
   [RELEASE.md](RELEASE.md). The remaining *Heroic-side* wiring (add to `RELEASE_TAGS`, a
   `downloadAurelia()`, and `archSpecificBinary('aurelia')`) is part of "The swap" below.
   *Interim for development:* point the `altAureliaBin` setting at a locally built
   `target/release/aurelia`.

Smaller follow-ups surfaced by the interfaces: `importGame` (adopt an existing on-disk
install), `isGameAvailable`, and `changeInstallPath` (distinct from `move` — relink without
copying) currently have no Aurelia command.

---

## The swap (deferred — only after the gaps above ship)

1. **Wire Aurelia as a runner.** Add `altAureliaBin` to `src/common/types.ts`, a
   `getAureliaBin()` in `src/backend/utils.ts` (mirroring `getNileBin`, with
   `archSpecificBinary('aurelia')`), and a `runRunnerCommand` on `SteamLibraryManager`/
   `SteamGameManager` that calls the shared `callRunner` with array-style `commandParts`,
   `--json` for data and `onOutput` for progress.
2. **Rewrite the managers to shell out:** `library.ts` (`list`/`dlc`), `games.ts`
   (`install`/`update`/`verify`/`uninstall`/`move`/`play`/`stop`/`getExtraInfo`),
   `user.ts` + the frontend login form (`login`/`logout`/`account`), plus a Steam-specific
   `onInstallOrUpdateOutput` that parses Aurelia's NDJSON into `InstallProgress`.
3. **Delete the now-dead local-file machinery:** `vdf.ts`, `xor.ts`, `steammessages.*`,
   `downloadProgress.ts`, the manifest/license-cache parsing, and the `steam://` constants.
4. **Bundle the binary** via `meta/downloadHelperBinaries.ts` (gap 8).

---

## Verification

- This file is spec only — no build/test impact.
- Before filing each gap, re-check the current Aurelia `README.md` / `USAGE.md` /
  `src/main.rs` to confirm the command really doesn't exist yet.
- Cross-check the "already covered" list against the live `Command` enum in `src/main.rs`.
