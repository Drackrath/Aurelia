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
    assert!(fx.launch_args.is_empty());
    assert!(fx.reg_ops.is_empty());
}

#[test]
fn fixup_lookup_returns_gta_v_entry() {
    let fx = game_fixups(271590);
    assert!(fx
        .env
        .iter()
        .any(|(k, v)| k == "WINE_LARGE_ADDRESS_AWARE" && v == "1"));
    assert!(fx
        .env
        .iter()
        .any(|(k, v)| k == "STAGING_SHARED_MEMORY" && v == "1"));
    assert!(fx
        .dll_overrides
        .iter()
        .any(|(dll, mode)| dll == "dinput8" && mode == "n,b"));
    assert_eq!(fx.launch_args, vec!["-ignoredifferentvideocard".to_string()]);
    assert!(fx.reg_ops.is_empty());
}

#[test]
fn no_seed_entry_overrides_amd_ags() {
    // Upstream correction: an amd_ags_x64 override breaks RE2 (exit 53) — the
    // builtin AGS stub may be absent from minimal runners and game-local priority
    // already resolves the game's own DLL. No seed entry may reintroduce it.
    for entry in FIXUPS {
        assert!(
            !entry.dll_overrides.iter().any(|(dll, _)| dll.contains("amd_ags")),
            "app {} carries an amd_ags override",
            entry.app_id
        );
    }
}

#[test]
fn reg_op_dword_renders_reg_add_args() {
    let op = RegOp::Dword {
        path: r"HKCU\Software\Test\Game",
        key: "SkipIntro",
        value: 1,
    };
    assert_eq!(
        op.to_reg_add_args(),
        vec![
            "reg.exe", "add", r"HKCU\Software\Test\Game",
            "/v", "SkipIntro", "/t", "REG_DWORD", "/d", "1", "/f",
        ]
    );
}

#[test]
fn reg_op_string_renders_reg_add_args() {
    let op = RegOp::String {
        path: r"HKCU\Software\Wine\Direct3D",
        key: "renderer",
        value: "vulkan",
    };
    assert_eq!(
        op.to_reg_add_args(),
        vec![
            "reg.exe", "add", r"HKCU\Software\Wine\Direct3D",
            "/v", "renderer", "/t", "REG_SZ", "/d", "vulkan", "/f",
        ]
    );
}
