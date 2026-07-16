use super::*;

#[test]
fn fixup_lookup_returns_seeded_entry() {
    let fx = game_fixups(211420);
    assert!(!fx.is_empty());
    assert!(fx
        .env
        .iter()
        .any(|(k, v)| k == "PROTON_NO_ESYNC" && v == "1"));
}

#[test]
fn fixup_lookup_returns_dll_override_entry() {
    let fx = game_fixups(22370);
    assert!(fx.env.is_empty());
    assert!(fx
        .dll_overrides
        .iter()
        .any(|(dll, mode)| dll == "xlive" && mode == "builtin"));
}

#[test]
fn fixup_lookup_unknown_app_is_empty() {
    let fx = game_fixups(4_294_967_295);
    assert!(fx.is_empty());
    assert!(fx.env.is_empty());
    assert!(fx.dll_overrides.is_empty());
}
