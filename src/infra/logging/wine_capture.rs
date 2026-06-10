#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum StartupMilestone {
    None = 0,
    InitialProcessBootstrap = 1,
    SteamBootstrapInitialized = 2,
    GameLocalDllsLoaded = 3,
    GraphicsRendererInitStarted = 4,
    RunningState = 5,
}

impl std::fmt::Display for StartupMilestone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::None => "none",
            Self::InitialProcessBootstrap => "initial_process_bootstrap",
            Self::SteamBootstrapInitialized => "steam_bootstrap_initialized",
            Self::GameLocalDllsLoaded => "game_local_dlls_loaded",
            Self::GraphicsRendererInitStarted => "graphics_renderer_init_started",
            Self::RunningState => "running_state",
        };
        write!(f, "{}", s)
    }
}

pub fn detect_startup_milestone(log_line: &str) -> Option<StartupMilestone> {
    let line_lower = log_line.to_lowercase();

    // Steam Runtime specific milestones (can be used for background Steam too)
    if line_lower.contains("steam.exe") && line_lower.contains("starting") {
        return Some(StartupMilestone::InitialProcessBootstrap);
    }

    // Generic bootstrap markers
    if line_lower.contains("wine_init") || line_lower.contains("ntdll:ldrload_dll") || line_lower.contains("kernelbase:loadlibrary") {
        return Some(StartupMilestone::InitialProcessBootstrap);
    }

    // Steam bootstrap
    if line_lower.contains("steamapi_init") || line_lower.contains("steamapi_restartappifnecessary") || line_lower.contains("steam_client: initialized") || line_lower.contains("steamapi_init_all") {
        return Some(StartupMilestone::SteamBootstrapInitialized);
    }

    // Game local DLLs (representative families for Batman/Amnesia/Metro)
    if line_lower.contains("loaddll") && (
        line_lower.contains("physx") || line_lower.contains("gfsdk") || line_lower.contains("nvtt") || // Batman
        line_lower.contains("sdl2") || line_lower.contains("newton") || line_lower.contains("devil") ||   // Amnesia
        line_lower.contains("physxdevice") || line_lower.contains("4a_backend")                         // Metro
    ) {
        return Some(StartupMilestone::GameLocalDllsLoaded);
    }

    // Renderer initialization
    if line_lower.contains("d3d11internalcreatedevice") ||
       line_lower.contains("dxvk: v") ||
       line_lower.contains("vkd3d-proton: v") ||
       line_lower.contains("presenter: actual swapchain properties") ||
       line_lower.contains("wined3d: v") {
        return Some(StartupMilestone::GraphicsRendererInitStarted);
    }

    // Likely running state
    if line_lower.contains("initialization complete") || line_lower.contains("game: main loop started") {
        return Some(StartupMilestone::RunningState);
    }

    None
}

pub fn classify_graphics_evidence(log_line: &str) -> Option<String> {
    let line_lower = log_line.to_lowercase();

    // Noise Filter: generic Wine loader noise is ignored for evidence but preserved for milestones
    if line_lower.contains("find_builtin_dll") ||
       line_lower.contains("winemac.drv") ||
       line_lower.contains("winecoreaudio.drv") ||
       line_lower.contains("xinput") ||
       line_lower.contains("libegl") {
        return None;
    }

    // DXVK signatures
    if line_lower.contains("info:  dxvk:") ||
       line_lower.contains("dxvk: v") ||
       line_lower.contains("info:  game:") ||
       line_lower.contains("d3d11internalcreatedevice") ||
       line_lower.contains("presenter: actual swapchain properties") ||
       (line_lower.contains("vulkan:") && line_lower.contains("found vkgetinstanceprocaddr")) {
        return Some(format!("DXVK Detected: {}", log_line.trim()));
    }

    // VKD3D-Proton signatures
    if line_lower.contains("vkd3d-proton: v") {
        return Some(format!("VKD3D-Proton Detected: {}", log_line.trim()));
    }

    // Generic VKD3D
    if line_lower.contains("vkd3d: v") {
        return Some(format!("VKD3D Detected: {}", log_line.trim()));
    }

    // WineD3D fallback hints
    if line_lower.contains("wined3d: v") || line_lower.contains("wined3d-adapter") {
        return Some(format!("WineD3D Fallback Detected: {}", log_line.trim()));
    }

    // DLL Load Failures
    if line_lower.contains("failed to load module") && line_lower.contains("status=") {
        // Filter out winemac.drv which is normal to fail on Linux (standard Wine bootstrap noise)
        if line_lower.contains("winemac.drv") {
            return None;
        }

        // Specific middleware/dependency failure patterns
        if line_lower.contains("physx") {
             return Some(format!("PhysX/Middleware failure: {}", log_line.trim()));
        }

        return Some(format!("DLL Load Failure: {}", log_line.trim()));
    }
    if line_lower.contains("not found") && line_lower.contains("which is needed by") {
        return Some(format!("DLL Dependency Missing: {}", log_line.trim()));
    }

    // Steam/Client patterns
    if line_lower.contains("failed to create steam.exe") ||
       line_lower.contains("cannot find 'steam.exe'") ||
       (line_lower.contains("steam.exe") && (line_lower.contains("not found") || line_lower.contains("failed"))) {
        return Some(format!("Steam Client/Environment Failure: {}", log_line.trim()));
    }

    if line_lower.contains("steamapi_init") && (line_lower.contains("failed") || line_lower.contains("error")) {
        return Some(format!("SteamAPI Initialization Failed: {}", log_line.trim()));
    }

    if line_lower.contains("steamapi_restartappifnecessary") && (line_lower.contains("returning true") || line_lower.contains("restarting")) {
        return Some(format!("Steam Handoff Triggered Restart: {}", log_line.trim()));
    }

    if line_lower.contains("steamapi_issteamrunning") && line_lower.contains("did not locate") {
        return Some(format!("SteamAPI Connection Failed (Steam not found): {}", log_line.trim()));
    }

    if line_lower.contains("user does not own") || line_lower.contains("ownership failed") {
        return Some(format!("Steam Ownership Validation Failed: {}", log_line.trim()));
    }

    if line_lower.contains("tried to access steam interface") && line_lower.contains("before steamapi_init") {
        return Some(format!("SteamAPI Access Violation: {}", log_line.trim()));
    }

    // Steam Client Artifact Detection
    if line_lower.contains("loaddll") || line_lower.contains("load_module") {
        if line_lower.contains("steam_api.dll") || line_lower.contains("steam_api64.dll") {
             return Some(format!("Steam Client Artifact: local ({})", log_line.trim()));
        }
        if line_lower.contains("steamclient.dll") || line_lower.contains("steamclient64.dll") {
             if line_lower.contains("program files") || line_lower.contains("steam") {
                 return Some(format!("Steam Client Artifact: windows ({})", log_line.trim()));
             }
        }
        if line_lower.contains("lsteamclient") || line_lower.contains("steamclient.so") {
             return Some(format!("Steam Client Artifact: host ({})", log_line.trim()));
        }
    }

    // Override/Policy regressions
    if line_lower.contains("invalid dll") ||
       (line_lower.contains("failed to load") && (line_lower.contains("d3d11"))) {
         return Some(format!("Override Policy Conflict: {}", log_line.trim()));
    }

    None
}
