//! Native Steamworks emulator; exports libsteam_api.so.
#![allow(non_snake_case)]

use std::collections::HashMap;
use std::ffi::{c_void, CStr};
use std::os::raw::c_char;
use std::sync::{Mutex, OnceLock};

const VTABLE_LEN: usize = 256;

// Every unhandled method returns zero.
unsafe extern "C" fn stub(_this: *mut c_void) -> usize {
    0
}

// Real returns games rely on.
unsafe extern "C" fn ret_true(_this: *mut c_void) -> bool {
    true
}
unsafe extern "C" fn ret_steamid(_this: *mut c_void) -> u64 {
    // Valid individual SteamID.
    0x0110_0001_0000_1001
}
unsafe extern "C" fn ret_appid(_this: *mut c_void) -> u32 {
    app_id()
}
unsafe extern "C" fn ret_name(_this: *mut c_void) -> *const c_char {
    b"Player\0".as_ptr() as *const c_char
}
unsafe extern "C" fn ret_lang(_this: *mut c_void) -> *const c_char {
    b"english\0".as_ptr() as *const c_char
}

/// The running game's app id.
fn app_id() -> u32 {
    static ID: OnceLock<u32> = OnceLock::new();
    *ID.get_or_init(|| {
        std::env::var("SteamAppId")
            .ok()
            .or_else(|| std::env::var("SteamGameId").ok())
            .and_then(|s| s.trim().parse().ok())
            .or_else(|| {
                std::fs::read_to_string("steam_appid.txt")
                    .ok()
                    .and_then(|s| s.trim().parse().ok())
            })
            .unwrap_or(0)
    })
}

/// Leak a vtable with overrides.
fn make_vtable(overrides: &[(usize, usize)]) -> usize {
    let mut v = vec![stub as *const () as usize; VTABLE_LEN];
    for &(i, f) in overrides {
        v[i] = f;
    }
    let boxed = v.into_boxed_slice();
    let ptr = boxed.as_ptr() as usize;
    std::mem::forget(boxed);
    ptr
}

#[repr(C)]
struct Iface {
    vtable: usize,
}

/// Leak a singleton interface object.
fn make_iface(vt: usize) -> *mut c_void {
    Box::into_raw(Box::new(Iface { vtable: vt })) as *mut c_void
}

/// Vtable for this interface family.
fn vtable_for(version: &str) -> usize {
    macro_rules! vt {
        ($cell:ident, $ov:expr) => {{
            static $cell: OnceLock<usize> = OnceLock::new();
            *$cell.get_or_init(|| make_vtable($ov))
        }};
    }
    if version.starts_with("SteamUser") {
        // 0 GetHSteamUser, 1 BLoggedOn, 2 GetSteamID.
        vt!(USER, &[(1, ret_true as *const () as usize), (2, ret_steamid as *const () as usize)])
    } else if version.starts_with("SteamUtils") {
        // 8 GetAppID, 21 GetSteamUILanguage.
        vt!(UTILS, &[(8, ret_appid as *const () as usize), (21, ret_lang as *const () as usize)])
    } else if version.starts_with("STEAMUSERSTATS") {
        // 0 RequestCurrentStats.
        vt!(STATS, &[(0, ret_true as *const () as usize)])
    } else if version.starts_with("SteamFriends") {
        // 0 GetPersonaName.
        vt!(FRIENDS, &[(0, ret_name as *const () as usize)])
    } else if version.starts_with("STEAMAPPS") {
        // 0 BIsSubscribed, 4/5 language getters.
        vt!(
            APPS,
            &[
                (0, ret_true as *const () as usize),
                (4, ret_lang as *const () as usize),
                (5, ret_lang as *const () as usize),
            ]
        )
    } else {
        vt!(GENERIC, &[])
    }
}

/// Cached interface singleton per version.
fn interface_for(version: &str) -> *mut c_void {
    static CACHE: OnceLock<Mutex<HashMap<String, usize>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut g = cache.lock().unwrap();
    if let Some(&p) = g.get(version) {
        return p as *mut c_void;
    }
    let p = make_iface(vtable_for(version)) as usize;
    g.insert(version.to_string(), p);
    p as *mut c_void
}

// flat exports

#[unsafe(no_mangle)]
pub unsafe extern "C" fn SteamAPI_Init() -> bool {
    true
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn SteamAPI_Shutdown() {}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn SteamAPI_RunCallbacks() {}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn SteamAPI_GetHSteamUser() -> i32 {
    1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn SteamAPI_GetHSteamPipe() -> i32 {
    1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn SteamAPI_RegisterCallback(_cb: *mut c_void, _icb: i32) {}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn SteamAPI_UnregisterCallback(_cb: *mut c_void) {}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn SteamAPI_RegisterCallResult(_cb: *mut c_void, _call: u64) {}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn SteamAPI_UnregisterCallResult(_cb: *mut c_void, _call: u64) {}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn SteamAPI_IsSteamRunning() -> bool {
    true
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn SteamAPI_RestartAppIfNecessary(_appid: u32) -> bool {
    false
}

/// Populate accessor; return interface storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn SteamInternal_ContextInit(ctx: *mut *mut c_void) -> *mut c_void {
    // ctx[0]=init fn, ctx[1]=counter, ctx[2]=interface storage.
    unsafe {
        let counter = ctx.add(1);
        if (*counter).is_null() {
            let init: extern "C" fn(*mut *mut c_void) = std::mem::transmute(*ctx);
            init(ctx.add(2));
            *counter = 1usize as *mut c_void;
        }
        ctx.add(2) as *mut c_void
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn SteamInternal_FindOrCreateUserInterface(
    _user: i32,
    version: *const c_char,
) -> *mut c_void {
    if version.is_null() {
        return interface_for("");
    }
    let v = unsafe { CStr::from_ptr(version) }.to_string_lossy().into_owned();
    interface_for(&v)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn SteamInternal_CreateInterface(version: *const c_char) -> *mut c_void {
    unsafe { SteamInternal_FindOrCreateUserInterface(1, version) }
}
