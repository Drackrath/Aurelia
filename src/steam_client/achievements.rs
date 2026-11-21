//! Achievement schema parsing.

use super::*;

/// Build the full Steam Community CDN URL for an achievement icon hash. Returns
/// it unchanged if it's already a URL, and empty for an empty hash.
pub(super) fn achievement_icon_url(appid: u32, icon: &str) -> String {
    if icon.is_empty() {
        String::new()
    } else if icon.starts_with("http") {
        icon.to_string()
    } else {
        format!(
            "https://cdn.cloudflare.steamstatic.com/steamcommunity/public/images/apps/{appid}/{icon}"
        )
    }
}

/// Parse the binary-KV achievement schema from a `ClientGetUserStats` response
/// into a map of achievement api-name → `(stat_id, bit)`, so the user's unlock
/// blocks (keyed by stat id, indexed by bit) can be matched to each achievement.
///
/// Schema shape: `<appid> { stats { <statid> { type "4" bits { <bit> { name
/// "ACH_…" … } } } } }` — `type == 4` marks an achievement bitfield stat.
pub(super) fn parse_achievement_schema(schema: &[u8]) -> HashMap<String, (u32, u32)> {
    if schema.is_empty() {
        return HashMap::new();
    }
    // The binary-KV parser recurses one frame per nesting level; a malformed or
    // misaligned blob can recurse on a run of zero bytes and overflow the stack.
    // Parse on a thread with a generous stack so it can never crash the process.
    let bytes = schema.to_vec();
    std::thread::Builder::new()
        .name("ach-schema-parse".to_string())
        .stack_size(16 * 1024 * 1024)
        .spawn(move || parse_achievement_schema_inner(&bytes))
        .ok()
        .and_then(|handle| handle.join().ok())
        .unwrap_or_default()
}

/// Inner schema parse. Parses the blob directly as binary KeyValues (no
/// appinfo-style offset scanning, which could feed the parser misaligned data).
pub(super) fn parse_achievement_schema_inner(schema: &[u8]) -> HashMap<String, (u32, u32)> {
    let mut map = HashMap::new();
    let Ok(vdf) = steam_vdf_parser::parse_binary(schema) else {
        return map;
    };
    let Some(root) = vdf.as_obj() else {
        return map;
    };
    // `stats` is directly under the appid-keyed root (or one level deeper).
    let stats = root.get("stats").and_then(|v| v.as_obj()).or_else(|| {
        root.values()
            .next()
            .and_then(|v| v.as_obj())
            .and_then(|o| o.get("stats"))
            .and_then(|v| v.as_obj())
    });
    let Some(stats) = stats else {
        return map;
    };

    for (stat_key, stat_val) in stats.iter() {
        let Ok(stat_id) = stat_key.parse::<u32>() else {
            continue;
        };
        let Some(stat_obj) = stat_val.as_obj() else {
            continue;
        };
        // Achievement-bitfield stats are exactly those carrying a `bits` table.
        // (Don't filter on `type`: in binary KV it's an integer, not "4".)
        let Some(bits) = stat_obj.get("bits").and_then(|v| v.as_obj()) else {
            continue;
        };
        for (bit_key, bit_val) in bits.iter() {
            let Ok(bit) = bit_key.parse::<u32>() else {
                continue;
            };
            if let Some(name) = bit_val
                .as_obj()
                .and_then(|o| o.get("name"))
                .and_then(|v| v.as_str())
            {
                map.insert(name.to_string(), (stat_id, bit));
            }
        }
    }
    map
}
