//! The architecture state machine, free of any JNI types.
//!
//! The JVM drives it through a small loop:
//!
//! 1. `run(uptime, fuel)` executes guest code until the fuel runs out, the
//!    guest finishes, or the guest calls an `oc` import that needs the game.
//! 2. For an import like that, wackasm *suspends* the executor with the
//!    import still pending, and `run` returns a [`Request`].
//! 3. Java services the request, possibly on another tick or thread, possibly
//!    after a world save/load, and calls `respond(status, payload)`. That
//!    completes the pending import, and the next `run` continues from there.
//!
//! The pending import lives inside the serialized `WackasmExecutor`, and the
//! outstanding `Request` lives in [`State`], so a machine can be snapshotted
//! at any point in this cycle, including in the middle of a component call.
//!
//! A machine runs a stack of processes (see [`crate::process`]): the BIOS or
//! OS at the bottom, and `oc.spawn`ed children above it. Whichever one is
//! active is the one `run` executes and `respond` answers.

use anyhow::{Context as _, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use wackasm::{
    HostYield, Value, WackasmExecutor, WackasmModule,
    imports::{NoResources, WackasmImports, WackasmLinkerResources},
};
use wackasm_wasi::{WASI_PREVIEW1, Wasi1};

use crate::{
    abi::*,
    cache::{self, Load},
    process::{Event, Process, ProcessIo},
    storage::WorkDir,
    value::OcValue,
    wasi::{Disk, HostAnswer, resolve},
};

const SNAPSHOT_MAGIC: &[u8; 4] = b"WKOC";
// v2: MemoryManager carries the host memory limit.
// v3: a stack of processes with WASI stdio.
// v4: processes have environment variables.
const SNAPSHOT_VERSION: u32 = 4;
/// WASI `errno` for "function not implemented".
const WASI_ENOSYS: i32 = 52;

/// What the guest wants from the game. Encoded for Java by [`Request::payload`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Request {
    Invoke {
        address: String,
        method: String,
        args: Vec<OcValue>,
    },
    /// `remaining` is `None` when the guest waits forever.
    PullSignal { remaining: Option<f64> },
    PushSignal { signal: Vec<OcValue> },
    Components { filter: String },
    Methods { address: String },
    Beep { frequency: i32, duration_ms: i32 },
    /// Which filesystem a WASI process should use; see `RUN_DEFAULT_DISK`.
    DefaultDisk,
    /// A method of a userdata value; see `RUN_INVOKE_VALUE`.
    InvokeValue {
        value: u32,
        method: String,
        args: Vec<OcValue>,
    },
    /// See `RUN_RELEASE_VALUE`.
    ReleaseValue { value: u32 },
}

impl Request {
    pub fn code(&self) -> i32 {
        match self {
            Request::Invoke { .. } => RUN_INVOKE,
            Request::PullSignal { .. } => RUN_PULL_SIGNAL,
            Request::PushSignal { .. } => RUN_PUSH_SIGNAL,
            Request::Components { .. } => RUN_COMPONENTS,
            Request::Methods { .. } => RUN_METHODS,
            Request::Beep { .. } => RUN_BEEP,
            Request::DefaultDisk => RUN_DEFAULT_DISK,
            Request::InvokeValue { .. } => RUN_INVOKE_VALUE,
            Request::ReleaseValue { .. } => RUN_RELEASE_VALUE,
        }
    }

    /// Encoded as an array; see the `RUN_*` docs in [`crate::abi`] for the shapes.
    pub fn payload(&self) -> OcValue {
        OcValue::Array(match self {
            Request::Invoke {
                address,
                method,
                args,
            } => vec![
                OcValue::str(address),
                OcValue::str(method),
                OcValue::Array(args.clone()),
            ],
            Request::PullSignal { remaining } => {
                vec![remaining.map_or(OcValue::Nil, OcValue::Float)]
            }
            Request::PushSignal { signal } => signal.clone(),
            Request::Components { filter } => vec![OcValue::str(filter)],
            Request::Methods { address } => vec![OcValue::str(address)],
            Request::Beep {
                frequency,
                duration_ms,
            } => vec![
                OcValue::Int(*frequency as i64),
                OcValue::Int(*duration_ms as i64),
            ],
            Request::DefaultDisk => Vec::new(),
            Request::InvokeValue { value, method, args } => {
                vec![OcValue::Userdata(*value), OcValue::str(method), OcValue::Array(args.clone())]
            }
            Request::ReleaseValue { value } => vec![OcValue::Userdata(*value)],
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum RunOutcome {
    Yield,
    Halted,
    Shutdown { reboot: bool },
    Error(String),
    Request(Request),
}

impl RunOutcome {
    pub fn code(&self) -> i32 {
        match self {
            RunOutcome::Yield => RUN_YIELD,
            RunOutcome::Halted => RUN_HALTED,
            RunOutcome::Shutdown { .. } => RUN_SHUTDOWN,
            RunOutcome::Error(_) => RUN_ERROR,
            RunOutcome::Request(r) => r.code(),
        }
    }

    pub fn payload(&self) -> Option<OcValue> {
        match self {
            RunOutcome::Yield | RunOutcome::Halted => None,
            RunOutcome::Shutdown { reboot } => Some(OcValue::Array(vec![OcValue::Bool(*reboot)])),
            RunOutcome::Error(msg) => Some(OcValue::Array(vec![OcValue::str(msg)])),
            RunOutcome::Request(r) => Some(r.payload()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
enum Status {
    Running,
    Halted,
    Shutdown { reboot: bool },
    Crashed(String),
}

/// Everything that survives a save. Serialized with bitcode into
/// `<work_dir>/machines/<key>.state`.
#[derive(Serialize, Deserialize)]
struct State {
    address: String,
    /// Process 0 is the BIOS/OS. Each later one is a child of the one before it.
    processes: Vec<Process>,
    status: Status,
    /// The request last reported to Java and not yet answered.
    request: Option<Request>,
    /// Uptime passed to the most recent `run`.
    uptime: f64,
}

/// How a child ended.
enum Ending {
    Exited(i32),
    Crashed(String),
}

impl State {
    /// The process that runs: the highest one that isn't paused. A paused
    /// process has handed an event to its parent, and that parent may in turn
    /// have paused to pass it further down (`time` relaying `cat`'s output
    /// to the shell), so the runner can be any level. Process 0 never pauses.
    fn active(&self) -> usize {
        self.processes.iter().rposition(|p| !p.paused).unwrap_or(0)
    }

    /// Caps every process at the machine's RAM minus what the others use, so
    /// together they never exceed it. Only the active process can grow.
    fn apply_limits(&mut self, total: Option<u64>) {
        let used: Vec<u64> = self.processes.iter().map(Process::memory_used).collect();
        let all: u64 = used.iter().sum();
        for (process, own) in self.processes.iter_mut().zip(&used) {
            process.executor.set_memory_limit(total.map(|t| t.saturating_sub(all - own)));
        }
    }

    /// RAM left for a new process, given what the existing ones use.
    fn available(&self, total: Option<u64>) -> Option<u64> {
        let all: u64 = self.processes.iter().map(Process::memory_used).sum();
        total.map(|t| t.saturating_sub(all))
    }

    /// Hands `event` to the parent of process `child`, completing the parent's
    /// pending `spawn`/`resume`. The parent continues with the child's fuel.
    fn deliver(&mut self, child: usize, event: Event) -> Result<()> {
        let fuel = self.processes[child].executor.get_fuel();
        let parent = &mut self.processes[child - 1];
        let code = event.code();
        parent.response = event.payload();
        parent.executor.set_fuel(fuel);
        parent.executor.import_return(&[Value::from_i32(code)])?;
        Ok(())
    }

    /// Pauses `child` (still alive) and hands `event` to its parent.
    fn pause(&mut self, child: usize, event: Event) -> Result<()> {
        self.processes[child].paused = true;
        self.deliver(child, event)
    }

    /// Ends `child` and everything above it, and tells its parent why,
    /// along with any output the child wrote last.
    fn end(&mut self, child: usize, ending: Ending, total: Option<u64>) -> Result<()> {
        let output = std::mem::take(&mut self.processes[child].io.output);
        let event = match ending {
            Ending::Exited(code) => Event::Exited { code, output },
            Ending::Crashed(message) => Event::Crashed { message, output },
        };
        self.deliver(child, event)?;
        self.processes.truncate(child);
        self.apply_limits(total);
        Ok(())
    }
}

pub struct OcMachine {
    work_dir: WorkDir,
    key: String,
    address: String,
    /// Host memory budget for all processes together. wackasm enforces it in
    /// `memory.grow`, so a guest sees allocation failure rather than being killed.
    memory_limit: Option<u64>,
    state: Option<State>,
}

/// What an import handler decided.
enum ImportAction {
    /// Finish the import now with these results and keep executing.
    Return(Vec<Value>),
    /// Leave the import pending and hand the request to Java.
    Suspend(Request),
    /// Stop the machine.
    Halt(Status),
    /// `exec`: throw the current program away and continue with this one.
    Replace(WackasmExecutor),
    /// `spawn`: run this child. The parent's import stays pending.
    Spawn(Process),
    /// `resume`: unpause the child. The parent's import stays pending.
    Resume,
    /// `kill`: end the child, then return this status to the parent.
    Kill,
    /// WASI `proc_exit`.
    Exit(i32),
    /// The child waits for its parent; the import stays pending and runs again
    /// after `resume`.
    Pause(Event),
}

/// Parses and instantiates `image`, and queues its entry point.
fn instantiate(image: &[u8], memory_limit: Option<u64>) -> Result<WackasmExecutor> {
    if !image.starts_with(WASM_MAGIC) {
        bail!("not a WebAssembly binary");
    }
    let module = match memory_limit {
        Some(limit) => WackasmModule::with_memory_limit(image, limit),
        None => WackasmModule::new(image),
    }
    .context("invalid wasm module")?;
    let mut executor = WackasmExecutor::new(module).context("instantiating module")?;
    ENTRY_POINTS
        .iter()
        .find(|name| executor.call_export(name, Vec::new()).is_ok())
        .ok_or_else(|| anyhow!("module exports none of {ENTRY_POINTS:?}"))?;
    Ok(executor)
}

impl OcMachine {
    /// `key` names this machine's files in the work dir. `address` is the
    /// computer's component address as the guest sees it. After [`Self::load`],
    /// the address saved in the snapshot is used instead.
    pub fn new(work_dir: WorkDir, key: &str, address: &str) -> Result<Self> {
        WorkDir::validate_key(key)?;
        Ok(Self {
            work_dir,
            key: key.to_owned(),
            address: address.to_owned(),
            memory_limit: None,
            state: None,
        })
    }

    /// Takes effect immediately on a running guest. Memory it already holds
    /// is kept, and only later growth is refused.
    pub fn set_memory_limit(&mut self, bytes: u64) {
        self.memory_limit = Some(bytes);
        if let Some(state) = &mut self.state {
            state.apply_limits(self.memory_limit);
        }
    }

    pub fn is_booted(&self) -> bool {
        self.state.is_some()
    }

    /// How many processes are alive: 1 for just the BIOS/OS.
    pub fn process_count(&self) -> usize {
        self.state.as_ref().map_or(0, |s| s.processes.len())
    }

    /// Starts the program in the EEPROM (normally the wasm BIOS, which then
    /// `exec`s `/init.wasm` from a filesystem). Like the Lua architecture, a
    /// machine without a wasm EEPROM won't boot.
    pub fn boot(&mut self, eeprom: Option<&[u8]>) -> Result<()> {
        self.state = None;
        self.work_dir.remove_state(&self.key)?;

        // Same wording as the Lua architecture, which players already know.
        let image = match eeprom {
            None => bail!("no bios found; install a configured EEPROM"),
            Some(bytes) if !bytes.starts_with(WASM_MAGIC) => {
                bail!("EEPROM does not contain a WebAssembly BIOS; install the wasm BIOS EEPROM")
            }
            Some(bytes) => bytes,
        };
        let executor = instantiate(image, self.memory_limit).context("bios")?;

        self.state = Some(State {
            address: self.address.clone(),
            processes: vec![Process::new(executor, vec!["bios".into()])],
            status: Status::Running,
            request: None,
            uptime: 0.0,
        });
        Ok(())
    }

    /// Runs one time slice. `Err` is returned only for host-side misuse (not
    /// booted). A failure of the BIOS/OS is reported as [`RunOutcome::Error`]
    /// and the machine stays crashed. A child's failure only ends that child.
    pub fn run(&mut self, uptime: f64, fuel: isize) -> Result<RunOutcome> {
        let state = self.state.as_mut().ok_or_else(|| anyhow!("machine not booted"))?;
        state.uptime = uptime;

        if let Some(outcome) = terminal_outcome(&state.status) {
            return Ok(outcome);
        }

        let active = state.active();
        state.processes[active].executor.set_fuel(fuel);
        let outcome = loop {
            match step(state, &self.work_dir, &self.key, self.memory_limit) {
                Ok(outcome) => break outcome,
                Err(error) => {
                    let failed = state.active();
                    if failed == 0 {
                        break crash(state, error);
                    }
                    // A child's trap or misuse ends only that child.
                    let ending = Ending::Crashed(format!("{error:#}"));
                    if let Err(error) = state.end(failed, ending, self.memory_limit) {
                        break crash(state, error);
                    }
                }
            }
        };
        if let RunOutcome::Request(request) = &outcome {
            state.request = Some(request.clone());
        }
        Ok(outcome)
    }

    /// The request awaiting a response, if any. It survives save/load, which
    /// lets Java service it from `runSynchronized` straight after a load.
    pub fn pending_request(&self) -> Option<&Request> {
        self.state.as_ref()?.request.as_ref()
    }

    /// The payload Java should act on now: the pending request if there is
    /// one, otherwise the halt reason (`[reboot]` or `[message]`).
    pub fn current_payload(&self) -> Option<OcValue> {
        let state = self.state.as_ref()?;
        if let Some(request) = &state.request {
            return Some(request.payload());
        }
        terminal_outcome(&state.status)?.payload()
    }

    /// Answers the pending request and unblocks the guest that made it.
    /// `payload` is an encoded value when `status` is `STATUS_OK`, and a UTF-8
    /// message otherwise.
    pub fn respond(&mut self, status: i32, payload: Vec<u8>) -> Result<()> {
        let state = self.state.as_mut().ok_or_else(|| anyhow!("machine not booted"))?;
        let request = state
            .request
            .take()
            .ok_or_else(|| anyhow!("no request is awaiting a response"))?;

        let (status, payload) = if status == STATUS_OK
            && !payload.is_empty()
            && OcValue::decode(&payload).is_err()
        {
            // Don't hand the guest something it can't parse.
            (STATUS_ERROR, b"host returned a malformed value".to_vec())
        } else {
            (status, payload)
        };

        // Requests only come from the active process, which can't change
        // until the request is answered.
        let active = state.active();
        let process = &mut state.processes[active];

        // A disk call from a WASI import: keep the answer for the import's
        // re-run instead of returning it to the guest (see `crate::wasi`).
        let mut host = process.io.host.borrow_mut();
        if host.awaiting {
            host.awaiting = false;
            if matches!(request, Request::DefaultDisk) {
                *process.io.disk.borrow_mut() = match OcValue::decode(&payload) {
                    Ok(OcValue::Array(items)) => match items.into_iter().next() {
                        Some(OcValue::String(address)) => Disk::Address(address),
                        _ => Disk::Missing,
                    },
                    _ => Disk::Missing,
                };
            } else {
                host.answers.push(HostAnswer { status, payload });
            }
            return Ok(());
        }
        drop(host);

        if matches!(request, Request::PullSignal { .. }) {
            process.pull_deadline = None;
        }
        process.response = payload;
        process
            .executor
            .import_return(&[Value::from_i32(status)])
            .context("completing import")?;
        Ok(())
    }

    /// Writes the snapshot into the work dir.
    pub fn save(&self) -> Result<()> {
        let state = self.state.as_ref().ok_or_else(|| anyhow!("machine not booted"))?;
        let body = bitcode::serialize(state).map_err(|e| anyhow!("serializing machine: {e}"))?;
        let mut bytes = Vec::with_capacity(body.len() + 8);
        bytes.extend_from_slice(SNAPSHOT_MAGIC);
        bytes.extend_from_slice(&SNAPSHOT_VERSION.to_le_bytes());
        bytes.extend_from_slice(&body);
        self.work_dir.write_state(&self.key, &bytes)
    }

    /// Restores the snapshot from the work dir. Returns `false` if none exists.
    pub fn load(&mut self) -> Result<bool> {
        let Some(bytes) = self.work_dir.read_state(&self.key)? else {
            return Ok(false);
        };
        let body = bytes
            .strip_prefix(SNAPSHOT_MAGIC)
            .ok_or_else(|| anyhow!("not a machine snapshot"))?;
        let (version, body) = body
            .split_first_chunk::<4>()
            .ok_or_else(|| anyhow!("snapshot truncated"))?;
        let version = u32::from_le_bytes(*version);
        if version != SNAPSHOT_VERSION {
            bail!("unsupported snapshot version {version}");
        }
        let mut state: State =
            bitcode::deserialize(body).map_err(|e| anyhow!("deserializing machine: {e}"))?;
        // A limit set before loading (from recomputeMemory) wins. Otherwise
        // recover the total: any process's cap plus what the others use.
        match self.memory_limit {
            Some(_) => state.apply_limits(self.memory_limit),
            None => {
                let others: u64 = state.processes[1..].iter().map(Process::memory_used).sum();
                self.memory_limit = state.processes[0].executor.memory().limit().map(|l| l + others);
            }
        }
        self.address = state.address.clone();
        self.state = Some(state);
        Ok(true)
    }

    /// Drops the running program and deletes its snapshot.
    pub fn discard(&mut self) -> Result<()> {
        self.state = None;
        self.work_dir.remove_state(&self.key)
    }
}

fn crash(state: &mut State, error: anyhow::Error) -> RunOutcome {
    let message = format!("{error:#}");
    state.status = Status::Crashed(message.clone());
    state.request = None;
    RunOutcome::Error(message)
}

fn terminal_outcome(status: &Status) -> Option<RunOutcome> {
    match status {
        Status::Running => None,
        Status::Halted => Some(RunOutcome::Halted),
        Status::Shutdown { reboot } => Some(RunOutcome::Shutdown { reboot: *reboot }),
        Status::Crashed(msg) => Some(RunOutcome::Error(msg.clone())),
    }
}

/// Runs the active process until the slice ends or Java is needed. An `Err`
/// is the active process's fault; the caller decides what that kills.
fn step(state: &mut State, work_dir: &WorkDir, key: &str, total: Option<u64>) -> Result<RunOutcome> {
    loop {
        let active = state.active();
        let yielded = state.processes[active].executor.execute()?;

        let (module, name, args, should_return) = match yielded {
            HostYield::OutOfFuel => return Ok(RunOutcome::Yield),
            HostYield::Return if active == 0 => {
                state.status = Status::Halted;
                return Ok(RunOutcome::Halted);
            }
            HostYield::Return => {
                // A child's `_start` returned: exit code 0.
                state.end(active, Ending::Exited(0), total)?;
                continue;
            }
            HostYield::ImportCall {
                module,
                name,
                args,
                should_return,
            } => (module, name, args, should_return),
        };

        let action = match module.as_str() {
            IMPORT_MODULE => handle_import(state, active, work_dir, key, total, &name, &args)?,
            WASI_PREVIEW1 => handle_wasi(state, active, &name, args, should_return)?,
            _ => bail!("unknown import {module}.{name}"),
        };
        match action {
            ImportAction::Return(results) => {
                if results.len() != should_return {
                    bail!(
                        "{module}.{name} declared {should_return} results, expected {}",
                        results.len()
                    );
                }
                let process = &mut state.processes[active];
                process.executor.import_return(&results)?;
                if active > 0 && !process.io.output.is_empty() {
                    // Show output as soon as it is written, like a terminal.
                    let output = std::mem::take(&mut process.io.output);
                    state.pause(active, Event::Output(output))?;
                } else if !process.io.output.is_empty() {
                    // The first process has no terminal: its stdout and
                    // stderr go to the machine's log (best-effort).
                    let output = std::mem::take(&mut process.io.output);
                    let text = String::from_utf8_lossy(&output);
                    let _ = work_dir.append_log(key, text.trim_end_matches('\n'));
                }
            }
            ImportAction::Suspend(request) => return Ok(RunOutcome::Request(request)),
            ImportAction::Halt(status) => {
                let outcome = terminal_outcome(&status).expect("halt status is terminal");
                state.status = status;
                return Ok(outcome);
            }
            ImportAction::Replace(executor) => {
                let process = &mut state.processes[active];
                // The new program inherits what is left of this slice's fuel.
                executor.set_fuel(process.executor.get_fuel());
                process.executor = executor;
                process.response.clear();
                process.pull_deadline = None;
                state.apply_limits(total);
            }
            ImportAction::Spawn(child) => {
                child.executor.set_fuel(state.processes[active].executor.get_fuel());
                state.processes.push(child);
                state.apply_limits(total);
            }
            ImportAction::Resume => {
                let fuel = state.processes[active].executor.get_fuel();
                let child = &mut state.processes[active + 1];
                child.paused = false;
                child.executor.set_fuel(fuel);
            }
            ImportAction::Kill => {
                let killed = state.processes.len() > active + 1;
                state.processes.truncate(active + 1);
                state.apply_limits(total);
                let status = if killed { STATUS_OK } else { STATUS_ERROR };
                state.processes[active].executor.import_return(&[Value::from_i32(status)])?;
            }
            ImportAction::Exit(_) if active == 0 => {
                state.status = Status::Halted;
                return Ok(RunOutcome::Halted);
            }
            ImportAction::Exit(code) => state.end(active, Ending::Exited(code), total)?,
            ImportAction::Pause(event) => state.pause(active, event)?,
        }
    }
}

/// A guest-supplied argument was unusable. The guest gets
/// `STATUS_BAD_ARGUMENT` with the message in its response buffer, and the
/// machine keeps running.
struct BadArgument(String);

fn guest_bytes(executor: &WackasmExecutor, ptr: Value, len: Value) -> Result<&[u8], BadArgument> {
    executor
        .memory()
        .read_bytes(0, ptr.as_u32(), len.as_u32())
        .map_err(|_| BadArgument("pointer out of bounds".into()))
}

fn guest_str(executor: &WackasmExecutor, ptr: Value, len: Value) -> Result<String, BadArgument> {
    String::from_utf8(guest_bytes(executor, ptr, len)?.to_vec())
        .map_err(|_| BadArgument("string is not valid UTF-8".into()))
}

fn guest_array(executor: &WackasmExecutor, ptr: Value, len: Value) -> Result<Vec<OcValue>, BadArgument> {
    match OcValue::decode(guest_bytes(executor, ptr, len)?) {
        Ok(OcValue::Array(items)) => Ok(items),
        Ok(_) => Err(BadArgument("expected an encoded array".into())),
        Err(e) => Err(BadArgument(format!("malformed value: {e}"))),
    }
}

fn guest_argv(executor: &WackasmExecutor, ptr: Value, len: Value) -> Result<Vec<String>, BadArgument> {
    guest_array(executor, ptr, len)?
        .into_iter()
        .map(|v| match v {
            OcValue::String(s) => Ok(s),
            _ => Err(BadArgument("argv must be an array of strings".into())),
        })
        .collect()
}

/// What to start, for `spawn_file` and `spawn_with`.
struct SpawnSpec {
    source: Source,
    argv: Vec<String>,
    /// `None` inherits the parent's environment.
    env: Option<Vec<String>>,
    /// The child's working directory (its `PWD`); `None` keeps the one its
    /// environment names.
    cwd: Option<String>,
    /// The child's WASI disk: the parent's (`spawn_with`), or left for Java
    /// to choose on first use (`spawn_file`, as before).
    inherit_disk: bool,
}

enum Source {
    Image(Vec<u8>),
    /// `disk: None` means the parent's WASI disk.
    File { disk: Option<String>, path: String },
}

/// Decodes `spawn_with`'s spec: an encoded map with `path` (and optionally
/// `disk`) or `image`, plus optional `argv` and `env`.
fn spawn_spec(executor: &WackasmExecutor, ptr: Value, len: Value) -> Result<SpawnSpec, BadArgument> {
    let entries = match OcValue::decode(guest_bytes(executor, ptr, len)?) {
        Ok(OcValue::Map(entries)) => entries,
        Ok(_) => return Err(BadArgument("spawn spec must be a map".into())),
        Err(e) => return Err(BadArgument(format!("malformed spawn spec: {e}"))),
    };
    let get = |key: &str| entries.iter().find(|(k, _)| k.as_str() == Some(key)).map(|(_, v)| v);
    let text = |key: &str| match get(key) {
        None | Some(OcValue::Nil) => Ok(None),
        Some(OcValue::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(BadArgument(format!("spawn spec `{key}` must be a string"))),
    };
    let strings = |key: &str| -> Result<Option<Vec<String>>, BadArgument> {
        match get(key) {
            None | Some(OcValue::Nil) => Ok(None),
            Some(OcValue::Array(items)) => items
                .iter()
                .map(|v| v.as_str().map(str::to_owned).ok_or_else(|| BadArgument(format!("spawn spec `{key}` must hold strings"))))
                .collect::<Result<Vec<_>, _>>()
                .map(Some),
            // {name = value} as well as ["name=value", ...].
            Some(OcValue::Map(pairs)) if key == "env" => pairs
                .iter()
                .map(|(k, v)| match (k.as_str(), v.as_str()) {
                    (Some(k), Some(v)) => Ok(format!("{k}={v}")),
                    _ => Err(BadArgument("spawn spec `env` must map strings to strings".into())),
                })
                .collect::<Result<Vec<_>, _>>()
                .map(Some),
            Some(_) => Err(BadArgument(format!("spawn spec `{key}` must be an array of strings"))),
        }
    };
    let source = match (get("image"), text("path")?) {
        (Some(OcValue::Bytes(image)), _) => Source::Image(image.clone()),
        (Some(OcValue::String(image)), _) => Source::Image(image.clone().into_bytes()),
        (Some(_), _) => return Err(BadArgument("spawn spec `image` must be bytes".into())),
        (None, Some(path)) => Source::File { disk: text("disk")?, path },
        (None, None) => return Err(BadArgument("spawn spec needs `path` or `image`".into())),
    };
    let argv = match strings("argv")? {
        Some(argv) => argv,
        None => vec![match &source {
            Source::File { path, .. } => path.clone(),
            Source::Image(_) => "program".into(),
        }],
    };
    Ok(SpawnSpec { source, argv, env: strings("env")?, cwd: text("cwd")?, inherit_disk: true })
}

/// `path` from `base`, or from the root if it's absolute; `..` can't climb
/// above `/`.
fn resolve_from(base: &str, path: &str) -> Result<String, wackasm_wasi::WasiErrno> {
    resolve(if path.starts_with('/') { "/" } else { base }, path.as_bytes())
}

/// The `PWD` an environment names, or `/`.
fn working_dir(env: &[String]) -> &str {
    env.iter().rev().find_map(|var| var.strip_prefix("PWD=")).filter(|dir| dir.starts_with('/')).unwrap_or("/")
}

/// `oc.files_disk`: the filesystem this process's WASI calls use, chosen
/// (by Java) on first use.
fn files_disk(process: &mut Process) -> Result<ImportAction> {
    {
        let mut host = process.io.host.borrow_mut();
        host.cursor = 0;
        host.needed = None;
    }
    let disk = process.io.disk();
    let mut host = process.io.host.borrow_mut();
    match disk {
        Ok(disk) => {
            drop(host);
            process.response = disk.into_bytes();
            Ok(ImportAction::Return(vec![Value::from_i32(STATUS_OK)]))
        }
        Err(_) => {
            if let Some(request) = host.needed.take() {
                host.awaiting = true;
                return Ok(ImportAction::Suspend(request));
            }
            drop(host);
            process.response = b"no filesystem to use".to_vec();
            Ok(ImportAction::Return(vec![Value::from_i32(STATUS_NOT_FOUND)]))
        }
    }
}

fn spawn_failed(process: &mut Process, status: i32, message: String) -> Result<ImportAction> {
    process.response = message.into_bytes();
    Ok(ImportAction::Return(vec![Value::from_i32(status)]))
}

/// `oc.spawn_file` and `oc.spawn_with`: load the program (from an image, or
/// from a disk through the module cache) and start it as a child.
fn spawn_child(state: &mut State, active: usize, total: Option<u64>, args: &[Value], with_spec: bool) -> Result<ImportAction> {
    let available = state.available(total);
    let has_child = state.processes.len() > active + 1;
    let process = &mut state.processes[active];
    let parsed = (|| {
        if has_child {
            return Err(BadArgument("a child process is already running; kill it first".into()));
        }
        let exec = &process.executor;
        if with_spec {
            return spawn_spec(exec, args[0], args[1]);
        }
        Ok(SpawnSpec {
            source: Source::File { disk: Some(guest_str(exec, args[0], args[1])?), path: guest_str(exec, args[2], args[3])? },
            argv: guest_argv(exec, args[4], args[5])?,
            env: None,
            cwd: None,
            inherit_disk: false,
        })
    })();
    let spec = match parsed {
        Ok(spec) => spec,
        Err(BadArgument(message)) => return spawn_failed(process, STATUS_BAD_ARGUMENT, message),
    };

    let mut env = spec.env.unwrap_or_else(|| process.io.env.clone());
    // Relative paths, in `cwd` and `path`, start at the directory the
    // caller's environment names.
    let base = working_dir(&process.io.env).to_owned();
    let cwd = match spec.cwd {
        Some(cwd) => match resolve_from(&base, &cwd) {
            Ok(cwd) => {
                env.retain(|var| !var.starts_with("PWD="));
                env.push(format!("PWD={cwd}"));
                cwd
            }
            Err(_) => return spawn_failed(process, STATUS_BAD_ARGUMENT, format!("{cwd}: outside the disk")),
        },
        None => base,
    };
    let source = match spec.source {
        Source::File { disk, path } if !path.starts_with('/') => match resolve_from(&cwd, &path) {
            Ok(path) => Source::File { disk, path },
            Err(_) => return spawn_failed(process, STATUS_BAD_ARGUMENT, format!("{path}: outside the disk")),
        },
        source => source,
    };
    let child_disk = if spec.inherit_disk { process.io.disk.borrow().clone() } else { Disk::Unknown };
    let child = |executor| {
        let mut child = Process::new(executor, spec.argv);
        child.io.env = env;
        *child.io.disk.borrow_mut() = child_disk;
        ImportAction::Spawn(child)
    };

    let (disk, path) = match source {
        Source::Image(image) => {
            return match instantiate(&image, available) {
                Ok(executor) => Ok(child(executor)),
                Err(e) => spawn_failed(process, STATUS_ERROR, format!("{e:#}")),
            };
        }
        Source::File { disk, path } => (disk, path),
    };

    // Runs again after each answered component call; see `crate::wasi`.
    {
        let mut host = process.io.host.borrow_mut();
        host.cursor = 0;
        host.needed = None;
    }
    let disk = match disk {
        Some(disk) => disk,
        // The caller's own disk, asking Java which one that is if need be.
        None => match process.io.disk() {
            Ok(disk) => disk,
            Err(_) => {
                let mut host = process.io.host.borrow_mut();
                if let Some(request) = host.needed.take() {
                    host.awaiting = true;
                    return Ok(ImportAction::Suspend(request));
                }
                drop(host);
                return spawn_failed(process, STATUS_ERROR, "no disk to load programs from".into());
            }
        },
    };
    let load = cache::load(&process.io, &disk, &path);
    let mut host = process.io.host.borrow_mut();
    if !matches!(load, Load::Pending) {
        host.answers.clear();
    }
    let (status, failure) = match load {
        Load::Pending => {
            let request = host.needed.take().expect("a pending load records its request");
            host.awaiting = true;
            return Ok(ImportAction::Suspend(request));
        }
        Load::Ready(module) => match cache::instantiate_compiled(&module, available) {
            Ok(executor) => return Ok(child(executor)),
            Err(e) => (STATUS_ERROR, format!("{path}: {e:#}")),
        },
        Load::Failed(message) => (STATUS_ERROR, message),
        Load::NotFound => (STATUS_NOT_FOUND, format!("{path}: no such file")),
    };
    drop(host);
    spawn_failed(process, status, failure)
}

fn write_guest(executor: &mut WackasmExecutor, ptr: Value, cap: Value, bytes: &[u8]) -> Result<u32, BadArgument> {
    let n = bytes.len().min(cap.as_u32() as usize);
    executor
        .write_memory(0, ptr.as_u32(), &bytes[..n])
        .map_err(|_| BadArgument("pointer out of bounds".into()))?;
    Ok(n as u32)
}

fn handle_import(
    state: &mut State,
    active: usize,
    work_dir: &WorkDir,
    key: &str,
    total: Option<u64>,
    name: &str,
    args: &[Value],
) -> Result<ImportAction> {
    use crate::abi::imports::*;

    let arity = match name {
        INVOKE | SPAWN_FILE => 6,
        INVOKE_VALUE => 5,
        SPAWN => 4,
        PULL_SIGNAL | PULL_SIGNAL_MS | SHUTDOWN | RELEASE_VALUE => 1,
        PUSH_SIGNAL | COMPONENTS | METHODS | BEEP | RESPONSE_READ | ADDRESS | LOG | CRASH | EXEC
        | RESUME | SPAWN_WITH => 2,
        RESPONSE_LEN | UPTIME | UPTIME_MS | MEMORY_LIMIT | KILL | FILES_DISK => 0,
        _ => bail!("unknown import oc.{name}"),
    };
    if args.len() != arity {
        bail!("oc.{name} expects {arity} parameters, got {}", args.len());
    }
    if name == SPAWN_FILE || name == SPAWN_WITH {
        return spawn_child(state, active, total, args, name == SPAWN_WITH);
    }
    if name == FILES_DISK {
        return files_disk(&mut state.processes[active]);
    }

    let available = state.available(total);
    let uptime = state.uptime;
    let address = state.address.as_bytes();
    let (below, above) = state.processes.split_at_mut(active + 1);
    let process = &mut below[active];
    let child = above.first_mut();
    let exec = &mut process.executor;
    let result: Result<ImportAction, BadArgument> = (|| {
        Ok(match name {
            INVOKE => ImportAction::Suspend(Request::Invoke {
                address: guest_str(exec, args[0], args[1])?,
                method: guest_str(exec, args[2], args[3])?,
                args: guest_array(exec, args[4], args[5])?,
            }),
            INVOKE_VALUE => ImportAction::Suspend(Request::InvokeValue {
                value: args[0].as_u32(),
                method: guest_str(exec, args[1], args[2])?,
                args: guest_array(exec, args[3], args[4])?,
            }),
            RELEASE_VALUE => ImportAction::Suspend(Request::ReleaseValue { value: args[0].as_u32() }),
            PULL_SIGNAL | PULL_SIGNAL_MS => {
                let timeout = if name == PULL_SIGNAL_MS {
                    args[0].as_i64() as f64 / 1000.0
                } else {
                    args[0].as_f64()
                };
                let deadline = *process.pull_deadline.get_or_insert(if timeout < 0.0 || timeout.is_nan() {
                    f64::INFINITY
                } else {
                    uptime + timeout
                });
                let remaining = deadline.is_finite().then(|| (deadline - uptime).max(0.0));
                ImportAction::Suspend(Request::PullSignal { remaining })
            }
            PUSH_SIGNAL => {
                let signal = guest_array(exec, args[0], args[1])?;
                if signal.first().and_then(OcValue::as_str).is_none() {
                    return Err(BadArgument("signal name must be a string".into()));
                }
                ImportAction::Suspend(Request::PushSignal { signal })
            }
            COMPONENTS => ImportAction::Suspend(Request::Components {
                filter: guest_str(exec, args[0], args[1])?,
            }),
            METHODS => ImportAction::Suspend(Request::Methods {
                address: guest_str(exec, args[0], args[1])?,
            }),
            BEEP => ImportAction::Suspend(Request::Beep {
                frequency: args[0].as_i32(),
                duration_ms: args[1].as_i32(),
            }),
            RESPONSE_LEN => ImportAction::Return(vec![Value::from_u32(process.response.len() as u32)]),
            RESPONSE_READ => {
                let n = write_guest(exec, args[0], args[1], &process.response)?;
                ImportAction::Return(vec![Value::from_u32(n)])
            }
            UPTIME => ImportAction::Return(vec![Value::from_f64(uptime)]),
            UPTIME_MS => ImportAction::Return(vec![Value::from_i64((uptime * 1000.0) as i64)]),
            ADDRESS => {
                write_guest(exec, args[0], args[1], address)?;
                ImportAction::Return(vec![Value::from_u32(address.len() as u32)])
            }
            MEMORY_LIMIT => {
                let limit = exec.memory().limit().unwrap_or(u64::MAX);
                ImportAction::Return(vec![Value::from_i64(limit.min(i64::MAX as u64) as i64)])
            }
            LOG => {
                let line = String::from_utf8_lossy(guest_bytes(exec, args[0], args[1])?).into_owned();
                // Logging is best-effort; a full disk shouldn't crash the guest.
                let _ = work_dir.append_log(key, &line);
                ImportAction::Return(Vec::new())
            }
            EXEC => {
                let image = guest_bytes(exec, args[0], args[1])?;
                // The replacement may also use what this process holds now.
                let own: u64 = (0..).map_while(|i| exec.memory().bytes(i)).map(|m| m.len() as u64).sum();
                match instantiate(image, available.map(|a| a + own)) {
                    Ok(next) => ImportAction::Replace(next),
                    // The caller keeps running and can report the failure itself.
                    Err(e) => {
                        process.response = format!("{e:#}").into_bytes();
                        ImportAction::Return(vec![Value::from_i32(STATUS_ERROR)])
                    }
                }
            }
            SPAWN => {
                if child.is_some() {
                    return Err(BadArgument("a child process is already running; kill it first".into()));
                }
                let image = guest_bytes(exec, args[0], args[1])?;
                let argv = guest_argv(exec, args[2], args[3])?;
                match instantiate(image, available) {
                    Ok(executor) => {
                        let mut child = Process::new(executor, argv);
                        child.io.env = process.io.env.clone();
                        ImportAction::Spawn(child)
                    }
                    Err(e) => {
                        process.response = format!("{e:#}").into_bytes();
                        ImportAction::Return(vec![Value::from_i32(STATUS_ERROR)])
                    }
                }
            }
            RESUME => match child {
                Some(child) if child.paused => {
                    let input = guest_bytes(exec, args[0], args[1])?;
                    if child.io.waiting_input && input.is_empty() {
                        child.io.stdin_eof = true;
                    }
                    child.io.stdin.extend(input);
                    child.io.waiting_input = false;
                    ImportAction::Resume
                }
                _ => {
                    process.response = b"no paused child process to resume".to_vec();
                    ImportAction::Return(vec![Value::from_i32(STATUS_ERROR)])
                }
            },
            KILL => {
                process.response.clear();
                ImportAction::Kill
            }
            SHUTDOWN => ImportAction::Halt(Status::Shutdown {
                reboot: args[0].as_i32() != 0,
            }),
            CRASH => {
                let message = String::from_utf8_lossy(guest_bytes(exec, args[0], args[1])?).into_owned();
                ImportAction::Halt(Status::Crashed(message))
            }
            _ => unreachable!("arity table covers every import"),
        })
    })();

    Ok(match result {
        Ok(action) => action,
        Err(BadArgument(message)) => match name {
            // These return nothing, so there's no status to report through.
            LOG | CRASH => bail!("oc.{name}: {message}"),
            _ => {
                state.processes[active].response = message.into_bytes();
                ImportAction::Return(vec![Value::from_i32(STATUS_BAD_ARGUMENT)])
            }
        },
    })
}

/// WASI preview 1, through `wackasm-wasi`'s marshalling over [`ProcessIo`].
fn handle_wasi(
    state: &mut State,
    active: usize,
    name: &str,
    args: Vec<Value>,
    should_return: usize,
) -> Result<ImportAction> {
    let process = &mut state.processes[active];
    process.io.uptime = state.uptime;
    let reads_stdin = name == "fd_read" && args.first().is_some_and(|fd| fd.as_u32() == 0);
    match name {
        "proc_exit" => return Ok(ImportAction::Exit(args.first().map_or(0, |v| v.as_i32()))),
        _ if reads_stdin && process.io.stdin.is_empty() && !process.io.stdin_eof => {
            if active == 0 {
                // Nobody can type into the BIOS/OS's stdin.
                process.io.stdin_eof = true;
            } else if !process.io.output.is_empty() {
                // A prompt written without a newline shows before the wait.
                // fd_read stays pending and asks for input when retried.
                return Ok(ImportAction::Pause(Event::Output(std::mem::take(&mut process.io.output))));
            } else {
                process.io.waiting_input = true;
                return Ok(ImportAction::Pause(Event::Input));
            }
        }
        _ => {}
    }

    let call = <Wasi1 as WackasmImports<ProcessIo, NoResources>>::get_import_func(&Wasi1, WASI_PREVIEW1, name);
    let Some(call) = call else {
        // Every WASI function returns an errno, so refuse unknown ones politely.
        if should_return == 1 {
            return Ok(ImportAction::Return(vec![Value::from_i32(WASI_ENOSYS)]));
        }
        bail!("unsupported WASI import {name}");
    };
    // Run it (again, if it has made disk calls before; see `crate::wasi`).
    {
        let mut host = process.io.host.borrow_mut();
        host.cursor = 0;
        host.needed = None;
    }
    let results = call.call(&mut process.io, &mut process.executor, &WackasmLinkerResources::default(), args)?;
    let mut host = process.io.host.borrow_mut();
    if let Some(request) = host.needed.take() {
        // The results are the placeholder failure; the import stays pending.
        host.awaiting = true;
        return Ok(ImportAction::Suspend(request));
    }
    host.answers.clear();
    Ok(ImportAction::Return(results))
}
