use crate::compat::anticheat::AntiCheat;
use crate::core::config::{GameConfig, GameRunner, LauncherConfig};
use crate::launch::stand_in_guard::*;

fn stand_in_on() -> LauncherConfig {
    LauncherConfig {
        steamworks_client: true,
        ..Default::default()
    }
}

#[test]
fn vac_category_is_protection() {
    assert_eq!(protections(Some(&[2, 8, 36]), &[]), vec![Protection::Vac]);
    assert!(protections(Some(&[2, 36]), &[]).is_empty());
    assert!(protections(Some(&[]), &[]).is_empty());
}

#[test]
fn unreadable_categories_fail_closed() {
    assert_eq!(protections(None, &[]), vec![Protection::Unknown]);
}

#[test]
fn installed_anticheats_are_protections() {
    let found = protections(Some(&[2]), &[AntiCheat::EasyAntiCheat, AntiCheat::BattlEye]);
    assert_eq!(found, vec![Protection::EasyAntiCheat, Protection::BattlEye]);
}

#[test]
fn protected_game_is_refused_unless_allowed() {
    let mut config = stand_in_on();
    assert_eq!(evaluate(10, Vec::new(), &config), Verdict::Clear);
    assert_eq!(
        evaluate(10, vec![Protection::Vac], &config),
        Verdict::Refused(vec![Protection::Vac])
    );
    config.steamworks_allowed_protected = vec![10];
    assert_eq!(
        evaluate(10, vec![Protection::Vac], &config),
        Verdict::Allowed(vec![Protection::Vac])
    );
    assert_eq!(
        evaluate(11, vec![Protection::BattlEye], &config),
        Verdict::Refused(vec![Protection::BattlEye])
    );
}

#[test]
fn guard_only_applies_in_stand_in_mode() {
    let off = LauncherConfig::default();
    assert!(!stand_in_applies(&off, 10, false));
    let on = stand_in_on();
    assert_eq!(stand_in_applies(&on, 10, false), cfg!(target_os = "linux"));
    assert!(!stand_in_applies(&on, 10, true));
}

#[test]
fn luxtorpeda_games_skip_the_guard() {
    let mut config = stand_in_on();
    config.luxtorpeda_enabled = true;
    config.game_configs.insert(
        10,
        GameConfig {
            runner: GameRunner::Luxtorpeda,
            ..Default::default()
        },
    );
    assert!(!stand_in_applies(&config, 10, false));
    assert_eq!(
        stand_in_applies(&config, 11, false),
        cfg!(target_os = "linux")
    );
}

#[test]
fn refusal_names_ban_risk_and_way_out() {
    let msg = refusal_message(730, "Game", &[Protection::Vac, Protection::EasyAntiCheat]);
    assert!(msg.contains("Game (730)"));
    assert!(msg.contains("VAC, EasyAntiCheat"));
    assert!(msg.contains("ban your real Steam account"));
    assert!(msg.contains("unmodified"));
    assert!(msg.contains("--allow 730"));
    assert!(msg.contains("steamworks-client off"));
}

#[test]
fn allow_prompt_warns_about_ban() {
    let prompt = allow_prompt(730);
    assert!(prompt.contains("app 730"));
    assert!(prompt.contains("ban your real Steam account"));
    assert!(prompt.trim_end().ends_with("[y/N]"));
}

#[test]
fn allowed_warning_mentions_multiplayer() {
    let msg = allowed_warning(5, "G", &[Protection::BattlEye]);
    assert!(msg.contains("BattlEye"));
    assert!(msg.contains("Multiplayer needs Valve's Steam client"));
}

#[test]
fn categories_parse_from_text_appinfo() {
    let vdf = "\"appinfo\"\n{\n\t\"appid\"\t\"5\"\n\t\"common\"\n\t{\n\t\t\"name\"\t\"G\"\n\
               \t\t\"category\"\n\t\t{\n\t\t\t\"category_8\"\t\"1\"\n\t\t\t\"category_2\"\t\"1\"\n\
               \t\t}\n\t}\n}\n";
    let ids = crate::steam_client::category_ids_from_pics(vdf.as_bytes()).unwrap();
    assert_eq!(ids, vec![2, 8]);
}

#[test]
fn categories_missing_common_is_unknown() {
    let vdf = "\"appinfo\"\n{\n\t\"appid\"\t\"5\"\n}\n";
    assert_eq!(
        crate::steam_client::category_ids_from_pics(vdf.as_bytes()),
        None
    );
}

#[test]
fn stand_in_drops_every_valve_steam_start() {
    // flag, owned, stand-in -> start Steam
    let rows = [
        (false, true, false, false),
        (true, true, false, true),
        (false, false, false, true),
        (true, true, true, false),
        (false, false, true, false),
        (true, false, true, false),
    ];
    for (flag, owned, stand_in, want) in rows {
        assert_eq!(
            wants_valve_steam(flag, owned, stand_in),
            want,
            "{flag} {owned} {stand_in}"
        );
    }
    assert!(REPLACES_STEAM_NOTE.contains("stand-in"));
}
