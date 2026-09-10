use super::*;

fn lang_cc(raw: &str) -> (String, String) {
    let l = parse_locale(raw).unwrap_or_else(|| panic!("should parse {raw}"));
    (steam_language(&l).to_string(), steam_country(&l))
}

#[test]
fn posix_tags_map_to_steam_codes() {
    assert_eq!(lang_cc("zh_CN.UTF-8"), ("schinese".into(), "CN".into()));
    assert_eq!(lang_cc("ja_JP.UTF-8"), ("japanese".into(), "JP".into()));
    assert_eq!(lang_cc("es_MX.UTF-8"), ("latam".into(), "MX".into()));
    assert_eq!(lang_cc("es_ES@euro"), ("spanish".into(), "ES".into()));
    assert_eq!(lang_cc("pt_BR"), ("brazilian".into(), "BR".into()));
    assert_eq!(lang_cc("nb_NO.UTF-8"), ("norwegian".into(), "NO".into()));
    assert_eq!(lang_cc("en_GB.UTF-8"), ("english".into(), "GB".into()));
}

#[test]
fn bcp47_script_selects_traditional_chinese() {
    assert_eq!(lang_cc("zh-Hant-TW"), ("tchinese".into(), "TW".into()));
    assert_eq!(lang_cc("zh-Hant"), ("tchinese".into(), "TW".into()));
    assert_eq!(lang_cc("zh_HK"), ("tchinese".into(), "HK".into()));
    assert_eq!(lang_cc("zh"), ("schinese".into(), "CN".into()));
}

#[test]
fn language_only_falls_back_to_a_default_country() {
    assert_eq!(lang_cc("de"), ("german".into(), "DE".into()));
    assert_eq!(lang_cc("en"), ("english".into(), "US".into()));
    assert_eq!(lang_cc("xx_YY"), ("english".into(), "YY".into()));
}

#[test]
fn neutral_and_garbage_locales_do_not_parse() {
    for raw in ["C", "POSIX", "C.UTF-8", "", "   ", "1234"] {
        assert!(parse_locale(raw).is_none(), "{raw:?} should not parse");
    }
}

#[test]
fn country_normalization() {
    assert_eq!(normalize_country("de").as_deref(), Some("DE"));
    assert_eq!(normalize_country(" uk ").as_deref(), Some("GB"));
    assert_eq!(normalize_country("usa"), None);
    assert_eq!(normalize_country("u1"), None);
}
