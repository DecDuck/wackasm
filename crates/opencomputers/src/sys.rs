//! The raw `oc` imports (see `crates/wackasm-oc/src/abi.rs`).
//!
//! Off wasm (for `cargo test` and docs on the host) every import panics.

#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "oc")]
unsafe extern "C" {
    pub fn invoke(addr: *const u8, addr_len: usize, method: *const u8, method_len: usize, args: *const u8, args_len: usize) -> i32;
    pub fn components(filter: *const u8, filter_len: usize) -> i32;
    pub fn methods(addr: *const u8, addr_len: usize) -> i32;
    pub fn pull_signal_ms(timeout_ms: i64) -> i32;
    pub fn push_signal(sig: *const u8, sig_len: usize) -> i32;
    pub fn beep(frequency: i32, duration_ms: i32) -> i32;
    pub fn response_len() -> i32;
    pub fn response_read(dst: *mut u8, cap: usize) -> i32;
    pub fn uptime_ms() -> i64;
    pub fn address(dst: *mut u8, cap: usize) -> i32;
    pub fn memory_limit() -> i64;
    pub fn log(msg: *const u8, len: usize);
    pub fn spawn_with(spec: *const u8, spec_len: usize) -> i32;
    pub fn resume(input: *const u8, input_len: usize) -> i32;
    pub fn kill() -> i32;
    pub fn files_disk() -> i32;
    pub fn shutdown(reboot: i32) -> !;
    pub fn crash(msg: *const u8, len: usize) -> !;
}

#[cfg(not(target_arch = "wasm32"))]
mod host {
    fn unavailable() -> ! {
        panic!("OpenComputers imports are only available inside a wackasm guest")
    }
    pub unsafe fn invoke(_: *const u8, _: usize, _: *const u8, _: usize, _: *const u8, _: usize) -> i32 { unavailable() }
    pub unsafe fn components(_: *const u8, _: usize) -> i32 { unavailable() }
    pub unsafe fn methods(_: *const u8, _: usize) -> i32 { unavailable() }
    pub unsafe fn pull_signal_ms(_: i64) -> i32 { unavailable() }
    pub unsafe fn push_signal(_: *const u8, _: usize) -> i32 { unavailable() }
    pub unsafe fn beep(_: i32, _: i32) -> i32 { unavailable() }
    pub unsafe fn response_len() -> i32 { unavailable() }
    pub unsafe fn response_read(_: *mut u8, _: usize) -> i32 { unavailable() }
    pub unsafe fn uptime_ms() -> i64 { unavailable() }
    pub unsafe fn address(_: *mut u8, _: usize) -> i32 { unavailable() }
    pub unsafe fn memory_limit() -> i64 { unavailable() }
    pub unsafe fn log(_: *const u8, _: usize) { unavailable() }
    pub unsafe fn spawn_with(_: *const u8, _: usize) -> i32 { unavailable() }
    pub unsafe fn resume(_: *const u8, _: usize) -> i32 { unavailable() }
    pub unsafe fn kill() -> i32 { unavailable() }
    pub unsafe fn files_disk() -> i32 { unavailable() }
    pub unsafe fn shutdown(_: i32) -> ! { unavailable() }
    pub unsafe fn crash(_: *const u8, _: usize) -> ! { unavailable() }
}
#[cfg(not(target_arch = "wasm32"))]
pub use host::*;

pub const STATUS_OK: i32 = 0;
pub const STATUS_NO_SUCH_COMPONENT: i32 = -3;
pub const STATUS_NO_SUCH_METHOD: i32 = -4;
pub const STATUS_NOT_FOUND: i32 = -5;

// Process events, as `spawn_with` and `resume` return them.
pub const EVENT_EXITED: i32 = 0;
pub const EVENT_OUTPUT: i32 = 1;
pub const EVENT_INPUT: i32 = 2;
pub const EVENT_CRASHED: i32 = 3;

/// The bytes of the response buffer left by the last host call.
pub fn response() -> Vec<u8> {
    let len = unsafe { response_len() }.max(0) as usize;
    let mut buf = vec![0u8; len];
    let n = unsafe { response_read(buf.as_mut_ptr(), len) }.max(0) as usize;
    buf.truncate(n);
    buf
}
