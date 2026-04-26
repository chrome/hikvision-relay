use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;

use once_cell::sync::Lazy;
use parking_lot::Mutex;

use std::ffi::c_void;

use crate::hikvision::ffi;

pub type CallbackFn = Arc<dyn Fn(u32, Vec<u8>) + Send + Sync + 'static>;

static CALLBACK_REGISTRY: Lazy<Mutex<HashMap<ffi::LONG, CallbackFn>>> = Lazy::new(|| Mutex::new(HashMap::new()));

unsafe extern "C" fn raw_cb(handle: ffi::LONG, data_type: ffi::DWORD, ptr: *mut u8, len: ffi::DWORD, _user: ffi::DWORD) {
    let chunk = if ptr.is_null() || len == 0 {
        Vec::new()
    } else {
        // SAFETY: SDK guarantees `ptr` for callback length `len`.
        unsafe { std::slice::from_raw_parts(ptr, len as usize).to_vec() }
    };
    let handler = CALLBACK_REGISTRY.lock().get(&handle).cloned();
    if let Some(handler) = handler {
        if catch_unwind(AssertUnwindSafe(|| (handler)(data_type as u32, chunk))).is_err() {
            crate::log_step!("sdk", "callback_panicked", "handle={handle} dataType={data_type}");
        }
    }
}

unsafe extern "C" fn raw_cb_v40(
    handle: ffi::LONG,
    data_type: ffi::DWORD,
    ptr: *mut u8,
    len: ffi::DWORD,
    _user_ptr: *mut c_void,
) {
    let chunk = if ptr.is_null() || len == 0 {
        Vec::new()
    } else {
        // SAFETY: SDK guarantees `ptr` for callback length `len`.
        unsafe { std::slice::from_raw_parts(ptr, len as usize).to_vec() }
    };
    let handler = CALLBACK_REGISTRY.lock().get(&handle).cloned();
    if let Some(handler) = handler {
        if catch_unwind(AssertUnwindSafe(|| (handler)(data_type as u32, chunk))).is_err() {
            crate::log_step!("sdk", "callback_panicked", "handle={handle} dataType={data_type}");
        }
    }
}

pub(super) fn callback_entrypoint() -> ffi::RealDataCallback {
    raw_cb
}

pub(super) fn callback_entrypoint_v40() -> unsafe extern "C" fn(ffi::LONG, ffi::DWORD, *mut u8, ffi::DWORD, *mut c_void) {
    raw_cb_v40
}

pub(super) fn register_callback(handle: ffi::LONG, cb: CallbackFn) {
    CALLBACK_REGISTRY.lock().insert(handle, cb);
}

pub(super) fn unregister_callback(handle: ffi::LONG) {
    CALLBACK_REGISTRY.lock().remove(&handle);
}

