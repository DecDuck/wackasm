//! Safe wrappers over the `oc` import module (see `crates/wackasm-oc/src/abi.rs`).

use crate::value::OcValue;

mod sys {
    #[link(wasm_import_module = "oc")]
    unsafe extern "C" {
        pub fn invoke(addr: *const u8, addr_len: usize, method: *const u8, method_len: usize, args: *const u8, args_len: usize) -> i32;
        pub fn pull_signal(timeout: f64) -> i32;
        pub fn push_signal(sig: *const u8, sig_len: usize) -> i32;
        pub fn components(filter: *const u8, filter_len: usize) -> i32;
        pub fn beep(frequency: i32, duration_ms: i32) -> i32;
        pub fn response_len() -> i32;
        pub fn response_read(dst: *mut u8, cap: usize) -> i32;
        pub fn uptime() -> f64;
        pub fn address(dst: *mut u8, cap: usize) -> i32;
        pub fn memory_limit() -> i64;
        pub fn log(msg: *const u8, len: usize);
        pub fn spawn(image: *const u8, image_len: usize, argv: *const u8, argv_len: usize) -> i32;
        pub fn spawn_with(spec: *const u8, spec_len: usize) -> i32;
        pub fn resume(input: *const u8, input_len: usize) -> i32;
        pub fn kill() -> i32;
        pub fn files_disk() -> i32;
        pub fn shutdown(reboot: i32) -> !;
        pub fn crash(msg: *const u8, len: usize) -> !;
    }
}

pub const STATUS_OK: i32 = 0;
pub const STATUS_TIMEOUT: i32 = 1;

fn response() -> Vec<u8> {
    let len = unsafe { sys::response_len() } as usize;
    let mut buf = vec![0u8; len];
    unsafe { sys::response_read(buf.as_mut_ptr(), len) };
    buf
}

fn response_value() -> OcValue {
    let bytes = response();
    if bytes.is_empty() {
        return OcValue::Nil;
    }
    OcValue::decode(&bytes).unwrap_or_else(|e| crash(&format!("bad response from host: {e}")))
}

fn response_error(status: i32) -> String {
    format!("status {status}: {}", String::from_utf8_lossy(&response()))
}

/// Calls `method` on the component at `address`.
pub fn invoke(address: &str, method: &str, args: Vec<OcValue>) -> Result<Vec<OcValue>, String> {
    let args = OcValue::Array(args).encode();
    let status = unsafe {
        sys::invoke(address.as_ptr(), address.len(), method.as_ptr(), method.len(), args.as_ptr(), args.len())
    };
    if status != STATUS_OK {
        return Err(response_error(status));
    }
    Ok(match response_value() {
        OcValue::Array(items) => items,
        OcValue::Nil => Vec::new(),
        other => vec![other],
    })
}

/// Address → type for every component whose type contains `filter`.
pub fn components(filter: &str) -> Vec<(String, String)> {
    let status = unsafe { sys::components(filter.as_ptr(), filter.len()) };
    if status != STATUS_OK {
        return Vec::new();
    }
    match response_value() {
        OcValue::Map(entries) => entries
            .into_iter()
            .filter_map(|(k, v)| Some((k.as_str()?.to_owned(), v.as_str()?.to_owned())))
            .collect(),
        _ => Vec::new(),
    }
}

pub fn component(kind: &str) -> Option<String> {
    components(kind).into_iter().find(|(_, t)| t == kind).map(|(a, _)| a)
}

pub struct Signal {
    pub name: String,
    pub args: Vec<OcValue>,
}

/// Waits for a signal. A negative timeout waits forever. Returns `None` on timeout.
pub fn pull_signal(timeout: f64) -> Option<Signal> {
    match unsafe { sys::pull_signal(timeout) } {
        STATUS_OK => match response_value() {
            OcValue::Array(mut items) if !items.is_empty() => {
                let name = match items.remove(0) {
                    OcValue::String(s) => s,
                    _ => return None,
                };
                Some(Signal { name, args: items })
            }
            _ => None,
        },
        _ => None,
    }
}

pub fn push_signal(name: &str, args: Vec<OcValue>) -> bool {
    let mut signal = vec![OcValue::str(name)];
    signal.extend(args);
    let bytes = OcValue::Array(signal).encode();
    unsafe { sys::push_signal(bytes.as_ptr(), bytes.len()) == STATUS_OK }
}

pub fn beep(frequency: i32, duration_ms: i32) {
    unsafe { sys::beep(frequency, duration_ms) };
}

pub fn uptime() -> f64 {
    unsafe { sys::uptime() }
}

pub fn address() -> String {
    let mut buf = [0u8; 64];
    let len = unsafe { sys::address(buf.as_mut_ptr(), buf.len()) } as usize;
    String::from_utf8_lossy(&buf[..len.min(buf.len())]).into_owned()
}

pub fn memory_limit() -> i64 {
    unsafe { sys::memory_limit() }
}

pub fn log(message: &str) {
    unsafe { sys::log(message.as_ptr(), message.len()) }
}

pub fn shutdown(reboot: bool) -> ! {
    unsafe { sys::shutdown(reboot as i32) }
}

pub fn crash(message: &str) -> ! {
    unsafe { sys::crash(message.as_ptr(), message.len()) }
}

/// OC hands numbers over as ints or doubles depending on the source.
pub fn number(value: &OcValue) -> Option<f64> {
    match value {
        OcValue::Int(i) => Some(*i as f64),
        OcValue::Float(f) => Some(*f),
        _ => None,
    }
}

// ---------------------------------------------------------------- filesystems

fn first_bytes(results: &[OcValue]) -> Option<Vec<u8>> {
    match results.first()? {
        OcValue::String(s) => Some(s.clone().into_bytes()),
        OcValue::Bytes(b) => Some(b.clone()),
        _ => None,
    }
}

/// Reads a whole file from the filesystem component at `fs`.
///
/// The buffer is sized up front: wasm memory never shrinks, so growing it by
/// doubling would leave this process holding twice the file for good. Running
/// out of memory is an error, not an abort.
pub fn read_file(fs: &str, path: &str) -> Result<Vec<u8>, String> {
    let size = invoke(fs, "size", vec![OcValue::str(path)])?.first().and_then(number).unwrap_or(0.0) as usize;
    let mut data = Vec::new();
    data.try_reserve_exact(size).map_err(|_| format!("not enough memory to load {size} bytes"))?;
    let opened = invoke(fs, "open", vec![OcValue::str(path)])?;
    let handle = match opened.first() {
        Some(handle @ OcValue::Userdata(_)) => handle.clone(),
        _ => return Err(opened.get(1).and_then(|r| r.as_str().map(str::to_owned)).unwrap_or_else(|| "cannot open".into())),
    };
    loop {
        // OC caps each read at its maxReadBuffer (2 KiB by default).
        let chunk = invoke(fs, "read", vec![handle.clone(), OcValue::Int(1 << 30)])?;
        match first_bytes(&chunk) {
            Some(bytes) => {
                data.try_reserve(bytes.len()).map_err(|_| "not enough memory".to_string())?;
                data.extend(bytes);
            }
            None => break,
        }
    }
    let _ = invoke(fs, "close", vec![handle]);
    Ok(data)
}

/// Directory entries; directories end in `/`, as OpenComputers reports them.
pub fn list_dir(fs: &str, path: &str) -> Result<Vec<String>, String> {
    let results = invoke(fs, "list", vec![OcValue::str(path)])?;
    match results.into_iter().next() {
        Some(OcValue::Array(items)) => Ok(items.into_iter().filter_map(|v| v.as_str().map(str::to_owned)).collect()),
        Some(OcValue::Map(entries)) => Ok(entries.into_iter().filter_map(|(_, v)| v.as_str().map(str::to_owned)).collect()),
        _ => Err("not a directory".into()),
    }
}

/// The filesystem programs' `std::fs` uses (the disk they see as `/`),
/// chosen by the host on first use.
pub fn files_disk() -> Option<String> {
    let status = unsafe { sys::files_disk() };
    (status == STATUS_OK).then(|| String::from_utf8_lossy(&response()).into_owned())
}

/// Whether `path` is a directory on `fs`.
pub fn is_dir(fs: &str, path: &str) -> bool {
    matches!(invoke(fs, "isDirectory", vec![OcValue::str(path)]).as_deref(), Ok([OcValue::Bool(true), ..]))
}

/// The filesystem the BIOS booted from, which it records in the EEPROM's data.
pub fn boot_address() -> Option<String> {
    let eeprom = component("eeprom")?;
    let data = first_bytes(&invoke(&eeprom, "getData", Vec::new()).ok()?)?;
    String::from_utf8(data).ok().filter(|s| !s.is_empty())
}

// ---------------------------------------------------------------- processes

/// What a child process did; see `EVENT_*` in `crates/wackasm-oc/src/abi.rs`.
pub enum ProcEvent {
    Exited { code: i64, output: Vec<u8> },
    Output(Vec<u8>),
    Input,
    Crashed { message: String, output: Vec<u8> },
}

/// `spawn_file`: there is no such file.
pub const STATUS_NOT_FOUND: i32 = -5;

fn proc_event(status: i32) -> Result<ProcEvent, String> {
    if status < 0 {
        return Err(String::from_utf8_lossy(&response()).into_owned());
    }
    let items = match response_value() {
        OcValue::Array(items) => items,
        _ => Vec::new(),
    };
    let bytes = |i: usize| match items.get(i) {
        Some(OcValue::Bytes(b)) => b.clone(),
        _ => Vec::new(),
    };
    Ok(match status {
        0 => ProcEvent::Exited { code: items.first().and_then(number).unwrap_or(0.0) as i64, output: bytes(1) },
        1 => ProcEvent::Output(bytes(0)),
        2 => ProcEvent::Input,
        _ => ProcEvent::Crashed {
            message: items.first().and_then(|v| v.as_str()).unwrap_or("crashed").to_owned(),
            output: bytes(1),
        },
    })
}

/// Starts `image` (a wasm binary, WASI or `oc`) as a child process with
/// `argv`, and runs it until its first event.
pub fn spawn(image: &[u8], argv: &[String]) -> Result<ProcEvent, String> {
    let argv = OcValue::Array(argv.iter().map(OcValue::str).collect()).encode();
    proc_event(unsafe { sys::spawn(image.as_ptr(), image.len(), argv.as_ptr(), argv.len()) })
}

/// Like [`spawn`], but the host loads `path` from the filesystem `fs`, using
/// its server-wide cache of compiled programs: a program that ran before
/// starts without being read or compiled again.
/// Also hands the program `env` (`NAME=value` strings) and its working
/// directory `cwd` (which it gets as `PWD`). `None` if there is no such file.
pub fn spawn_file(fs: &str, path: &str, argv: &[String], env: &[&str], cwd: &str) -> Option<Result<ProcEvent, String>> {
    let strings = |items: Vec<OcValue>| OcValue::Array(items);
    let spec = OcValue::Map(vec![
        (OcValue::str("disk"), OcValue::str(fs)),
        (OcValue::str("path"), OcValue::str(path)),
        (OcValue::str("argv"), strings(argv.iter().map(OcValue::str).collect())),
        (OcValue::str("env"), strings(env.iter().map(|v| OcValue::str(*v)).collect())),
        (OcValue::str("cwd"), OcValue::str(cwd)),
    ])
    .encode();
    let status = unsafe { sys::spawn_with(spec.as_ptr(), spec.len()) };
    (status != STATUS_NOT_FOUND).then(|| proc_event(status))
}

/// Continues the child after an event, appending `input` to its stdin. An
/// empty input answers `ProcEvent::Input` with end of file.
pub fn resume(input: &[u8]) -> Result<ProcEvent, String> {
    proc_event(unsafe { sys::resume(input.as_ptr(), input.len()) })
}

pub fn kill() {
    unsafe { sys::kill() };
}
