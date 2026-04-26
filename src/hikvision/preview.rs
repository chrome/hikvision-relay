use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;

use once_cell::sync::Lazy;
use parking_lot::Mutex;

use crate::hikvision::ffi;

pub type CallbackFn = Arc<dyn Fn(u32, Vec<u8>) + Send + Sync + 'static>;

static CALLBACK_REGISTRY: Lazy<Mutex<HashMap<i32, CallbackFn>>> = Lazy::new(|| Mutex::new(HashMap::new()));

unsafe extern "C" fn raw_cb(handle: i32, data_type: u32, ptr: *mut u8, len: u32, _user: u32) {
    let chunk = if ptr.is_null() || len == 0 {
        Vec::new()
    } else {
        // SAFETY: SDK guarantees `ptr` for callback length `len`.
        unsafe { std::slice::from_raw_parts(ptr, len as usize).to_vec() }
    };
    let handler = CALLBACK_REGISTRY.lock().get(&handle).cloned();
    if let Some(handler) = handler {
        if catch_unwind(AssertUnwindSafe(|| (handler)(data_type, chunk))).is_err() {
            crate::log_step!("sdk", "callback_panicked", "handle={handle} dataType={data_type}");
        }
    }
}

pub(super) fn callback_entrypoint() -> ffi::RealDataCallback {
    raw_cb
}

pub(super) fn register_callback(handle: i32, cb: CallbackFn) {
    CALLBACK_REGISTRY.lock().insert(handle, cb);
}

pub(super) fn unregister_callback(handle: i32) {
    CALLBACK_REGISTRY.lock().remove(&handle);
}

