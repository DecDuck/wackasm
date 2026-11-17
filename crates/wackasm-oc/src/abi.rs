//! Constants shared across the three boundaries:
//!
//! * guest ⇄ Rust: the `oc` import module and the status codes its functions
//!   return to the guest;
//! * Rust ⇄ Java: the codes `run()` returns, which tell the Java architecture
//!   what the machine wants next.
//!
//! `WackasmNative.java` mirrors the `RUN_*` and `STATUS_*` values.

/// Import module name guests link against.
pub const IMPORT_MODULE: &str = "oc";

/// Exports tried, in order, as the program entry point.
pub const ENTRY_POINTS: &[&str] = &["oc_main", "_start"];

/// Magic bytes at the start of every wasm binary.
pub const WASM_MAGIC: &[u8; 4] = b"\0asm";

// ---------------------------------------------------------------------------
// Rust → Java: results of `run()`.
// ---------------------------------------------------------------------------

/// Fuel for this slice ran out. Java returns `Sleep(0)`.
pub const RUN_YIELD: i32 = 0;
/// The entry point returned. Java returns `Shutdown(false)`.
pub const RUN_HALTED: i32 = 1;
/// The guest asked to shut down. Payload: `[reboot: bool]`.
pub const RUN_SHUTDOWN: i32 = 2;
/// The guest trapped or crashed. Payload: `[message: string]`.
pub const RUN_ERROR: i32 = 3;
/// Component call. Payload: `[address, method, args: array]`.
/// Java runs it inline if the callback is direct, otherwise from
/// `runSynchronized`, then calls `respond`.
pub const RUN_INVOKE: i32 = 4;
/// The guest is waiting for a signal. Payload: `[remaining_seconds | nil]`
/// (nil means wait forever). Java pops a signal and responds with it, responds
/// with `STATUS_TIMEOUT` once the remaining time is `<= 0`, or otherwise
/// sleeps without responding.
pub const RUN_PULL_SIGNAL: i32 = 5;
/// Queue a signal on this machine. Payload: `[name, args...]`.
pub const RUN_PUSH_SIGNAL: i32 = 6;
/// List components. Payload: `[filter: string]`. Response: map address → type.
pub const RUN_COMPONENTS: i32 = 7;
/// List a component's methods. Payload: `[address]`.
/// Response: map method → `{direct: bool, doc: string}`.
pub const RUN_METHODS: i32 = 8;
/// Beep. Payload: `[frequency: int, duration_ms: int]`.
pub const RUN_BEEP: i32 = 9;

// ---------------------------------------------------------------------------
// Status codes: what Java passes to `respond()`, and what host-backed imports
// return to the guest. The response buffer holds the encoded result on
// success, or a UTF-8 error message on failure.
// ---------------------------------------------------------------------------

pub const STATUS_OK: i32 = 0;
/// `pull_signal` timed out. The response buffer is empty.
pub const STATUS_TIMEOUT: i32 = 1;
/// Generic failure; the response buffer holds the message.
pub const STATUS_ERROR: i32 = -1;
/// The guest passed malformed data (bad pointer, UTF-8, or encoding).
pub const STATUS_BAD_ARGUMENT: i32 = -2;
pub const STATUS_NO_SUCH_COMPONENT: i32 = -3;
pub const STATUS_NO_SUCH_METHOD: i32 = -4;

/// Every function in the `oc` import module. The signature comments use wasm
/// types; `ptr`/`len` are `i32` offsets into memory 0.
pub mod imports {
    /// `(addr_ptr, addr_len, method_ptr, method_len, args_ptr, args_len) -> status`.
    /// `args` is an encoded array. On success the response is an encoded array of results.
    pub const INVOKE: &str = "invoke";
    /// `(timeout_seconds: f64) -> status`. A negative timeout waits forever.
    /// On `STATUS_OK` the response is an encoded array `[name, args...]`.
    pub const PULL_SIGNAL: &str = "pull_signal";
    /// `(signal_ptr, signal_len) -> status`; the signal is an encoded array `[name, args...]`.
    pub const PUSH_SIGNAL: &str = "push_signal";
    /// `(filter_ptr, filter_len) -> status`; response is an encoded map.
    pub const COMPONENTS: &str = "components";
    /// `(addr_ptr, addr_len) -> status`; response is an encoded map.
    pub const METHODS: &str = "methods";
    /// `(frequency: i32, duration_ms: i32) -> status`.
    pub const BEEP: &str = "beep";

    // Handled entirely in Rust, with no trip to Java:

    /// `() -> len`: byte length of the current response buffer.
    pub const RESPONSE_LEN: &str = "response_len";
    /// `(dst_ptr, dst_len) -> copied`: copies the response buffer into guest memory.
    pub const RESPONSE_READ: &str = "response_read";
    /// `() -> f64`: machine uptime in seconds, as of the start of this slice.
    pub const UPTIME: &str = "uptime";
    /// `(dst_ptr, dst_len) -> len`: writes this computer's address (UTF-8) and
    /// returns its full length.
    pub const ADDRESS: &str = "address";
    /// `() -> i64`: bytes of linear memory the guest may use.
    pub const MEMORY_LIMIT: &str = "memory_limit";
    /// `(msg_ptr, msg_len) -> ()`: appends a line to the machine's log file in the work dir.
    pub const LOG: &str = "log";
    /// `(reboot: i32) -> ()`: halts execution. `run()` then reports `RUN_SHUTDOWN`.
    pub const SHUTDOWN: &str = "shutdown";
    /// `(msg_ptr, msg_len) -> ()`: halts with an error. `run()` then reports `RUN_ERROR`.
    pub const CRASH: &str = "crash";
}
