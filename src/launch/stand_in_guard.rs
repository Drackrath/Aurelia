//! Anti-cheat guard for stand-in launches.

use crate::compat::anticheat::AntiCheat;
use crate::core::config::{GameRunner, LauncherConfig};

/// Store category: Valve Anti-Cheat enabled.
pub const VAC_CATEGORY: u32 = 8;

/// Protection that rules out the stand-in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protection {
    Vac,
    EasyAntiCheat,
    BattlEye,
    /// Store categories could not be read.
    Unknown,
}

impl Protection {
    pub fn label(self) -> &'static str {
        match self {
            Protection::Vac => "VAC",
            Protection::EasyAntiCheat => "EasyAntiCheat",
            Protection::BattlEye => "BattlEye",
            Protection::Unknown => "unknown anti-cheat (store categories unreadable)",
        }
    }
}

/// Outcome of the guard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// No protection found.
    Clear,
    /// Protected, but allowed per game.
    Allowed(Vec<Protection>),
    /// Protected and not allowed.
    Refused(Vec<Protection>),
}

/// Protections from categories and install.
pub fn protections(category_ids: Option<&[u32]>, anticheats: &[AntiCheat]) -> Vec<Protection> {
    let mut found = Vec::new();
    match category_ids {
        Some(ids) if ids.contains(&VAC_CATEGORY) => found.push(Protection::Vac),
        Some(_) => {}
        None => found.push(Protection::Unknown),
    }
    for kind in anticheats {
        found.push(match kind {
            AntiCheat::EasyAntiCheat => Protection::EasyAntiCheat,
            AntiCheat::BattlEye => Protection::BattlEye,
        });
    }
    found
}

/// Decide whether the stand-in may run.
pub fn evaluate(app_id: u32, found: Vec<Protection>, config: &LauncherConfig) -> Verdict {
    if found.is_empty() {
        Verdict::Clear
    } else if config.steamworks_allowed_protected.contains(&app_id) {
        Verdict::Allowed(found)
    } else {
        Verdict::Refused(found)
    }
}

/// Whether this launch uses the stand-in.
pub fn stand_in_applies(config: &LauncherConfig, app_id: u32, force_native_engine: bool) -> bool {
    if !cfg!(target_os = "linux") || !config.steamworks_client || force_native_engine {
        return false;
    }
    let luxtorpeda = config.luxtorpeda_enabled
        && config
            .game_configs
            .get(&app_id)
            .is_some_and(|g| g.runner == GameRunner::Luxtorpeda);
    !luxtorpeda
}

fn labels(found: &[Protection]) -> String {
    found
        .iter()
        .map(|p| p.label())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Why protected games are risky.
pub const RISK_TEXT: &str = "VAC-secured servers reject sessions that do not come from \
    Valve's Steam client, so multiplayer will not work. Worse, EasyAntiCheat, BattlEye \
    and VAC run inside the game process, where they see a steamclient.so that is not \
    Valve's; anti-cheat may flag that and ban your real Steam account, even though \
    Aurelia's build is unmodified.";

/// Refusal shown by `play`.
pub fn refusal_message(app_id: u32, name: &str, found: &[Protection]) -> String {
    format!(
        "Refusing to launch {name} ({app_id}) with the stand-in steamclient.so: it uses \
         {}. {RISK_TEXT} Play it with Valve's Steam client, turn the stand-in off \
         (`aurelia config steamworks-client off`), or allow it for this game at your own \
         risk (`aurelia config steamworks-client --allow {app_id}`).",
        labels(found)
    )
}

/// Prompt for the per-game allow.
pub fn allow_prompt(app_id: u32) -> String {
    format!(
        "Allow the stand-in steamclient.so for app {app_id} even if it uses VAC, \
         EasyAntiCheat or BattlEye? {RISK_TEXT} Only single-player is expected to \
         work. Allow anyway? [y/N] "
    )
}

/// Warning for an allowed launch.
pub fn allowed_warning(app_id: u32, name: &str, found: &[Protection]) -> String {
    format!(
        "{name} ({app_id}) uses {} and is allowed with the stand-in steamclient.so. \
         Multiplayer needs Valve's Steam client; anti-cheat may ban the account.",
        labels(found)
    )
}
