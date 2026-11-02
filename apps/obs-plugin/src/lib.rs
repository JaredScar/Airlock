//! C ABI for the OBS audio filter. The delay itself is `SampleDelay`.

use airlock_core::SampleDelay;

/// # Safety
/// The returned pointer is owned by the caller and freed with `airlock_delay_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn airlock_delay_new(delay_frames: u32, channels: u32) -> *mut SampleDelay {
    Box::into_raw(Box::new(SampleDelay::new(
        delay_frames as usize,
        channels as usize,
    )))
}

/// # Safety
/// `ptr` is null or a pointer returned by `airlock_delay_new` that has not been freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn airlock_delay_free(ptr: *mut SampleDelay) {
    if ptr.is_null() {
        return;
    }
    drop(unsafe { Box::from_raw(ptr) });
}

/// # Safety
/// `ptr` is null or a live pointer from `airlock_delay_new`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn airlock_delay_set(ptr: *mut SampleDelay, delay_frames: u32) {
    if ptr.is_null() {
        return;
    }
    unsafe { (*ptr).set_delay_frames(delay_frames as usize) };
}

/// # Safety
/// `ptr` is null or a live pointer from `airlock_delay_new`. `samples` points at
/// `frames * channels` interleaved `f32` values, and `frames` is the frame count.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn airlock_delay_process(
    ptr: *mut SampleDelay,
    samples: *mut f32,
    frames: u32,
) {
    if ptr.is_null() || samples.is_null() || frames == 0 {
        return;
    }
    let channels = unsafe { (*ptr).channels() };
    let Some(n) = (frames as usize).checked_mul(channels) else {
        return;
    };
    let buf = unsafe { std::slice::from_raw_parts_mut(samples, n) };
    unsafe { (*ptr).process_in_place(buf) };
}

/// # Safety
/// `ptr` is null or a live pointer from `airlock_delay_new`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn airlock_delay_dump(ptr: *mut SampleDelay) {
    if ptr.is_null() {
        return;
    }
    unsafe { (*ptr).dump() };
}
