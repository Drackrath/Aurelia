//! Data-driven, Rust-native per-game fixup registry.
//!
//! Aurelia deliberately does NOT embed a scripting engine (e.g. `rhai`) for
//! per-game workarounds — the project keeps strict binary-size discipline. Instead
//! fixups are a static, const table keyed by Steam `app_id`. Adding a game is a
//! one-line data edit in [`FIXUPS`]; no code changes, no runtime download, no extra
//! dependency.
//!
//! A fixup carries two fragments that the launch pipeline merges into the game's
//! environment at launch time (see the `wine_tkg` runner's `build_env`):
//!   * `env` — extra environment variables (e.g. `PROTON_NO_ESYNC=1`).
//!   * `dll_overrides` — `WINEDLLOVERRIDES` entries as `(dll_name, mode)` pairs,
//!     where `mode` is Wine's override syntax such as `"native,builtin"` / `"n,b"`
//!     / `"builtin"`.
//!
//! Merge policy (enforced by the consumer): explicit user / per-game settings ALWAYS
//! win over these auto-fixups on conflict.

/// Fixups resolved for a single game, ready to be merged into the launch env.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GameFixups {
    /// Extra environment variables `(key, value)`.
    pub env: Vec<(String, String)>,
    /// `WINEDLLOVERRIDES` fragments `(dll_name, mode)`, e.g. `("d3d11", "native,builtin")`.
    pub dll_overrides: Vec<(String, String)>,
}

impl GameFixups {
    /// Whether this fixup set carries anything to merge.
    pub fn is_empty(&self) -> bool {
        self.env.is_empty() && self.dll_overrides.is_empty()
    }
}

/// A single static registry row. Kept as borrowed `&'static str` slices so the whole
/// table lives in read-only data with zero startup cost.
struct FixupEntry {
    app_id: u32,
    env: &'static [(&'static str, &'static str)],
    dll_overrides: &'static [(&'static str, &'static str)],
}

/// The fixup registry.
///
/// To add a game, append one `FixupEntry { .. }` row here — that is the entire
/// change required. The seed entries below are illustrative, well-known-safe
/// workarounds; extend as needed.
const FIXUPS: &[FixupEntry] = &[
    // Dark Souls: Prepare to Die Edition (App 211420) — the ancient GFWL-era build is
    // widely run with esync disabled to avoid input/audio stalls under Wine.
    FixupEntry {
        app_id: 211420,
        env: &[("PROTON_NO_ESYNC", "1")],
        dll_overrides: &[],
    },
    // Fallout 3: GOTY (App 22370) — Games-for-Windows-Live shim; xlive is forced to
    // builtin so the game can start without the GFWL client.
    FixupEntry {
        app_id: 22370,
        env: &[],
        dll_overrides: &[("xlive", "builtin")],
    },
];

/// Look up the fixups for `app_id`. Returns an empty [`GameFixups`] when the game
/// has no registry entry (the common case).
pub fn game_fixups(app_id: u32) -> GameFixups {
    match FIXUPS.iter().find(|e| e.app_id == app_id) {
        Some(entry) => GameFixups {
            env: entry
                .env
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            dll_overrides: entry
                .dll_overrides
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        },
        None => GameFixups::default(),
    }
}

#[cfg(test)]
#[path = "fixups_tests.rs"]
mod tests;
