#![allow(clippy::missing_safety_doc)]

use std::ffi::c_void;
use std::path::{Path, PathBuf};

use libloading::Library;
use once_cell::sync::OnceCell;

use crate::error::{AppError, AppResult};

// Re-export pre-generated bindgen declarations.
#[allow(
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    dead_code,
    improper_ctypes,
    unused
)]
mod generated {
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    include!("generated/windows_x86_64.rs");

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    include!("generated/linux_x86_64.rs");

    #[cfg(all(target_os = "linux", target_arch = "x86"))]
    include!("generated/linux_x86.rs");

    #[cfg(not(any(
        all(target_os = "windows", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "x86")
    )))]
    compile_error!("No pre-generated Hikvision bindings for this target");
}
pub use generated::*;

// Keep the library alive for the whole process so function pointers remain valid.
static SDK_LIBRARY: OnceCell<Library> = OnceCell::new();
static SDK_API: OnceCell<HCNetSdkLib> = OnceCell::new();
#[cfg(windows)]
static SDK_DLL_DIRS: OnceCell<()> = OnceCell::new();

pub fn ensure_hcnetsdk_loaded(custom_path: Option<&str>) -> AppResult<()> {
    if SDK_LIBRARY.get().is_some() {
        return Ok(());
    }
    let path = resolve_sdk_path(custom_path)?;
    if !path.exists() {
        return Err(AppError::Sdk(format!(
            "SDK library not found at '{}'. Set HIKVISION_SDK_PATH to the SDK root for this platform.",
            path.display()
        )));
    }
    #[cfg(windows)]
    ensure_windows_sdk_dll_dirs(&path)?;
    // SAFETY: Dynamic loading by path; handle is retained in static OnceCell.
    unsafe {
        if SDK_API.get().is_none() {
            let api = HCNetSdkLib::new(&path)
                .map_err(|e| AppError::Sdk(format!("failed to load HCNetSDK API table: {e}")))?;
            let _ = SDK_API.set(api);
        }
        if SDK_LIBRARY.get().is_none() {
            let lib = Library::new(&path)
                .map_err(|e| AppError::Sdk(format!("failed to pin SDK library: {e}")))?;
            let _ = SDK_LIBRARY.set(lib);
        }
    }
    Ok(())
}

#[cfg(windows)]
fn ensure_windows_sdk_dll_dirs(sdk_library_path: &Path) -> AppResult<()> {
    if SDK_DLL_DIRS.get().is_some() {
        return Ok(());
    }

    let lib_dir = sdk_library_path
        .parent()
        .ok_or_else(|| AppError::Sdk(String::from("invalid SDK library path: missing parent directory")))?;
    let lib_dir = std::fs::canonicalize(lib_dir)
        .map_err(|e| AppError::Sdk(format!("failed to resolve SDK lib directory: {e}")))?;
    let com_dir = lib_dir.join("HCNetSDKCom");
    prepend_path_if_needed(&lib_dir)?;
    if com_dir.exists() {
        prepend_path_if_needed(&com_dir)?;
    }

    let _ = SDK_DLL_DIRS.set(());
    Ok(())
}

#[cfg(windows)]
fn prepend_path_if_needed(dir: &Path) -> AppResult<()> {
    let current = std::env::var_os("PATH").unwrap_or_default();
    let mut entries: Vec<std::path::PathBuf> = std::env::split_paths(&current).collect();
    if entries.iter().any(|p| p == dir) {
        return Ok(());
    }
    entries.insert(0, dir.to_path_buf());
    let joined = std::env::join_paths(entries)
        .map_err(|e| AppError::Sdk(format!("failed to update PATH for SDK loading: {e}")))?;
    std::env::set_var("PATH", joined);
    Ok(())
}

fn resolve_sdk_path(custom_path: Option<&str>) -> AppResult<std::path::PathBuf> {
    if let Some(p) = custom_path {
        let root = Path::new(p);
        let candidate = sdk_library_path_from_root(root);
        if candidate.exists() {
            return Ok(candidate);
        }
    }

    Err(AppError::Sdk(format!(
        "SDK library '{}' not found. Set HIKVISION_SDK_PATH to the current platform SDK root (folder containing incEn/ and lib/).",
        sdk_library_name(),
    )))
}

fn sdk_library_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "HCNetSDK.dll"
    } else {
        "libhcnetsdk.so"
    }
}

fn sdk_library_path_from_root(root: &Path) -> PathBuf {
    root.join("lib").join(sdk_library_name())
}

pub type RealDataCallback = unsafe extern "C" fn(i32, u32, *mut u8, u32, u32);

fn api() -> &'static HCNetSdkLib {
    SDK_API.get().expect("HCNetSDK API must be initialized")
}

pub unsafe fn net_dvr_init() -> i32 {
    api().NET_DVR_Init()
}
pub unsafe fn net_dvr_cleanup() -> i32 {
    api().NET_DVR_Cleanup()
}
pub unsafe fn net_dvr_get_last_error() -> u32 {
    api().NET_DVR_GetLastError()
}
pub unsafe fn net_dvr_set_connect_time(wait_ms: u32, retries: u32) -> i32 {
    api().NET_DVR_SetConnectTime(wait_ms, retries)
}
pub unsafe fn net_dvr_login_v40(login: *mut NET_DVR_USER_LOGIN_INFO, device_info: *mut NET_DVR_DEVICEINFO_V40) -> i32 {
    api().NET_DVR_Login_V40(login, device_info)
}
pub unsafe fn net_dvr_logout(user_id: i32) -> i32 {
    api().NET_DVR_Logout(user_id)
}
pub unsafe fn net_dvr_get_sdk_version() -> u32 {
    api().NET_DVR_GetSDKVersion()
}
pub unsafe fn net_dvr_get_sdk_build_version() -> u32 {
    api().NET_DVR_GetSDKBuildVersion()
}
pub unsafe fn net_dvr_get_dvr_config(
    user_id: i32,
    command: u32,
    channel: i32,
    out: *mut c_void,
    out_len: u32,
    bytes_returned: *mut u32,
) -> i32 {
    api().NET_DVR_GetDVRConfig(user_id, command, channel, out, out_len, bytes_returned)
}
pub unsafe fn net_dvr_realplay_v40(
    user_id: i32,
    preview_info: *mut NET_DVR_PREVIEWINFO,
    cb: Option<unsafe extern "C" fn(i32, u32, *mut u8, u32, *mut c_void)>,
    user_ptr: *mut c_void,
) -> i32 {
    api().NET_DVR_RealPlay_V40(user_id, preview_info, cb, user_ptr)
}
pub unsafe fn net_dvr_stop_realplay(handle: i32) -> i32 {
    api().NET_DVR_StopRealPlay(handle)
}
pub unsafe fn net_dvr_set_standard_data_callback(
    handle: i32,
    cb: Option<RealDataCallback>,
    user: u32,
) -> i32 {
    let cb_cast: Option<unsafe extern "C" fn(i32, u32, *mut u8, u32, u32)> = cb;
    api().NET_DVR_SetStandardDataCallBack(handle, cb_cast, user)
}

