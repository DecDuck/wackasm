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

use anyhow::{Context as _, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use wackasm::{HostYield, Value, WackasmExecutor, WackasmModule};

use crate::{abi::*, storage::WorkDir, value::OcValue};

const SNAPSHOT_MAGIC: &[u8; 4] = b"WKOC";
// v2: MemoryManager carries the host memory limit.
const SNAPSHOT_VERSION: u32 = 2;

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
    executor: WackasmExecutor,
    status: Status,
    /// The request last reported to Java and not yet answered.
    request: Option<Request>,
    /// Bytes the guest reads back with `response_read`.
    response: Vec<u8>,
    /// Absolute uptime at which the pending `pull_signal` expires (`INFINITY` = never).
    pull_deadline: Option<f64>,
    /// Uptime passed to the most recent `run`.
    uptime: f64,
}

pub struct OcMachine {
    work_dir: WorkDir,
    key: String,
    address: String,
    /// Host memory budget. wackasm enforces it in `memory.grow`, so a guest
    /// sees allocation failure rather than being killed.
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
            state.executor.set_memory_limit(self.memory_limit);
        }
    }

    pub fn is_booted(&self) -> bool {
        self.state.is_some()
    }

    /// Starts a fresh program. If `eeprom` holds a wasm binary it is used as
    /// the image; otherwise the server's `<work_dir>/boot.wasm` is.
    pub fn boot(&mut self, eeprom: Option<&[u8]>) -> Result<()> {
        self.state = None;
        self.work_dir.remove_state(&self.key)?;

        let image = match eeprom {
            Some(bytes) if bytes.starts_with(WASM_MAGIC) => bytes.to_vec(),
            _ => self.work_dir.default_boot_image()?.ok_or_else(|| {
                anyhow!("no bootable image: EEPROM is not wasm and boot.wasm is missing")
            })?,
        };

        let module = match self.memory_limit {
            Some(limit) => WackasmModule::with_memory_limit(&image, limit),
            None => WackasmModule::new(&image),
        }
        .context("invalid wasm module")?;
        let mut executor = WackasmExecutor::new(module).context("instantiating module")?;
        ENTRY_POINTS
            .iter()
            .find(|name| executor.call_export(name, Vec::new()).is_ok())
            .ok_or_else(|| anyhow!("module exports none of {ENTRY_POINTS:?}"))?;

        self.state = Some(State {
            address: self.address.clone(),
            executor,
            status: Status::Running,
            request: None,
            response: Vec::new(),
            pull_deadline: None,
            uptime: 0.0,
        });
        Ok(())
    }

    /// Runs one time slice. `Err` is returned only for host-side misuse (not
    /// booted). Guest failures are reported as [`RunOutcome::Error`], and the
    /// machine stays crashed after that.
    pub fn run(&mut self, uptime: f64, fuel: isize) -> Result<RunOutcome> {
        let state = self.state.as_mut().ok_or_else(|| anyhow!("machine not booted"))?;
        state.uptime = uptime;

        if let Some(outcome) = terminal_outcome(&state.status) {
            return Ok(outcome);
        }

        state.executor.set_fuel(fuel);
        let outcome = match step(state, &self.work_dir, &self.key) {
            Ok(outcome) => outcome,
            Err(error) => {
                let message = format!("{error:#}");
                state.status = Status::Crashed(message.clone());
                state.request = None;
                RunOutcome::Error(message)
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

    /// Answers the pending request and unblocks the guest. `payload` is an
    /// encoded value when `status` is `STATUS_OK`, and a UTF-8 message otherwise.
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

        if matches!(request, Request::PullSignal { .. }) {
            state.pull_deadline = None;
        }
        state.response = payload;
        state
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
        // keep the one saved in the snapshot.
        match self.memory_limit {
            Some(_) => state.executor.set_memory_limit(self.memory_limit),
            None => self.memory_limit = state.executor.memory().limit(),
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

fn terminal_outcome(status: &Status) -> Option<RunOutcome> {
    match status {
        Status::Running => None,
        Status::Halted => Some(RunOutcome::Halted),
        Status::Shutdown { reboot } => Some(RunOutcome::Shutdown { reboot: *reboot }),
        Status::Crashed(msg) => Some(RunOutcome::Error(msg.clone())),
    }
}

fn step(state: &mut State, work_dir: &WorkDir, key: &str) -> Result<RunOutcome> {
    loop {
        let yielded = state.executor.execute()?;

        let (module, name, args, should_return) = match yielded {
            HostYield::OutOfFuel => return Ok(RunOutcome::Yield),
            HostYield::Return => {
                state.status = Status::Halted;
                return Ok(RunOutcome::Halted);
            }
            HostYield::ImportCall {
                module,
                name,
                args,
                should_return,
            } => (module, name, args, should_return),
        };

        if module != IMPORT_MODULE {
            bail!("unknown import {module}.{name}");
        }
        match handle_import(state, work_dir, key, &name, &args)? {
            ImportAction::Return(results) => {
                if results.len() != should_return {
                    bail!(
                        "{module}.{name} declared {should_return} results, expected {}",
                        results.len()
                    );
                }
                state.executor.import_return(&results)?;
            }
            ImportAction::Suspend(request) => return Ok(RunOutcome::Request(request)),
            ImportAction::Halt(status) => {
                let outcome = terminal_outcome(&status).expect("halt status is terminal");
                state.status = status;
                return Ok(outcome);
            }
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

fn write_guest(executor: &mut WackasmExecutor, ptr: Value, cap: Value, bytes: &[u8]) -> Result<u32, BadArgument> {
    let n = bytes.len().min(cap.as_u32() as usize);
    executor
        .write_memory(0, ptr.as_u32(), &bytes[..n])
        .map_err(|_| BadArgument("pointer out of bounds".into()))?;
    Ok(n as u32)
}

fn handle_import(
    state: &mut State,
    work_dir: &WorkDir,
    key: &str,
    name: &str,
    args: &[Value],
) -> Result<ImportAction> {
    use crate::abi::imports::*;

    let arity = match name {
        INVOKE => 6,
        PULL_SIGNAL | SHUTDOWN => 1,
        PUSH_SIGNAL | COMPONENTS | METHODS | BEEP | RESPONSE_READ | ADDRESS | LOG | CRASH => 2,
        RESPONSE_LEN | UPTIME | MEMORY_LIMIT => 0,
        _ => bail!("unknown import oc.{name}"),
    };
    if args.len() != arity {
        bail!("oc.{name} expects {arity} parameters, got {}", args.len());
    }

    let exec = &mut state.executor;
    let result: Result<ImportAction, BadArgument> = (|| {
        Ok(match name {
            INVOKE => ImportAction::Suspend(Request::Invoke {
                address: guest_str(exec, args[0], args[1])?,
                method: guest_str(exec, args[2], args[3])?,
                args: guest_array(exec, args[4], args[5])?,
            }),
            PULL_SIGNAL => {
                let timeout = args[0].as_f64();
                let deadline = *state.pull_deadline.get_or_insert(if timeout < 0.0 || timeout.is_nan() {
                    f64::INFINITY
                } else {
                    state.uptime + timeout
                });
                let remaining = deadline.is_finite().then(|| (deadline - state.uptime).max(0.0));
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
            RESPONSE_LEN => ImportAction::Return(vec![Value::from_u32(state.response.len() as u32)]),
            RESPONSE_READ => {
                let n = write_guest(exec, args[0], args[1], &state.response)?;
                ImportAction::Return(vec![Value::from_u32(n)])
            }
            UPTIME => ImportAction::Return(vec![Value::from_f64(state.uptime)]),
            ADDRESS => {
                write_guest(exec, args[0], args[1], state.address.as_bytes())?;
                ImportAction::Return(vec![Value::from_u32(state.address.len() as u32)])
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
                state.response = message.into_bytes();
                ImportAction::Return(vec![Value::from_i32(STATUS_BAD_ARGUMENT)])
            }
        },
    })
}

