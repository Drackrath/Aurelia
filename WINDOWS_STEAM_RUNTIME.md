# Windows Steam Runtime — Setup Guide

Aurelia can host a self-contained **Windows Steam client inside Wine** (a "master
prefix") purely to satisfy the Steamworks/DRM handshake that some Windows games need,
without a host Steam client. The game itself is still launched **directly** by Aurelia
(never through a `steam://run` handoff).

This guide is the exact, tested setup. For the command reference, see the
[Windows Steam runtime section in USAGE.md](USAGE.md#windows-steam-runtime).

> **Have host Steam already?** You probably don't need this. Set
> `aurelia config steam-runtime-policy off` and use `aurelia play <APPID> --steam` — it
> bridges to your host Steam client and needs none of the setup below. The in-Wine
> runtime is for machines **without** a host Steam client.

---

## The recipe

```bash
# 1. One-time: a Wine/Proton runner that bundles DXVK + vkd3d (GE-Proton does).
aurelia proton install GE-Proton9-20          # skip if already installed
aurelia config steam-runtime-runner GE-Proton9-20

# 2. Install the Windows Steam into the master prefix.
aurelia steam-runtime install

# 3. Sign in to the in-Wine Steam (a window appears — sign in, incl. Steam Guard).
aurelia steam-runtime login
aurelia steam-runtime stop                    # optional: close it once signed in

# 4. Route games through the runtime.
aurelia config steam-runtime-policy on        # global default …
#   … or per game:  aurelia config game <APPID> --steam-runtime on

# 5. Launch a WINDOWS game (native-Linux games bypass the runtime entirely).
aurelia play <WINDOWS_APPID>
```

Steps 1–3 are one-time. After that, launching is just step 5. The in-Wine Steam keeps
its own login in the master prefix and is started silently in the background for each
launch that needs it (re-run `steam-runtime login` if that session ever expires).

---

## What the machine actually needs

1. **Working Vulkan.** The whole thing hinges on it: Steam's CEF UI (the login window
   and client) renders through DXVK → Vulkan. Confirm with:
   ```bash
   vulkaninfo --summary        # must list your GPU
   ```
   A bare-Wine prefix on `wined3d` alone crashes Chromium's GPU process — Aurelia works
   around this by giving the prefix DXVK (see below), which requires Vulkan.

2. **A DXVK + vkd3d-bundling runner** as `steam_runtime_runner` — GE-Proton, or any
   Proton / wine-tkg tree containing `files/…/dxvk` and `files/…/vkd3d`. Aurelia copies
   those DLLs into the master prefix automatically on install/login/launch
   (`ensure_steam_runtime_prefix_libs`). A plain upstream Wine without them will not
   render the Steam UI.

3. **A display** — X11, or Wayland with XWayland. Both are fine.

Everything else that used to break the in-Wine Steam is handled automatically now:

| Handled for you | Why it mattered |
| --- | --- |
| DXVK/`vkd3d` copied into the prefix | `dxgi → wined3d → libvkd3d` was missing under bare Wine → Chromium GPU process crash-loop → "please reinstall". |
| `lsteamclient` disabled for the client | GE-Proton's game-bridge `lsteamclient` hijacked the client's `CLIENTENGINE` interface → `InternalAPI_Init` assert. |
| GUI commands run locally, not via the daemon | A daemon-spawned Steam had a stale session env and the window never appeared. |
| `XAUTHORITY` forwarded to the Wine launch | Cookie-auth X servers otherwise rejected Wine's window → invisible client. |
| Startup log redirected off the terminal | Wine `fixme:` / Steam bootstrapper spam. Capture it with `AURELIA_DIAGNOSE_INSTALL=1`. |

---

## Caveats that will bite people

- **Native-Linux games never use the in-Wine runtime.** The policy has no effect on
  them — they run natively regardless. Test the runtime with a **Windows-only** title.
- The account you sign the in-Wine Steam into must **own / have access** to the game,
  or the DRM handshake fails.
- First `install`/`login` **self-updates Steam** and takes a couple of minutes — normal.
- The in-Wine login is **separate** from `aurelia login` (which is Aurelia's own
  session for the library/downloads). See [Runtime authentication](USAGE.md#runtime-authentication).

---

## Games installed *through* the in-Wine Steam (incl. Family-Shared)

Some titles can only be installed by a real Steam client — most notably **Family-Shared**
games, which Aurelia can't download itself (they need the owner's authorisation). For
those, sign in to the in-Wine Steam (`aurelia steam-runtime login`) and install the game
from its window, into the in-Wine Steam's own library inside the master prefix.

Aurelia discovers those installs and flags them in `list` with a **`[wine]`** tag:

```
  2874130  installed   family-shared  -   Berserk or Die [wine]
```

`aurelia play <APPID>` handles them automatically: instead of the Proton pipeline, it
hands the game to the in-Wine Steam (`steam.exe -applaunch`), exactly as launching it
from that Steam's own library would — the only way its Steamworks/DRM handshake succeeds.
A one-line warning notes this, because:

- Aurelia's **Proton/DXVK per-game settings don't apply** — the in-Wine Steam runs it in
  the master prefix with that prefix's DXVK.
- Aurelia doesn't own the process, so **session tracking is best-effort** (it waits for the
  game to exit, then reports "Finished playing").

Games you install with `aurelia install <APPID>` land in Aurelia's own library instead,
run through the normal Proton pipeline, and carry no `[wine]` tag — the in-Wine runtime is
then only their DRM backend.

## Choosing where Steam comes from (`steam-runtime-policy`)

A launch that asks for Steam (`play --steam`, or forced for Family-Shared games)
resolves its Steam client from a policy — per-game first, then the global default:

| Policy | Behavior |
| --- | --- |
| `auto` (default) | Host Steam if installed, else fall back to the in-Wine runtime. |
| `on` | **Always** the in-Wine runtime, even if host Steam exists. |
| `off` | Host Steam only; never the in-Wine runtime. |

```bash
aurelia config steam-runtime-policy on            # global default
aurelia config game <APPID> --steam-runtime on    # per-game override (wins)
```

---

## Troubleshooting

Almost every failure is Vulkan or the runner. Triage:

```bash
vulkaninfo --summary                              # GPU present & Vulkan OK?

# Capture the in-Wine Steam's real output to a log instead of /dev/null:
AURELIA_DIAGNOSE_INSTALL=1 aurelia steam-runtime login

# Steam's own logs live in the master prefix:
SL="$HOME/.config/Aurelia/master_steam_prefix/pfx/drive_c/Program Files (x86)/Steam/logs"
grep -i "GPU process exited"  "$SL/cef_log.txt"       # GPU crash-loop?  → Vulkan/runner
grep -i "InternalAPI_Init"    "$SL"/../dumps/*.dmp     # client init assert
tail -n 20 "$SL/connection_log.txt"                   # is it reaching Steam?
```

- **`GPU process exited unexpectedly` repeatedly** — the prefix isn't getting DXVK, or
  Vulkan doesn't work. Confirm `vulkaninfo`, and that your runner bundles `dxvk`/`vkd3d`.
- **Client corrupt / "please reinstall"** — start clean:
  ```bash
  aurelia steam-runtime install --reinstall     # wipes the prefix, installs fresh
  ```
- **Steam won't render at all despite working Vulkan** — that machine's Wine can't run
  Chromium; use **host Steam** instead (`config steam-runtime-policy off`).

### Useful commands

```bash
aurelia steam-runtime status                  # resolved prefix, steam.exe present, runner
aurelia steam-runtime stop                    # kill the in-Wine Steam session
aurelia steam-runtime login                   # re-open to sign in / switch accounts
aurelia steam-runtime uninstall               # remove the master prefix entirely
```
