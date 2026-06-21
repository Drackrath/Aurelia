//! Unit tests for `UmuRunner::build_command` / `build_env`.
//!
//! These exercise the umu runner against a fabricated `LaunchContext` (no real `umu-run`
//! binary or Proton install needed). They lock the umu invocation contract: `umu-run` takes
//! the game executable directly as the first argument (no `proton run` wrapper) and drives
//! Proton through environment variables (`GAMEID`, `STORE`, `PROTONPATH`, prefix/compat env).
//! See `.docs/umu-integration-plan.md` §8 and `.docs/wiki/16-tests.md` §4.

use aurelia::config::LauncherConfig;
use aurelia::infra::runners::{LaunchContext, Runner, UmuRunner};
use aurelia::models::LibraryGame;
use aurelia::steam_client::{LaunchInfo, LaunchTarget};
use std::collections::HashMap;
use std::sync::LazyLock;
use tempfile::tempdir;

/// Point `config_dir()` (used by `build_env` for the fake-steam trap + log path) at a single
/// throwaway directory for the whole test binary, so these tests never touch the real
/// `~/.config/Aurelia`. Set exactly once (no per-test env mutation → no races between the
/// parallel async tests). The tempdir is leaked so it outlives every test.
static ISOLATED_CONFIG: LazyLock<()> = LazyLock::new(|| {
    let dir = Box::leak(Box::new(tempdir().unwrap()));
    // SAFETY: set once during lazy init, before any test reads the env var; never mutated again.
    unsafe { std::env::set_var("AURELIA_CONFIG_DIR", dir.path()) };
});

/// Build a fixture `LaunchContext` for a Windows/Proton game. `install_path`, the Steam
/// library and the Proton dir are real tempdir paths so the runner's path/prefix/Proton
/// resolution produces deterministic, absolute values.
fn umu_context(
    library_path: &str,
    install_path: &str,
    proton_path: &str,
    user_config: Option<aurelia::models::UserAppConfig>,
) -> LaunchContext {
    let mut config = LauncherConfig::default();
    config.steam_library_path = library_path.to_string();
    // NB: deliberately left at the default `use_shared_compat_data = false`. The umu runner
    // forces the per-game compatdata layout itself, so WINEPREFIX and STEAM_COMPAT_DATA_PATH
    // must point at `<library>/.../compatdata/<id>[/pfx]` regardless of this flag.

    LaunchContext {
        app: LibraryGame {
            app_id: 480,
            name: "Test Game".to_string(),
            playtime_forever_minutes: None,
            is_installed: true,
            install_path: Some(install_path.to_string()),
            local_manifest_ids: Default::default(),
            update_available: false,
            update_queued: false,
            active_branch: "public".to_string(),
            is_owned: true,
            is_family_shared: false,
            online_required: None,
            platform: None,
        },
        launch_info: LaunchInfo {
            app_id: 480,
            id: "0".to_string(),
            description: "Test".to_string(),
            executable: "bin/game.exe".to_string(),
            arguments: "-skipintro".to_string(),
            workingdir: None,
            target: LaunchTarget::WindowsProton,
        },
        launcher_config: config,
        user_config,
        proton_path: Some(proton_path.to_string()),
        steam_enabled: false,
        target_architecture: aurelia::models::ExecutableArchitecture::X86_64,
        dll_resolutions: Vec::new(),
        verification_ptr: std::ptr::null_mut(),
    }
}

/// Build the runner's env with `config_dir()` isolated to the per-binary tempdir.
async fn build_env_isolated(ctx: &LaunchContext) -> HashMap<String, String> {
    LazyLock::force(&ISOLATED_CONFIG);
    UmuRunner.build_env(ctx).await.expect("build_env")
}

#[tokio::test]
async fn test_build_command_uses_configured_umu_path() {
    let lib = tempdir().unwrap();
    let install = tempdir().unwrap();
    let proton = tempdir().unwrap();

    let mut ctx = umu_context(
        &lib.path().to_string_lossy(),
        &install.path().to_string_lossy(),
        &proton.path().to_string_lossy(),
        None,
    );
    ctx.launcher_config.umu_path = Some("/opt/umu/umu-run".to_string());

    LazyLock::force(&ISOLATED_CONFIG);
    let spec = UmuRunner.build_command(&ctx).await.expect("build_command");

    assert_eq!(spec.program, std::path::PathBuf::from("/opt/umu/umu-run"));
}

#[tokio::test]
async fn test_build_command_program_defaults_to_umu_run() {
    let lib = tempdir().unwrap();
    let install = tempdir().unwrap();
    let proton = tempdir().unwrap();

    let ctx = umu_context(
        &lib.path().to_string_lossy(),
        &install.path().to_string_lossy(),
        &proton.path().to_string_lossy(),
        None,
    );

    LazyLock::force(&ISOLATED_CONFIG);
    let spec = UmuRunner.build_command(&ctx).await.expect("build_command");

    // No umu_path configured → resolved on $PATH as the bare `umu-run`.
    assert_eq!(spec.program, std::path::PathBuf::from("umu-run"));
}

#[tokio::test]
async fn test_build_command_exe_is_first_arg_no_proton_run_wrapper() {
    let lib = tempdir().unwrap();
    let install = tempdir().unwrap();
    let proton = tempdir().unwrap();

    let ctx = umu_context(
        &lib.path().to_string_lossy(),
        &install.path().to_string_lossy(),
        &proton.path().to_string_lossy(),
        None,
    );

    LazyLock::force(&ISOLATED_CONFIG);
    let spec = UmuRunner.build_command(&ctx).await.expect("build_command");

    // The game executable is arg 0 — umu-run takes it directly, with NO `run` sub-command
    // and NO `proton` wrapper (contrast WineTkgRunner's `proton run <exe>`).
    let expected_exe = install.path().join("bin/game.exe");
    assert_eq!(spec.args[0], expected_exe.to_string_lossy());
    assert_ne!(spec.args[0], "run");
    assert!(!spec.args.iter().any(|a| a == "run"));
    assert!(!spec.args.iter().any(|a| a.ends_with("proton")));

    // LaunchInfo arguments follow the exe.
    assert!(spec.args.iter().any(|a| a == "-skipintro"));

    // cwd is the executable's parent (the game's bin dir).
    assert_eq!(spec.cwd, Some(install.path().join("bin")));
}

#[tokio::test]
async fn test_build_env_umu_and_steam_identity_contract() {
    let lib = tempdir().unwrap();
    let install = tempdir().unwrap();
    let proton = tempdir().unwrap();

    let ctx = umu_context(
        &lib.path().to_string_lossy(),
        &install.path().to_string_lossy(),
        &proton.path().to_string_lossy(),
        None,
    );

    let env = build_env_isolated(&ctx).await;

    // --- umu / protonfixes identity ---
    assert_eq!(env.get("GAMEID").unwrap(), "umu-480");
    assert_eq!(env.get("STORE").unwrap(), "steam");
    assert_eq!(env.get("PROTON_VERB").unwrap(), "waitforexitandrun");

    // --- PROTONPATH is the Proton *directory* (runner root), not a `proton` script ---
    assert_eq!(env.get("PROTONPATH").unwrap(), &proton.path().to_string_lossy().to_string());

    // --- Steam compat identity, required for stop_game's /proc/*/environ sweep ---
    assert_eq!(env.get("STEAM_COMPAT_APP_ID").unwrap(), "480");
    assert_eq!(env.get("SteamAppId").unwrap(), "480");

    // --- Prefix + compatdata wiring ---
    let compat = lib.path().join("steamapps/compatdata/480");
    assert_eq!(
        env.get("STEAM_COMPAT_DATA_PATH").unwrap(),
        &compat.to_string_lossy().to_string()
    );
    let prefix = env.get("WINEPREFIX").unwrap();
    assert!(
        prefix.ends_with("compatdata/480/pfx"),
        "WINEPREFIX should be the per-game pfx, got {prefix}"
    );

    // --- Host-Steam install path is always set (fake-steam trap when steam_enabled=false) ---
    assert!(env.contains_key("STEAM_COMPAT_CLIENT_INSTALL_PATH"));

    // --- All env keys the integration plan §2 lists as required are present ---
    for key in [
        "GAMEID",
        "STORE",
        "PROTONPATH",
        "WINEPREFIX",
        "STEAM_COMPAT_DATA_PATH",
        "STEAM_COMPAT_APP_ID",
        "PROTON_VERB",
    ] {
        assert!(env.contains_key(key), "missing required env var {key}");
    }
}

#[tokio::test]
async fn test_build_env_does_not_manage_dlls_but_passes_user_overrides() {
    use aurelia::models::UserAppConfig;

    let lib = tempdir().unwrap();
    let install = tempdir().unwrap();
    let proton = tempdir().unwrap();

    // Baseline: umu must NOT synthesize WINEDLLOVERRIDES (Proton/protonfixes own DLLs, D2).
    let ctx = umu_context(
        &lib.path().to_string_lossy(),
        &install.path().to_string_lossy(),
        &proton.path().to_string_lossy(),
        None,
    );
    let env = build_env_isolated(&ctx).await;
    assert!(
        !env.contains_key("WINEDLLOVERRIDES"),
        "umu must not synthesize WINEDLLOVERRIDES; got {:?}",
        env.get("WINEDLLOVERRIDES")
    );

    // But a user-supplied env override is still passed through additively.
    let mut user = UserAppConfig::default();
    user.env_variables
        .insert("WINEDLLOVERRIDES".to_string(), "winemenubuilder.exe=d".to_string());
    user.env_variables
        .insert("MY_CUSTOM".to_string(), "1".to_string());
    let ctx_user = umu_context(
        &lib.path().to_string_lossy(),
        &install.path().to_string_lossy(),
        &proton.path().to_string_lossy(),
        Some(user),
    );
    let env_user = build_env_isolated(&ctx_user).await;
    assert_eq!(
        env_user.get("WINEDLLOVERRIDES").unwrap(),
        "winemenubuilder.exe=d"
    );
    assert_eq!(env_user.get("MY_CUSTOM").unwrap(), "1");
}
