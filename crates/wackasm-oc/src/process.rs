//! Processes: the guests on a machine, stacked parent above child.
//!
//! The BIOS starts as process 0. A process can `oc.spawn` a child, which then
//! runs while the parent waits inside `spawn`/`resume`. The child runs until
//! it has something for the parent: an [`Event`]. The child then *pauses*,
//! and the parent's import returns the event. Only one process runs at a
//! time, and every process is part of the machine snapshot.
//!
//! Children usually speak WASI preview 1, so [`ProcessIo`] gives each
//! process WASI stdio:
//! * each write to stdout or stderr is handed to the parent as `OUTPUT`;
//! * reading stdin with nothing buffered pauses the child with `INPUT` until
//!   the parent `resume`s it with a line (or with nothing, meaning end of file);
//! * argv is whatever the parent passed to `spawn`;
//! * files live on an OpenComputers filesystem (see [`crate::wasi`]).

use std::{cell::RefCell, collections::{BTreeMap, VecDeque}};

use serde::{Deserialize, Serialize};
use wackasm::WackasmExecutor;

use crate::{
    abi::*,
    value::OcValue,
    wasi::{Disk, HostCalls, OpenFile},
};

#[derive(Serialize, Deserialize)]
pub(crate) struct Process {
    pub executor: WackasmExecutor,
    /// Bytes this process reads back with `response_read`.
    pub response: Vec<u8>,
    /// Absolute uptime at which the pending `pull_signal` expires (`INFINITY` = never).
    pub pull_deadline: Option<f64>,
    pub io: ProcessIo,
    /// This child has delivered an event and waits for its parent's `resume`.
    pub paused: bool,
    /// Unique for the machine's lifetime (process 0 is 0). Async calls belong
    /// to a process by id, so a call outliving its process is dropped.
    pub id: u64,
    /// The next `invoke_async` ticket.
    pub next_ticket: i64,
    /// Async calls that have completed and not been read yet.
    pub completions: VecDeque<Completion>,
    /// Tickets cancelled while Java was running them: their results are dropped.
    pub cancelled: Vec<i64>,
}

/// A finished async call, as `completion_next` returns it.
#[derive(Serialize, Deserialize)]
pub(crate) struct Completion {
    pub ticket: i64,
    pub status: i32,
    pub payload: Vec<u8>,
}

impl Process {
    pub fn new(executor: WackasmExecutor, args: Vec<String>) -> Self {
        Self {
            executor,
            response: Vec::new(),
            pull_deadline: None,
            io: ProcessIo {
                args,
                ..ProcessIo::default()
            },
            paused: false,
            id: 0,
            next_ticket: 0,
            completions: VecDeque::new(),
            cancelled: Vec::new(),
        }
    }

    pub fn memory_used(&self) -> u64 {
        (0..)
            .map_while(|i| self.executor.memory().bytes(i))
            .map(|m| m.len() as u64)
            .sum()
    }
}

/// Something a child hands its parent, as the parent's `spawn`/`resume` result.
pub(crate) enum Event {
    /// With whatever output the child wrote last.
    Exited { code: i32, output: Vec<u8> },
    Output(Vec<u8>),
    Input,
    Crashed { message: String, output: Vec<u8> },
}

impl Event {
    pub fn code(&self) -> i32 {
        match self {
            Event::Exited { .. } => EVENT_EXITED,
            Event::Output(_) => EVENT_OUTPUT,
            Event::Input => EVENT_INPUT,
            Event::Crashed { .. } => EVENT_CRASHED,
        }
    }

    /// What the parent finds in its response buffer.
    pub fn payload(self) -> Vec<u8> {
        OcValue::Array(match self {
            Event::Exited { code, output } => vec![OcValue::Int(code as i64), OcValue::Bytes(output)],
            Event::Output(bytes) => vec![OcValue::Bytes(bytes)],
            Event::Input => Vec::new(),
            Event::Crashed { message, output } => vec![OcValue::String(message), OcValue::Bytes(output)],
        })
        .encode()
    }
}

#[derive(Default, Serialize, Deserialize)]
pub(crate) struct ProcessIo {
    pub args: Vec<String>,
    /// `NAME=value` pairs, for WASI `environ_get`.
    pub env: Vec<String>,
    pub stdin: VecDeque<u8>,
    /// The parent resumed with no input: the next empty read returns end of file.
    pub stdin_eof: bool,
    /// The child is paused inside `fd_read(0)`.
    pub waiting_input: bool,
    /// stdout and stderr, interleaved as written, not yet handed to the parent.
    pub output: Vec<u8>,
    /// Machine uptime in seconds, for the WASI clocks.
    pub uptime: f64,
    /// The filesystem component preopened as `/`; see [`crate::wasi`].
    pub disk: RefCell<Disk>,
    /// Disk calls of the WASI import in flight.
    pub host: RefCell<HostCalls>,
    /// Open files and directories, by fd (from 4; 3 is the root).
    pub files: BTreeMap<u32, OpenFile>,
    pub next_fd: u32,
}
