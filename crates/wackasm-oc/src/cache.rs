//! A server-wide cache of compiled programs for `oc.spawn_file`.
//!
//! Reading a program through `filesystem.read` dominates spawn time: OC caps
//! each read at 2 KiB and limits direct calls per tick, so a 60 KB program
//! takes several ticks to load. Compiling it takes only a few milliseconds.
//! So `spawn_file` asks the disk for the file's size and modification time
//! (two cheap calls), and if a compiled module is cached under
//! `(disk, path, size, mtime)` it starts that without reading anything.
//! Otherwise the host reads the file itself, compiles it, and caches it.
//!
//! The host reads through the replay mechanism (see [`crate::wasi`]), so
//! loading can be suspended and saved like anything else. Entries hold the
//! module as bitcode, not yet instantiated: each spawn deserializes a fresh
//! copy, which is about 4x faster than compiling. The cache is shared by every
//! machine in the server process and evicts least recently used entries past
//! [`CAPACITY`].

use std::sync::{Arc, Mutex};

use anyhow::{Context as _, Result, anyhow, bail};
use wackasm::{WackasmExecutor, WackasmModule};

use crate::{abi::*, machine::Request, process::ProcessIo, value::OcValue};

/// Total bytes of cached modules kept.
pub const CAPACITY: usize = 64 * 1024 * 1024;

#[derive(Clone, PartialEq, Eq, Debug)]
struct Key {
    disk: String,
    path: String,
    size: u64,
    modified: u64,
}

/// Most recently used last.
static CACHE: Mutex<Vec<(Key, Arc<Vec<u8>>)>> = Mutex::new(Vec::new());

fn lookup(key: &Key) -> Option<Arc<Vec<u8>>> {
    let mut cache = CACHE.lock().ok()?;
    let index = cache.iter().position(|(k, _)| k == key)?;
    let entry = cache.remove(index);
    let module = entry.1.clone();
    cache.push(entry);
    Some(module)
}

fn store(key: Key, module: Arc<Vec<u8>>) {
    let Ok(mut cache) = CACHE.lock() else { return };
    // A changed file (same path, new size/mtime) replaces its old entry.
    cache.retain(|(k, _)| !(k.disk == key.disk && k.path == key.path));
    cache.push((key, module));
    let mut total: usize = cache.iter().map(|(_, m)| m.len()).sum();
    while total > CAPACITY && cache.len() > 1 {
        total -= cache.remove(0).1.len();
    }
}

/// Drops every cached module.
pub fn clear() {
    if let Ok(mut cache) = CACHE.lock() {
        cache.clear();
    }
}

/// Validates and compiles a wasm binary into the cached form.
pub(crate) fn compile(image: &[u8]) -> Result<Vec<u8>> {
    if !image.starts_with(WASM_MAGIC) {
        bail!("not a WebAssembly binary");
    }
    let module = WackasmModule::new(image).context("invalid wasm module")?;
    bitcode::serialize(&module).map_err(|e| anyhow!("caching module: {e}"))
}

/// Instantiates a cached module under `memory_limit` and queues its entry point.
pub(crate) fn instantiate_compiled(compiled: &[u8], memory_limit: Option<u64>) -> Result<WackasmExecutor> {
    let module: WackasmModule = bitcode::deserialize(compiled).map_err(|e| anyhow!("loading cached module: {e}"))?;
    let mut executor = WackasmExecutor::new(module).context("instantiating module")?;
    if let Some(limit) = memory_limit {
        let used = executor.memory().used_bytes();
        if used > limit {
            bail!("initial memory of {used} bytes exceeds the limit of {limit} bytes");
        }
    }
    executor.set_memory_limit(memory_limit);
    ENTRY_POINTS
        .iter()
        .find(|name| executor.call_export(name, Vec::new()).is_ok())
        .ok_or_else(|| anyhow!("module exports none of {ENTRY_POINTS:?}"))?;
    Ok(executor)
}

pub(crate) enum Load {
    Ready(Arc<Vec<u8>>),
    /// A component call is needed; the request is recorded in the process's
    /// host calls, and the import runs again once it is answered.
    Pending,
    Failed(String),
    NotFound,
}

/// `?` for loading: a pending call or a failure ends this run.
macro_rules! call {
    ($io:expr, $disk:expr, $method:expr, $($arg:expr),*) => {
        match $io.host(Request::Invoke {
            address: $disk.to_owned(),
            method: $method.to_owned(),
            args: vec![$($arg),*],
        }) {
            Err(_) => return Load::Pending,
            Ok(answer) if answer.status != STATUS_OK => {
                return Load::Failed(String::from_utf8_lossy(&answer.payload).into_owned())
            }
            Ok(answer) => match OcValue::decode(&answer.payload) {
                Ok(OcValue::Array(items)) => items,
                _ => Vec::new(),
            },
        }
    };
}

fn number(results: &[OcValue]) -> u64 {
    match results.first() {
        Some(OcValue::Int(n)) => (*n).max(0) as u64,
        Some(OcValue::Float(f)) => f.max(0.0) as u64,
        _ => 0,
    }
}

/// Finds `path` on `disk` in the cache, or reads and compiles it. Runs again
/// from the top after each answered call (see [`crate::wasi`]).
///
/// A cache hit costs two component calls. A missing file needs no `exists`
/// check: OC reports its size and time as 0, which never matches an entry
/// (an empty file doesn't compile), and the `open` that follows fails.
pub(crate) fn load(io: &ProcessIo, disk: &str, path: &str) -> Load {
    let p = || OcValue::str(path);
    let key = Key {
        disk: disk.to_owned(),
        path: path.to_owned(),
        size: number(&call!(io, disk, "size", p())),
        modified: number(&call!(io, disk, "lastModified", p())),
    };
    if let Some(module) = lookup(&key) {
        return Load::Ready(module);
    }

    let opened = match io.host(Request::Invoke {
        address: disk.to_owned(),
        method: "open".into(),
        args: vec![p(), OcValue::str("rb")],
    }) {
        Err(_) => return Load::Pending,
        Ok(answer) if answer.status != STATUS_OK => {
            return if key.size == 0 && key.modified == 0 {
                Load::NotFound
            } else {
                Load::Failed(format!("{path}: {}", String::from_utf8_lossy(&answer.payload)))
            };
        }
        Ok(answer) => match OcValue::decode(&answer.payload) {
            Ok(OcValue::Array(items)) => items,
            _ => Vec::new(),
        },
    };
    let Some(handle @ OcValue::Userdata(_)) = opened.into_iter().next() else {
        return Load::Failed(format!("{path}: cannot open"));
    };
    let mut image = Vec::with_capacity(key.size as usize);
    loop {
        match call!(io, disk, "read", handle.clone(), OcValue::Int(1 << 30)).into_iter().next() {
            Some(OcValue::Bytes(chunk)) => image.extend(chunk),
            Some(OcValue::String(chunk)) => image.extend(chunk.into_bytes()),
            _ => break,
        }
    }
    call!(io, disk, "close", handle);

    match compile(&image) {
        Ok(module) => {
            let module = Arc::new(module);
            store(key, module.clone());
            Load::Ready(module)
        }
        Err(e) => Load::Failed(format!("{path}: {e:#}")),
    }
}
