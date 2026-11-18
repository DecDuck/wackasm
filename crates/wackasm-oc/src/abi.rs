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
/// A WASI process needs a disk for its files. Payload: `[]`. Response:
/// `[address]` of the filesystem to use, or `[]` if there is none. Java picks
/// the first usable disk: the boot disk if writable, else the first writable
/// filesystem, else any. It never picks the computer's tmpfs.
pub const RUN_DEFAULT_DISK: i32 = 10;
/// Method call on a userdata value (an object a component returned, such as
/// the internet card's HTTP request). Payload: `[value: userdata, method,
/// args: array]`. Java calls it through `Machine.invoke(Value, ...)`, inline
/// if the callback is direct, otherwise from `runSynchronized`, then calls
/// `respond`. An unknown value is `STATUS_NO_SUCH_COMPONENT`.
pub const RUN_INVOKE_VALUE: i32 = 11;
/// The guest is done with a userdata value. Payload: `[value: userdata]`.
/// Java disposes of it (closing a socket, for example), forgets its id and
/// responds `STATUS_OK`.
pub const RUN_RELEASE_VALUE: i32 = 12;

// ---------------------------------------------------------------------------
// Process events: what `spawn` and `resume` return to the parent. The
// parent's response buffer holds the payload as an encoded array.
// ---------------------------------------------------------------------------

/// The child finished. Payload: `[exit_code, last_output: bytes]`. The code
/// is 0 when `_start` returned.
pub const EVENT_EXITED: i32 = 0;
/// The child wrote to stdout or stderr. Payload: `[bytes]`.
pub const EVENT_OUTPUT: i32 = 1;
/// The child is blocked reading stdin. Payload: `[]`. `resume` with a line,
/// or with nothing to signal end of file.
pub const EVENT_INPUT: i32 = 2;
/// The child trapped. Payload: `[message, last_output: bytes]`. It is gone.
pub const EVENT_CRASHED: i32 = 3;

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
/// `spawn_file`: there is no such file.
pub const STATUS_NOT_FOUND: i32 = -5;

/// Every function in the `oc` import module. The signature comments use wasm
/// types; `ptr`/`len` are `i32` offsets into memory 0.
pub mod imports {
    /// `(addr_ptr, addr_len, method_ptr, method_len, args_ptr, args_len) -> status`.
    /// `args` is an encoded array. On success the response is an encoded array of results.
    pub const INVOKE: &str = "invoke";
    /// `(timeout_seconds: f64) -> status`. A negative timeout waits forever.
    /// On `STATUS_OK` the response is an encoded array `[name, args...]`.
    pub const PULL_SIGNAL: &str = "pull_signal";
    /// `(timeout_ms: i64) -> status`: `pull_signal` with an integer timeout
    /// in milliseconds (negative waits forever), for guests that avoid floats.
    pub const PULL_SIGNAL_MS: &str = "pull_signal_ms";
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
    /// `() -> i64`: machine uptime in milliseconds.
    pub const UPTIME_MS: &str = "uptime_ms";
    /// `(dst_ptr, dst_len) -> len`: writes this computer's address (UTF-8) and
    /// returns its full length.
    pub const ADDRESS: &str = "address";
    /// `() -> i64`: bytes of linear memory the guest may use.
    pub const MEMORY_LIMIT: &str = "memory_limit";
    /// `(msg_ptr, msg_len) -> ()`: appends a line to the machine's log file in the work dir.
    pub const LOG: &str = "log";
    /// `(image_ptr, image_len) -> status`: replaces the running program with
    /// the wasm binary at `image`, under the same memory limit, and starts its
    /// entry point. It does not return on success. On failure it returns
    /// `STATUS_ERROR` with the reason in the response buffer, and the caller
    /// keeps running. This is how the BIOS hands over to `/init.wasm`.
    pub const EXEC: &str = "exec";
    /// `(image, image_len, argv, argv_len) -> event | status`: starts the wasm
    /// binary at `image` as a child process with `argv` (an encoded array of
    /// strings, `argv[0]` being the program name) and runs it until its first
    /// event. The child shares this machine's RAM. Fails with `STATUS_ERROR`
    /// (reason in the response) if the image won't load.
    pub const SPAWN: &str = "spawn";
    /// `(fs, fs_len, path, path_len, argv, argv_len) -> event | status`: like
    /// `spawn`, but the host loads `path` from the filesystem component `fs`.
    /// Compiled programs are cached server-wide by disk, path, size and
    /// modification time, so running a program again skips reading and
    /// compiling it. The image never passes through the caller's memory.
    /// `STATUS_NOT_FOUND` if there is no such file.
    pub const SPAWN_FILE: &str = "spawn_file";
    /// `(input, input_len) -> event | status`: continues the paused child.
    /// The input is appended to its stdin. An empty input answers `EVENT_INPUT`
    /// with end of file. `STATUS_ERROR` if there is no child.
    pub const RESUME: &str = "resume";
    /// `(spec, spec_len) -> event | status`: the general spawn. `spec` is an
    /// encoded map:
    /// * `path`: a wasm file to load through the module cache, from `disk` (a
    ///   filesystem address), or from the caller's own WASI disk if no `disk`
    ///   is given; or `image`: the wasm binary itself, as bytes;
    /// * `argv`: array of strings (default `[path]`);
    /// * `env`: array of `NAME=value` strings, or a map of names to values
    ///   (default: the caller's environment);
    /// * `cwd`: the child's working directory, which it gets as `PWD`
    ///   (default: the `PWD` in its environment).
    ///
    /// A relative `cwd` starts at the caller's `PWD`, and a relative `path`
    /// at the child's working directory. The child's WASI disk is the
    /// caller's. Returns the first event, like
    /// `spawn`, or `STATUS_NOT_FOUND`, `STATUS_ERROR` or `STATUS_BAD_ARGUMENT`.
    pub const SPAWN_WITH: &str = "spawn_with";
    /// `(value: u32, method, method_len, args, args_len) -> status`: like
    /// `invoke`, but calls a method on a userdata value (`value` is its id, as
    /// the codec carries it). The response holds the encoded results.
    pub const INVOKE_VALUE: &str = "invoke_value";
    /// `(value: u32) -> status`: disposes of a userdata value and forgets it.
    /// Values are otherwise kept until the machine stops.
    pub const RELEASE_VALUE: &str = "release_value";
    /// `() -> status`: the address of the filesystem this process's WASI
    /// calls use (the disk it sees as `/`), in the response buffer.
    /// `STATUS_NOT_FOUND` if there is none. Picking it is the same as on a
    /// program's first file access: the boot disk if writable, else the first
    /// writable disk, else a read-only one. Children share their parent's.
    pub const FILES_DISK: &str = "files_disk";
    /// `() -> status`: ends the child (and any of its children) immediately.
    pub const KILL: &str = "kill";
    /// `(reboot: i32) -> ()`: halts execution. `run()` then reports `RUN_SHUTDOWN`.
    pub const SHUTDOWN: &str = "shutdown";
    /// `(msg_ptr, msg_len) -> ()`: halts with an error. `run()` then reports `RUN_ERROR`.
    pub const CRASH: &str = "crash";
}
