use std::collections::HashMap;

use crate::launch::wayland_native::*;

fn runner(with_driver: bool) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let arch = dir.path().join("files/lib/wine/x86_64-windows");
    std::fs::create_dir_all(&arch).unwrap();
    if with_driver {
        std::fs::write(arch.join("winewayland.drv"), b"").unwrap();
    }
    dir
}

#[test]
fn proton_with_driver_gets_flag() {
    let dir = runner(true);
    assert_eq!(
        plan(Some(dir.path())),
        WaylandPlan::Enable(vec![(PROTON_FLAG, "1")])
    );
}

#[test]
fn proton_without_driver_is_unsupported() {
    let dir = runner(false);
    assert!(matches!(
        plan(Some(dir.path())),
        WaylandPlan::Unsupported(_)
    ));
}

#[test]
fn native_sets_sdl_driver() {
    let WaylandPlan::Enable(vars) = plan(None) else {
        panic!("native must enable");
    };
    assert!(vars.contains(&("SDL_VIDEODRIVER", "wayland")));
}

#[test]
fn user_values_win() {
    let mut env = HashMap::from([(PROTON_FLAG.to_string(), "0".to_string())]);
    apply(
        &mut env,
        &[(PROTON_FLAG, "1"), ("SDL_VIDEODRIVER", "wayland")],
    );
    assert_eq!(env[PROTON_FLAG], "0");
    assert_eq!(env["SDL_VIDEODRIVER"], "wayland");
}
