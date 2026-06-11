# Releasing Aurelia

How to cut a release and publish per-platform binaries — both for end users and so Heroic
can bundle Aurelia as a managed runner.

Builds are produced **from this repository**, so the patched `steam-vent` (git) and vendored
`steam-cdn` resolve correctly. You do **not** publish to crates.io (the patched deps make
that impossible); distribution is via tagged GitHub releases with attached binaries.

---

## 1. Bump the version

Update `version` in [Cargo.toml](Cargo.toml), commit, and tag:

```bash
# edit Cargo.toml: version = "0.1.1"
cargo build --release            # refresh Cargo.lock
git commit -am "Release v0.1.1"
git tag -a v0.1.1 -m "Aurelia v0.1.1"
git push origin main --tags
```

Use semantic versions and a `v`-prefixed tag (`v0.1.1`) — that tag is what Heroic's
`RELEASE_TAGS` will pin to.

---

## 2. Build the binaries

Aurelia targets **Linux first, Windows too** (macOS optional). Build each target in release
mode. The simplest reliable path is to build natively on each OS in CI; cross-compiling from
one host also works with the right toolchains.

| OS / arch | Rust target | Build command |
| --- | --- | --- |
| Linux x86-64 | `x86_64-unknown-linux-gnu` | `cargo build --release --target x86_64-unknown-linux-gnu` |
| Linux ARM64 | `aarch64-unknown-linux-gnu` | `cargo build --release --target aarch64-unknown-linux-gnu` |
| Windows x86-64 | `x86_64-pc-windows-msvc` | `cargo build --release --target x86_64-pc-windows-msvc` |
| Windows ARM64 | `aarch64-pc-windows-msvc` | `cargo build --release --target aarch64-pc-windows-msvc` |
| macOS x86-64 *(opt.)* | `x86_64-apple-darwin` | `cargo build --release --target x86_64-apple-darwin` |
| macOS ARM64 *(opt.)* | `aarch64-apple-darwin` | `cargo build --release --target aarch64-apple-darwin` |

The binary lands at `target/<triple>/release/aurelia` (`aurelia.exe` on Windows). Install a
target first with `rustup target add <triple>`; cross-compiling Linux needs the matching
linker (e.g. `gcc-aarch64-linux-gnu`).

---

## 3. Name the assets (Heroic convention)

Heroic downloads runner binaries from GitHub releases and expects a specific asset name per
OS/arch (see `meta/downloadHelperBinaries.ts` in the Heroic repo — the pattern is
`<runner>_<os>_<arch>`, with `.exe` on Windows). Rename each built binary to match, so Heroic
can fetch it unmodified:

| OS / arch | Release asset filename |
| --- | --- |
| Linux x86-64 | `aurelia_linux_x86_64` |
| Linux ARM64 | `aurelia_linux_arm64` |
| Windows x86-64 | `aurelia_windows_x86_64.exe` |
| Windows ARM64 | `aurelia_windows_arm64.exe` |
| macOS x86-64 | `aurelia_macOS_x86_64` |
| macOS ARM64 | `aurelia_macOS_arm64` |

```bash
# example, Linux x86-64
cp target/x86_64-unknown-linux-gnu/release/aurelia aurelia_linux_x86_64
chmod +x aurelia_linux_x86_64
```

> Keep the OS token exactly as `linux` / `windows` / `macOS` and the arch token as `x86_64` /
> `arm64` — Heroic matches these strings literally.

---

## 4. Create the GitHub release

With the [`gh`](https://cli.github.com/) CLI:

```bash
gh release create v0.1.1 \
  --title "Aurelia v0.1.1" \
  --notes "..." \
  aurelia_linux_x86_64 \
  aurelia_linux_arm64 \
  aurelia_windows_x86_64.exe \
  aurelia_windows_arm64.exe
```

Or upload the files manually under **Releases → Draft a new release** on GitHub.

---

## 5. (Optional) Other artifacts

- **Debian package** — `cargo deb` uses the `[package.metadata.deb]` block already in
  `Cargo.toml` and emits a `.deb` under `target/debian/`.
- **Direct source install** — users with a Rust toolchain can skip releases entirely:
  `cargo install --git https://github.com/Drackrath/Aurelia.git --tag v0.1.1 --locked`.

---

## 6. Wire it into Heroic (one-time, in the Heroic repo)

Once a tagged release with the assets above exists, the Heroic side needs (handled there, not
here):

1. Add the tag to `RELEASE_TAGS` and a `downloadAurelia()` in `meta/downloadHelperBinaries.ts`
   using the asset names from §3.
2. Map `archSpecificBinary('aurelia')` and add `getAureliaBin()` / `altAureliaBin`
   (`src/backend/utils.ts`, `src/common/types.ts`).

See [Heroic Compability.md](Heroic%20Compability.md) → "The swap" for the full integration.

---

## Release checklist

- [ ] `version` bumped in `Cargo.toml`; `Cargo.lock` refreshed and committed
- [ ] `cargo build --release` clean; `cargo test` green
- [ ] Binaries built for each target
- [ ] Assets renamed to the Heroic convention (§3)
- [ ] `git tag vX.Y.Z` pushed
- [ ] GitHub release created with all assets attached
- [ ] (If bundling) Heroic `RELEASE_TAGS` / `downloadAurelia()` updated to the new tag
