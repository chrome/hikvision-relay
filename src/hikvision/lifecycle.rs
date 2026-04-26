use std::sync::atomic::{AtomicUsize, Ordering};

use crate::hikvision::client::HikSdk;

static SDK_RUNTIME_REFCOUNT: AtomicUsize = AtomicUsize::new(0);

pub(super) fn acquire_runtime(sdk: &dyn HikSdk) -> bool {
    if SDK_RUNTIME_REFCOUNT.fetch_add(1, Ordering::AcqRel) == 0 {
        if sdk.init() == 0 {
            SDK_RUNTIME_REFCOUNT.fetch_sub(1, Ordering::AcqRel);
            return false;
        }
    }
    true
}

pub(super) fn release_runtime(sdk: &dyn HikSdk) {
    if SDK_RUNTIME_REFCOUNT.fetch_sub(1, Ordering::AcqRel) == 1 {
        let _ = sdk.cleanup();
    }
}
