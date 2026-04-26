use std::ffi::c_void;
use std::ptr::{addr_of, addr_of_mut};
use serde::Serialize;

use crate::config::{ConnectionConfig, StreamType};
use crate::error::{AppError, AppResult};
use crate::hikvision::ffi;
use crate::hikvision::lifecycle::{acquire_runtime, release_runtime};
pub use crate::hikvision::preview::CallbackFn;
use crate::hikvision::preview::{callback_entrypoint, register_callback, unregister_callback};

pub const NET_DVR_IPPARACFG_V31_SIZE: usize = std::mem::size_of::<ffi::NET_DVR_IPPARACFG_V31>();

#[derive(Debug, Clone, Serialize)]
pub struct SdkMeta {
    pub sdk_version_raw: u32,
    pub sdk_build_version_raw: u32,
    pub sdk_version_hex: String,
    pub sdk_build_version_hex: String,
}

#[derive(Debug, Clone, Copy)]
pub enum IpConfigVersion {
    V40,
    V31,
    Legacy,
}

#[derive(Debug, Clone)]
pub struct IpConfigRaw {
    pub version: IpConfigVersion,
    pub data: Vec<u8>,
    pub bytes_returned: usize,
}

pub trait HikSdk: Send + Sync {
    fn set_connect_time(&self, wait_ms: u32, retries: u32) -> i32;
    fn init(&self) -> i32;
    fn cleanup(&self) -> i32;
    fn get_last_error(&self) -> u32;
    fn login_v40(
        &self,
        login: *mut ffi::NET_DVR_USER_LOGIN_INFO,
        device_info: *mut ffi::NET_DVR_DEVICEINFO_V40,
    ) -> ffi::LONG;
    fn logout(&self, user_id: ffi::LONG) -> i32;
    fn get_sdk_version(&self) -> u32;
    fn get_sdk_build_version(&self) -> u32;
    fn get_dvr_config(
        &self,
        user_id: ffi::LONG,
        command: u32,
        channel: ffi::LONG,
        out: *mut c_void,
        out_len: u32,
        bytes_returned: *mut u32,
    ) -> i32;
    fn realplay_v40(
        &self,
        user_id: ffi::LONG,
        preview_info: *mut ffi::NET_DVR_PREVIEWINFO,
        cb: Option<unsafe extern "C" fn(ffi::LONG, ffi::DWORD, *mut u8, ffi::DWORD, *mut c_void)>,
        user_ptr: *mut c_void,
    ) -> ffi::LONG;
    fn stop_realplay(&self, handle: ffi::LONG) -> i32;
    fn set_standard_data_callback(
        &self,
        handle: ffi::LONG,
        cb: Option<ffi::RealDataCallback>,
        user: u32,
    ) -> i32;
}

struct RealHikSdk;

impl HikSdk for RealHikSdk {
    fn set_connect_time(&self, wait_ms: u32, retries: u32) -> i32 {
        // SAFETY: direct SDK call.
        unsafe { ffi::net_dvr_set_connect_time(wait_ms, retries) }
    }
    fn init(&self) -> i32 {
        // SAFETY: direct SDK call.
        unsafe { ffi::net_dvr_init() }
    }
    fn cleanup(&self) -> i32 {
        // SAFETY: direct SDK call.
        unsafe { ffi::net_dvr_cleanup() }
    }
    fn get_last_error(&self) -> u32 {
        // SAFETY: direct SDK call.
        unsafe { ffi::net_dvr_get_last_error() }
    }
    fn login_v40(
        &self,
        login: *mut ffi::NET_DVR_USER_LOGIN_INFO,
        device_info: *mut ffi::NET_DVR_DEVICEINFO_V40,
    ) -> ffi::LONG {
        // SAFETY: pointers are owned by caller.
        unsafe { ffi::net_dvr_login_v40(login, device_info) }
    }
    fn logout(&self, user_id: ffi::LONG) -> i32 {
        // SAFETY: direct SDK call.
        unsafe { ffi::net_dvr_logout(user_id) }
    }
    fn get_sdk_version(&self) -> u32 {
        // SAFETY: direct SDK call.
        unsafe { ffi::net_dvr_get_sdk_version() }
    }
    fn get_sdk_build_version(&self) -> u32 {
        // SAFETY: direct SDK call.
        unsafe { ffi::net_dvr_get_sdk_build_version() }
    }
    fn get_dvr_config(
        &self,
        user_id: ffi::LONG,
        command: u32,
        channel: ffi::LONG,
        out: *mut c_void,
        out_len: u32,
        bytes_returned: *mut u32,
    ) -> i32 {
        // SAFETY: pointers are owned by caller.
        unsafe { ffi::net_dvr_get_dvr_config(user_id, command, channel, out, out_len, bytes_returned) }
    }
    fn realplay_v40(
        &self,
        user_id: ffi::LONG,
        preview_info: *mut ffi::NET_DVR_PREVIEWINFO,
        cb: Option<unsafe extern "C" fn(ffi::LONG, ffi::DWORD, *mut u8, ffi::DWORD, *mut c_void)>,
        user_ptr: *mut c_void,
    ) -> ffi::LONG {
        // SAFETY: pointers are owned by caller.
        unsafe { ffi::net_dvr_realplay_v40(user_id, preview_info, cb, user_ptr) }
    }
    fn stop_realplay(&self, handle: ffi::LONG) -> i32 {
        // SAFETY: direct SDK call.
        unsafe { ffi::net_dvr_stop_realplay(handle) }
    }
    fn set_standard_data_callback(
        &self,
        handle: ffi::LONG,
        cb: Option<ffi::RealDataCallback>,
        user: u32,
    ) -> i32 {
        // SAFETY: direct SDK call.
        unsafe { ffi::net_dvr_set_standard_data_callback(handle, cb, user) }
    }
}

static REAL_SDK: RealHikSdk = RealHikSdk;
fn sdk_code_error(context: impl Into<String>, code: u32) -> AppError {
    AppError::SdkCode {
        code,
        context: context.into(),
    }
}

pub struct HikvisionClient {
    sdk: &'static dyn HikSdk,
    user_id: ffi::LONG,
    real_handle: ffi::LONG,
    initialized: bool,
    /// First digital channel index from `NET_DVR_DEVICEINFO_V40` (byte offset 66), default 1.
    start_digital_channel: u8,
}

impl HikvisionClient {
    pub fn new(sdk_root_path: Option<&str>) -> AppResult<Self> {
        ffi::ensure_hcnetsdk_loaded(sdk_root_path)?;
        Ok(Self::with_sdk(&REAL_SDK))
    }

    fn with_sdk(sdk: &'static dyn HikSdk) -> Self {
        Self {
            sdk,
            user_id: -1,
            real_handle: -1,
            initialized: false,
            start_digital_channel: 1,
        }
    }

    /// Digital channel base from last successful `login()` (matches Node `startDigitalChannel`).
    pub fn start_digital_channel(&self) -> u8 {
        self.start_digital_channel.max(1)
    }

    pub fn last_error(&self) -> u32 {
        self.sdk.get_last_error()
    }

    pub fn init(&mut self) -> AppResult<()> {
        if self.initialized {
            return Ok(());
        }
        if !acquire_runtime(self.sdk) {
            return Err(sdk_code_error("NET_DVR_Init failed", self.last_error()));
        }
        self.initialized = true;
        Ok(())
    }

    pub fn login(&mut self, conn: &ConnectionConfig) -> AppResult<()> {
        if !self.initialized {
            return Err(AppError::Sdk(String::from("SDK is not initialized")));
        }
        if self.sdk.set_connect_time(5000, 2) == 0 {
            crate::log_step!(
                "sdk",
                "set_connect_time_failed",
                "errorCode={}",
                self.last_error()
            );
        }
        // SAFETY: zero-init POD C structs and fill fields.
        let mut login_info: ffi::NET_DVR_USER_LOGIN_INFO = unsafe { std::mem::zeroed() };
        let mut dev_info: ffi::NET_DVR_DEVICEINFO_V40 = unsafe { std::mem::zeroed() };

        write_cstr(&mut login_info.sDeviceAddress, &conn.ip);
        write_cstr(&mut login_info.sUserName, &conn.user);
        write_cstr(&mut login_info.sPassword, &conn.password);
        login_info.wPort = conn.port;
        login_info.bUseAsynLogin = 0;

        let uid = self
            .sdk
            .login_v40(addr_of_mut!(login_info), addr_of_mut!(dev_info));
        if uid < 0 {
            return Err(sdk_code_error("NET_DVR_Login_V40 failed", self.last_error()));
        }
        self.user_id = uid;
        // Match Node `parseDeviceInfoV40`: `byStartDChan` at offset 66 in the V40 device info blob.
        let dev_slice = unsafe {
            std::slice::from_raw_parts(
                addr_of!(dev_info).cast::<u8>(),
                std::mem::size_of::<ffi::NET_DVR_DEVICEINFO_V40>(),
            )
        };
        self.start_digital_channel = dev_slice.get(66).copied().unwrap_or(1).max(1);
        Ok(())
    }

    pub fn get_sdk_meta(&self) -> SdkMeta {
        // SAFETY: pure SDK queries.
        let v = self.sdk.get_sdk_version();
        let b = self.sdk.get_sdk_build_version();
        SdkMeta {
            sdk_version_raw: v,
            sdk_build_version_raw: b,
            sdk_version_hex: format!("0x{v:x}"),
            sdk_build_version_hex: format!("0x{b:x}"),
        }
    }

    pub fn get_ip_config_with_fallback(&self) -> AppResult<IpConfigRaw> {
        let try_read = |command: u32, size: usize| -> Result<Option<IpConfigRaw>, AppError> {
            let mut out = vec![0_u8; size];
            if size >= 4 {
                out[0..4].copy_from_slice(&(size as u32).to_le_bytes());
            }
            let mut returned: u32 = 0;
            let ok = self.sdk.get_dvr_config(
                self.user_id,
                command,
                0,
                out.as_mut_ptr().cast::<c_void>(),
                size as u32,
                &mut returned as *mut u32,
            );
            if ok == 0 {
                if self.last_error() == ffi::NET_DVR_PARAMETER_ERROR {
                    return Ok(None);
                }
                return Err(sdk_code_error(
                    format!("NET_DVR_GetDVRConfig command={command} failed"),
                    self.last_error(),
                ));
            }
            let bytes_returned = returned as usize;
            if bytes_returned > out.len() {
                return Err(AppError::Sdk(format!(
                    "NET_DVR_GetDVRConfig returned invalid length: {bytes_returned} > {}",
                    out.len()
                )));
            }
            out.truncate(bytes_returned);
            Ok(Some(IpConfigRaw {
                version: match command {
                    ffi::NET_DVR_GET_IPPARACFG_V40 => IpConfigVersion::V40,
                    ffi::NET_DVR_GET_IPPARACFG_V31 => IpConfigVersion::V31,
                    _ => IpConfigVersion::Legacy,
                },
                data: out,
                bytes_returned,
            }))
        };

        if let Some(v40) = try_read(ffi::NET_DVR_GET_IPPARACFG_V40, std::mem::size_of::<ffi::NET_DVR_IPPARACFG_V40>())? {
            return Ok(v40);
        }
        if let Some(v31) = try_read(ffi::NET_DVR_GET_IPPARACFG_V31, NET_DVR_IPPARACFG_V31_SIZE)? {
            return Ok(v31);
        }
        if let Some(legacy) = try_read(ffi::NET_DVR_GET_IPPARACFG, NET_DVR_IPPARACFG_V31_SIZE)? {
            return Ok(legacy);
        }
        Err(AppError::Sdk(String::from("All NET_DVR_GetDVRConfig fallbacks failed")))
    }

    pub fn start_preview_and_attach_callback(
        &mut self,
        channel: i32,
        stream: StreamType,
        cb: CallbackFn,
    ) -> AppResult<()> {
        if self.user_id < 0 {
            return Err(AppError::Sdk(String::from("Not logged in")));
        }
        if self.real_handle >= 0 {
            return Err(AppError::Sdk(String::from("Preview already running")));
        }
        let mut preview_info: ffi::NET_DVR_PREVIEWINFO = unsafe { std::mem::zeroed() };
        preview_info.lChannel = channel as ffi::LONG;
        preview_info.dwStreamType = if stream == StreamType::Sub { 1 } else { 0 };
        preview_info.dwLinkMode = 0;
        preview_info.bBlocked = 1;
        preview_info.dwDisplayBufNum = 1;

        crate::log_step!(
            "sdk",
            "starting_preview",
            "channel={} streamType={}",
            channel,
            if stream == StreamType::Sub { "sub" } else { "main" }
        );
        let h = self
            .sdk
            .realplay_v40(self.user_id, addr_of_mut!(preview_info), None, std::ptr::null_mut());
        if h < 0 {
            return Err(sdk_code_error("NET_DVR_RealPlay_V40 failed", self.last_error()));
        }
        self.real_handle = h;
        crate::log_step!("sdk", "preview_started", "realHandle={}", self.real_handle);
        register_callback(self.real_handle, cb);
        let ok = self
            .sdk
            .set_standard_data_callback(self.real_handle, Some(callback_entrypoint()), 0);
        if ok == 0 {
            unregister_callback(self.real_handle);
            let _ = self.sdk.stop_realplay(self.real_handle);
            self.real_handle = -1;
            return Err(sdk_code_error(
                "NET_DVR_SetStandardDataCallBack failed",
                self.last_error(),
            ));
        }
        crate::log_step!("sdk", "standard_callback_attached");
        Ok(())
    }

    pub fn stop_preview(&mut self) {
        if self.real_handle >= 0 {
            unregister_callback(self.real_handle);
            let _ = self.sdk.stop_realplay(self.real_handle);
            self.real_handle = -1;
        }
    }

    pub fn logout(&mut self) {
        if self.user_id >= 0 {
            let _ = self.sdk.logout(self.user_id);
            self.user_id = -1;
        }
    }

    pub fn cleanup(&mut self) {
        if self.initialized {
            release_runtime(self.sdk);
            self.initialized = false;
        }
    }
}

impl Drop for HikvisionClient {
    fn drop(&mut self) {
        self.stop_preview();
        self.logout();
        self.cleanup();
    }
}

trait CChar: Sized {
    fn from_u8(b: u8) -> Self;
    fn zero() -> Self {
        Self::from_u8(0)
    }
}

impl CChar for i8 {
    fn from_u8(b: u8) -> Self {
        b as i8
    }
}

impl CChar for u8 {
    fn from_u8(b: u8) -> Self {
        b
    }
}

fn write_cstr<T: CChar + Copy>(target: &mut [T], value: &str) {
    target.fill(T::zero());
    if target.is_empty() {
        return;
    }
    let bytes = value.as_bytes();
    let take = bytes.len().min(target.len() - 1);
    for i in 0..take {
        target[i] = T::from_u8(bytes[i]);
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RelayStats {
    pub pipeline_start_count: usize,
    pub pipeline_stop_count: usize,
    pub sdk_chunks: usize,
    pub sdk_bytes: usize,
    pub rtp_packets: usize,
    pub nals_emitted: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_sdk_error: Option<String>,
}


