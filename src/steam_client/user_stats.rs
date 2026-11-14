//! Stats and achievements for games.
//!
//! Reads `ClientGetUserStats`, writes through
//! `ClientStoreUserStats2`.

use super::*;
use steam_vent_proto::steammessages_clientserver_userstats::{
    CMsgClientStoreUserStats2, CMsgClientStoreUserStatsResponse, cmsg_client_store_user_stats2,
};

/// Stat type codes in the schema.
pub mod stat_kind {
    pub const INT: u8 = 1;
    pub const FLOAT: u8 = 2;
    pub const AVG_RATE: u8 = 3;
    pub const ACHIEVEMENTS: u8 = 4;
    pub const GROUP_ACHIEVEMENTS: u8 = 5;
}

/// One stat with its value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stat {
    pub id: u32,
    pub name: String,
    pub kind: u8,
    /// Raw bits; floats as `f32`.
    pub value: u32,
}

/// One achievement with its state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Achievement {
    /// Bitfield stat holding it.
    pub block: u32,
    pub bit: u8,
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub hidden: bool,
    /// Zero while locked.
    pub unlock_time: u32,
}

/// A user's stats for one game.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UserStats {
    pub eresult: i32,
    pub crc: u32,
    pub stats: Vec<Stat>,
    pub achievements: Vec<Achievement>,
}

/// What Steam said to a store.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StoreOutcome {
    pub eresult: i32,
    pub crc: u32,
    /// Rejected stats and reverted values.
    pub reverted: Vec<(u32, u32)>,
}

fn int_of(value: &steam_vdf_parser::Value) -> Option<i64> {
    use steam_vdf_parser::Value;
    match value {
        Value::I32(n) => Some(i64::from(*n)),
        Value::U64(n) => i64::try_from(*n).ok(),
        Value::Pointer(n) => Some(i64::from(*n)),
        Value::Float(f) => Some(*f as i64),
        Value::Str(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn float_of(value: &steam_vdf_parser::Value) -> Option<f32> {
    use steam_vdf_parser::Value;
    match value {
        Value::Float(f) => Some(*f),
        Value::Str(s) => s.trim().parse().ok(),
        other => int_of(other).map(|n| n as f32),
    }
}

/// Localized text, else English, else any.
fn localized(value: Option<&steam_vdf_parser::Value>, language: &str) -> String {
    let Some(value) = value else {
        return String::new();
    };
    if let Some(text) = value.as_str() {
        return text.to_string();
    }
    let Some(obj) = value.as_obj() else {
        return String::new();
    };
    [language, "english"]
        .iter()
        .find_map(|l| obj.get(l).and_then(|v| v.as_str()))
        .or_else(|| {
            obj.iter()
                .find(|(k, _)| *k != "token")
                .and_then(|(_, v)| v.as_str())
        })
        .unwrap_or_default()
        .to_string()
}

/// Schema stats and achievement definitions.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Schema {
    /// Value stats with default bits.
    pub stats: Vec<Stat>,
    /// Achievements, all locked.
    pub achievements: Vec<Achievement>,
}

/// Parses an already-decoded schema tree.
pub fn schema_from_vdf(root: &steam_vdf_parser::Value, language: &str) -> Schema {
    let mut schema = Schema::default();
    let Some(root) = root.as_obj() else {
        return schema;
    };
    let stats = root.get("stats").and_then(|v| v.as_obj()).or_else(|| {
        root.values()
            .find_map(|v| v.as_obj().and_then(|o| o.get("stats")))
            .and_then(|v| v.as_obj())
    });
    let Some(stats) = stats else {
        return schema;
    };
    for (key, node) in stats.iter() {
        let (Ok(id), Some(node)) = (key.parse::<u32>(), node.as_obj()) else {
            continue;
        };
        let kind = node
            .get("type_int")
            .or_else(|| node.get("type"))
            .and_then(int_of)
            .and_then(|k| u8::try_from(k).ok())
            .unwrap_or(0);
        let name = node
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        match kind {
            stat_kind::INT => {
                let default = node.get("default").and_then(int_of).unwrap_or(0);
                schema.stats.push(Stat {
                    id,
                    name: name.to_string(),
                    kind,
                    value: default as i32 as u32,
                });
            }
            stat_kind::FLOAT | stat_kind::AVG_RATE => {
                let default = node.get("default").and_then(float_of).unwrap_or(0.0);
                schema.stats.push(Stat {
                    id,
                    name: name.to_string(),
                    kind,
                    value: default.to_bits(),
                });
            }
            _ => {}
        }
        let Some(bits) = node.get("bits").and_then(|v| v.as_obj()) else {
            continue;
        };
        for (bit_key, bit_node) in bits.iter() {
            let Some(bit_node) = bit_node.as_obj() else {
                continue;
            };
            let bit = bit_node
                .get("bit")
                .and_then(int_of)
                .or_else(|| bit_key.parse().ok())
                .and_then(|b| u8::try_from(b).ok())
                .filter(|b| *b < 32);
            let name = bit_node.get("name").and_then(|v| v.as_str());
            let (Some(bit), Some(name)) = (bit, name) else {
                continue;
            };
            let display = bit_node.get("display").and_then(|v| v.as_obj());
            let field = |key: &str| display.and_then(|d| d.get(key));
            schema.achievements.push(Achievement {
                block: id,
                bit,
                name: name.to_string(),
                display_name: localized(field("name"), language),
                description: localized(field("desc"), language),
                hidden: field("hidden").and_then(int_of).is_some_and(|h| h != 0),
                unlock_time: 0,
            });
        }
    }
    schema.stats.sort_by_key(|s| s.id);
    schema.achievements.sort_by_key(|a| (a.block, a.bit));
    schema
}

/// Binary or text schema blob.
pub fn parse_schema(bytes: &[u8], language: &str) -> Schema {
    if bytes.is_empty() {
        return Schema::default();
    }
    // Deep blobs: parse on big stack.
    let bytes = bytes.to_vec();
    let language = language.to_string();
    std::thread::Builder::new()
        .name("stats-schema-parse".to_string())
        .stack_size(16 * 1024 * 1024)
        .spawn(move || match find_vdf_in_pics(&bytes) {
            Ok(vdf) => schema_from_vdf(vdf.value(), &language),
            Err(_) => Schema::default(),
        })
        .ok()
        .and_then(|h| h.join().ok())
        .unwrap_or_default()
}

/// Fills schema defaults with Steam's values.
pub fn merge_values(
    schema: Schema,
    values: &HashMap<u32, u32>,
    unlocks: &HashMap<u32, Vec<u32>>,
) -> (Vec<Stat>, Vec<Achievement>) {
    let stats = schema
        .stats
        .into_iter()
        .map(|mut s| {
            if let Some(v) = values.get(&s.id) {
                s.value = *v;
            }
            s
        })
        .collect();
    let achievements = schema
        .achievements
        .into_iter()
        .map(|mut a| {
            a.unlock_time = unlocks
                .get(&a.block)
                .and_then(|times| times.get(usize::from(a.bit)))
                .copied()
                .unwrap_or(0);
            a
        })
        .collect();
    (stats, achievements)
}

impl SteamClient {
    /// Stats of `steam_id`, else ours.
    pub async fn user_stats(
        &self,
        appid: u32,
        steam_id: Option<u64>,
        language: &str,
    ) -> Result<UserStats> {
        let connection = self.require_connection()?;
        let own = self.steam_id().context("not logged in")?;
        let mut request = CMsgClientGetUserStats::new();
        request.set_game_id(u64::from(appid));
        request.set_schema_local_version(-1);
        request.set_crc_stats(0);
        request.set_steam_id_for_user(steam_id.unwrap_or(own));
        let response: CMsgClientGetUserStatsResponse = connection
            .job(request)
            .await
            .context("ClientGetUserStats failed")?;
        let schema = parse_schema(response.schema(), language);
        let values: HashMap<u32, u32> = response
            .stats
            .iter()
            .map(|s| (s.stat_id(), s.stat_value()))
            .collect();
        let unlocks: HashMap<u32, Vec<u32>> = response
            .achievement_blocks
            .iter()
            .map(|b| (b.achievement_id(), b.unlock_time.clone()))
            .collect();
        let (stats, achievements) = merge_values(schema, &values, &unlocks);
        Ok(UserStats {
            eresult: response.eresult(),
            crc: response.crc_stats(),
            stats,
            achievements,
        })
    }

    /// Stores changed stats for us.
    pub async fn store_user_stats(
        &self,
        appid: u32,
        crc: u32,
        reset: bool,
        changes: &[(u32, u32)],
    ) -> Result<StoreOutcome> {
        let connection = self.require_connection()?;
        let own = self.steam_id().context("not logged in")?;
        let mut request = CMsgClientStoreUserStats2::new();
        request.set_game_id(u64::from(appid));
        request.set_settor_steam_id(own);
        request.set_settee_steam_id(own);
        request.set_crc_stats(crc);
        request.set_explicit_reset(reset);
        request.stats = changes
            .iter()
            .map(|&(id, value)| {
                let mut stat = cmsg_client_store_user_stats2::Stats::new();
                stat.set_stat_id(id);
                stat.set_stat_value(value);
                stat
            })
            .collect();
        let response: CMsgClientStoreUserStatsResponse = connection
            .job(request)
            .await
            .context("ClientStoreUserStats2 failed")?;
        Ok(StoreOutcome {
            eresult: response.eresult(),
            crc: response.crc_stats(),
            reverted: response
                .stats_failed_validation
                .iter()
                .map(|s| (s.stat_id(), s.reverted_stat_value()))
                .collect(),
        })
    }
}

#[cfg(test)]
#[path = "user_stats_tests.rs"]
mod tests;
