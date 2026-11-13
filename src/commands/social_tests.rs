use super::*;
use aurelia::steam_client::Friend;
use std::collections::HashMap;

fn friend(steam_id: u64, name: Option<&str>) -> Friend {
    Friend {
        steam_id,
        relationship: 3,
        persona_name: name.map(str::to_string),
        persona_state: None,
        game_app_id: None,
        game_name: None,
    }
}

#[test]
fn cached_names_fill_only_missing() {
    let mut friends = vec![
        friend(76561198000000001, None),
        friend(76561198000000002, Some("Live")),
        friend(76561198000000003, None),
    ];
    let cache = HashMap::from([
        (76561198000000001, "Cached".to_string()),
        (76561198000000002, "Stale".to_string()),
    ]);
    let filled = apply_cached_names(&mut friends, &cache);
    assert_eq!(friends[0].persona_name.as_deref(), Some("Cached"));
    assert_eq!(friends[1].persona_name.as_deref(), Some("Live"));
    assert_eq!(friends[2].persona_name, None);
    assert_eq!(filled.len(), 1);
    assert!(filled.contains(&76561198000000001));
    // Status stays unknown.
    assert_eq!(friends[0].persona_state, None);
}
