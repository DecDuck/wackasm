//! Running other programs, in the style of `std::process`.
//!
//! WASI preview 1 has no way to start a process, so this goes through the
//! architecture's own process model: a program can run one child at a time,
//! which runs while the parent waits, and pauses whenever it has something
//! for the parent (output, a read from stdin, or its exit). [`Child`] exposes
//! that as a stream of [`Event`]s. [`Command::status`] and [`Child::wait`]
//! relay it to this program's own stdio, which is how output from a program
//! run by a program run from the shell reaches the screen.
//!
//! ```no_run
//! use wackasm_opencomputers::process::Command;
//!
//! // Run with this program's stdin and stdout.
//! let status = Command::new("hello").arg("world").status()?;
//! println!("hello exited with {status}");
//!
//! // Capture the output instead.
//! let output = Command::new("cat").arg("notes.txt").output()?;
//! println!("{} bytes", output.stdout.len());
//! # Ok::<(), wackasm_opencomputers::Error>(())
//! ```
//!
//! Children start with this program's environment (plus changes made with
//! [`Command::env`] and friends) and see the same disk as `/`. stderr is
//! merged into stdout.
//!
//! # Working directory
//!
//! `std::env::current_dir` and `std::env::set_current_dir` work as usual, and
//! relative paths in `std::fs` start at the current directory. A child
//! starts in this program's current directory, or the one given to
//! [`Command::current_dir`]. It gets that directory as `PWD`, the way Unix
//! shells pass it on.
//!
//! WASI programs always start in `/`, so something has to change into `PWD`
//! before `main`. This crate does that, in a constructor that runs before
//! `main` whenever the linker keeps it. It always does with LTO (as in the
//! example programs' release profile), and when the program calls into
//! [`process`](self). Otherwise, for example in a debug build that only uses
//! `std::fs`, call [`enter_working_dir`] at the top of `main`. A program that
//! doesn't link this crate at all starts in `/`, but can read `PWD` itself.

use std::{
    collections::VecDeque,
    fmt,
    io::{Read, Write},
    path::Path,
    sync::Once,
};

use crate::{
    component::{self, Address, Component},
    components::Eeprom,
    error::{Error, Result},
    sys,
    value::{OcString, Value},
};

/// How a program ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExitStatus {
    /// It exited (`proc_exit`, or returned from `main`) with this code.
    Exited(i32),
    /// It trapped; the message says why.
    Crashed(String),
}

impl ExitStatus {
    pub fn success(&self) -> bool {
        matches!(self, ExitStatus::Exited(0))
    }

    /// The exit code, unless it crashed.
    pub fn code(&self) -> Option<i32> {
        match self {
            ExitStatus::Exited(code) => Some(*code),
            ExitStatus::Crashed(_) => None,
        }
    }
}

impl fmt::Display for ExitStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExitStatus::Exited(code) => write!(f, "exit status: {code}"),
            ExitStatus::Crashed(message) => write!(f, "crashed: {message}"),
        }
    }
}

/// What a captured run produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Output {
    pub status: ExitStatus,
    /// stdout and stderr, interleaved as written.
    pub stdout: Vec<u8>,
}

/// Something a running child did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// It wrote to stdout or stderr.
    Output(Vec<u8>),
    /// It is waiting to read stdin. Give it data with [`Child::write_stdin`],
    /// or end of file with [`Child::close_stdin`], before the next
    /// [`Child::next_event`]. Giving it neither counts as end of file.
    Input,
    /// It finished. Any last output came first, as an `Output` event.
    Exit(ExitStatus),
}

/// A program to run.
#[derive(Clone, Debug)]
pub struct Command {
    program: String,
    image: Option<Vec<u8>>,
    args: Vec<String>,
    env: Vec<(String, Option<String>)>,
    env_clear: bool,
    disk: Option<Address>,
    cwd: Option<String>,
}

impl Command {
    /// A program by name (looked up in each directory of `PATH`, default
    /// `/bin`, as `NAME.wasm` and then `NAME`) or by path (when it contains
    /// a `/`). Disks are searched in order: this program's own, the boot
    /// disk, then the rest, unless [`Command::disk`] picks one.
    pub fn new(program: impl Into<String>) -> Command {
        Command {
            program: program.into(),
            image: None,
            args: Vec::new(),
            env: Vec::new(),
            env_clear: false,
            disk: None,
            cwd: None,
        }
    }

    /// A program from a wasm binary in memory. `name` is its `argv[0]`.
    pub fn from_image(name: impl Into<String>, image: impl Into<Vec<u8>>) -> Command {
        Command { image: Some(image.into()), ..Command::new(name) }
    }

    pub fn arg(&mut self, arg: impl Into<String>) -> &mut Command {
        self.args.push(arg.into());
        self
    }

    pub fn args<I: IntoIterator<Item = S>, S: Into<String>>(&mut self, args: I) -> &mut Command {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    pub fn env(&mut self, key: impl Into<String>, value: impl Into<String>) -> &mut Command {
        self.env.push((key.into(), Some(value.into())));
        self
    }

    pub fn envs<I: IntoIterator<Item = (K, V)>, K: Into<String>, V: Into<String>>(&mut self, vars: I) -> &mut Command {
        self.env.extend(vars.into_iter().map(|(k, v)| (k.into(), Some(v.into()))));
        self
    }

    pub fn env_remove(&mut self, key: impl Into<String>) -> &mut Command {
        self.env.push((key.into(), None));
        self
    }

    /// Start from an empty environment instead of this program's.
    pub fn env_clear(&mut self) -> &mut Command {
        self.env_clear = true;
        self.env.clear();
        self
    }

    /// Load the program from this filesystem only.
    pub fn disk(&mut self, disk: impl Into<Address>) -> &mut Command {
        self.disk = Some(disk.into());
        self
    }

    /// Runs the child in `dir` instead of this program's current directory.
    /// A relative `dir` starts at the current directory. A relative program
    /// path (`./tool.wasm`) is looked up from the child's directory.
    pub fn current_dir(&mut self, dir: impl AsRef<Path>) -> &mut Command {
        self.cwd = Some(dir.as_ref().to_string_lossy().into_owned());
        self
    }

    pub fn get_current_dir(&self) -> Option<&Path> {
        self.cwd.as_deref().map(Path::new)
    }

    pub fn get_program(&self) -> &str {
        &self.program
    }

    pub fn get_args(&self) -> &[String] {
        &self.args
    }

    /// Starts the program and runs it to its first event.
    pub fn spawn(&mut self) -> Result<Child> {
        let mut base = vec![
            ("argv", Value::from(std::iter::once(self.program.clone()).chain(self.args.iter().cloned()).collect::<Vec<_>>())),
            ("env", Value::from(self.environment())),
        ];
        if let Some(dir) = self.child_dir() {
            base.push(("cwd", Value::from(dir)));
        }
        if let Some(image) = &self.image {
            base.push(("image", Value::String(OcString(image.clone()))));
            return Child::start(&spec(base)).map(|c| c.expect("images are never missing"));
        }
        let disks: Vec<Option<Address>> = match &self.disk {
            Some(disk) => vec![Some(disk.clone())],
            None => search_disks(),
        };
        for disk in disks {
            for path in self.candidate_paths() {
                let mut entries = base.clone();
                entries.push(("path", Value::from(path)));
                if let Some(disk) = &disk {
                    entries.push(("disk", Value::from(disk)));
                }
                if let Some(child) = Child::start(&spec(entries))? {
                    return Ok(child);
                }
            }
        }
        Err(Error::ProgramNotFound(self.program.clone()))
    }

    /// Runs the program with this program's stdin and stdout, and waits for it.
    pub fn status(&mut self) -> Result<ExitStatus> {
        self.spawn()?.wait()
    }

    /// Runs the program with no input (stdin is at end of file), capturing
    /// its output.
    pub fn output(&mut self) -> Result<Output> {
        let mut child = self.spawn()?;
        child.close_stdin();
        child.wait_with_output()
    }

    fn environment(&self) -> Vec<String> {
        let mut vars: Vec<(String, String)> = if self.env_clear { Vec::new() } else { std::env::vars().collect() };
        for (key, value) in &self.env {
            vars.retain(|(k, _)| k != key);
            if let Some(value) = value {
                vars.push((key.clone(), value.clone()));
            }
        }
        vars.into_iter().map(|(k, v)| format!("{k}={v}")).collect()
    }

    /// The child's working directory, made absolute here because only this
    /// program knows its own current directory (the host only knows the `PWD`
    /// it started with). The host normalizes `..`.
    fn child_dir(&self) -> Option<String> {
        let here = std::env::current_dir().ok().map(|dir| dir.to_string_lossy().into_owned());
        match (&self.cwd, here) {
            (Some(dir), Some(here)) if !dir.starts_with('/') => Some(format!("{}/{dir}", here.trim_end_matches('/'))),
            (Some(dir), _) => Some(dir.clone()),
            (None, here) => here,
        }
    }

    fn candidate_paths(&self) -> Vec<String> {
        if self.program.contains('/') {
            return vec![self.program.clone()];
        }
        let path = self
            .env
            .iter()
            .rev()
            .find(|(k, _)| k == "PATH")
            .map(|(_, v)| v.clone().unwrap_or_default())
            .or_else(|| (!self.env_clear).then(|| std::env::var("PATH").ok()).flatten())
            .unwrap_or_else(|| "/bin".into());
        path.split(':')
            .filter(|dir| !dir.is_empty())
            .flat_map(|dir| {
                let dir = dir.trim_end_matches('/');
                [format!("{dir}/{}.wasm", self.program), format!("{dir}/{}", self.program)]
            })
            .collect()
    }
}

fn spec(entries: Vec<(&str, Value)>) -> Vec<u8> {
    Value::Map(entries.into_iter().map(|(k, v)| (Value::from(k), v)).collect()).encode()
}

/// This program's disk (`None`, the host's default), then the boot disk,
/// then every other filesystem.
fn search_disks() -> Vec<Option<Address>> {
    let mut disks = vec![None];
    let boot = Eeprom::first()
        .and_then(|eeprom| eeprom.get_data().ok())
        .and_then(|data| data.as_str().map(|s| Address(s.to_owned())))
        .filter(|address| !address.as_str().is_empty());
    let mut rest: Vec<Address> = component::list("filesystem").into_iter().map(|(a, _)| a).collect();
    if let Some(boot) = boot {
        rest.retain(|a| *a != boot);
        disks.push(Some(boot));
    }
    disks.extend(rest.into_iter().map(Some));
    disks
}

/// Changes into the directory `PWD` names, if it's not `/`. The constructor
/// described in the [module docs](self#working-directory) does this before
/// `main`. Calling it again does nothing, so it's safe at the top of `main`
/// either way.
pub fn enter_working_dir() {
    static ENTERED: Once = Once::new();
    ENTERED.call_once(|| {
        if let Ok(dir) = std::env::var("PWD") {
            if dir.starts_with('/') && dir != "/" {
                let _ = std::env::set_current_dir(dir);
            }
        }
    });
}

/// Runs [`enter_working_dir`] before `main` (wasi-libc calls `.init_array`
/// entries from `_start`).
#[cfg(target_os = "wasi")]
#[used]
#[unsafe(link_section = ".init_array")]
static ENTER_WORKING_DIR: extern "C" fn() = {
    extern "C" fn enter() {
        enter_working_dir();
    }
    enter
};

/// A running program. Dropping it before it exits kills it.
#[derive(Debug)]
pub struct Child {
    events: VecDeque<Event>,
    /// The child is paused and needs a `resume` before its next event.
    paused: bool,
    wants_input: bool,
    stdin: Vec<u8>,
    stdin_closed: bool,
    finished: bool,
}

impl Child {
    /// `Ok(None)` if there is no such file.
    fn start(spec: &[u8]) -> Result<Option<Child>> {
        let status = unsafe { sys::spawn_with(spec.as_ptr(), spec.len()) };
        if status == sys::STATUS_NOT_FOUND {
            return Ok(None);
        }
        let mut child = Child { events: VecDeque::new(), paused: false, wants_input: false, stdin: Vec::new(), stdin_closed: false, finished: false };
        child.take(status)?;
        Ok(Some(child))
    }

    /// Records the event a `spawn_with`/`resume` returned.
    fn take(&mut self, status: i32) -> Result<()> {
        if status < 0 {
            self.finished = true;
            return Err(Error::Failed(String::from_utf8_lossy(&sys::response()).into_owned()));
        }
        let items = match Value::decode(&sys::response()) {
            Some(Value::Array(items)) => items,
            _ => Vec::new(),
        };
        let bytes = |i: usize| items.get(i).and_then(Value::as_bytes).map(<[u8]>::to_vec).unwrap_or_default();
        self.paused = true;
        self.wants_input = false;
        match status {
            sys::EVENT_OUTPUT => self.events.push_back(Event::Output(bytes(0))),
            sys::EVENT_INPUT => {
                self.wants_input = true;
                self.events.push_back(Event::Input);
            }
            sys::EVENT_EXITED | sys::EVENT_CRASHED => {
                self.finished = true;
                self.paused = false;
                let output = bytes(1);
                if !output.is_empty() {
                    self.events.push_back(Event::Output(output));
                }
                let status = if status == sys::EVENT_EXITED {
                    ExitStatus::Exited(items.first().and_then(Value::as_i64).unwrap_or(0) as i32)
                } else {
                    ExitStatus::Crashed(items.first().and_then(Value::as_str).unwrap_or("crashed").to_owned())
                };
                self.events.push_back(Event::Exit(status));
            }
            other => return Err(Error::UnexpectedReply(format!("process event {other}"))),
        }
        Ok(())
    }

    /// The next thing the child does, running it until then.
    pub fn next_event(&mut self) -> Result<Event> {
        if let Some(event) = self.events.pop_front() {
            return Ok(event);
        }
        if self.finished || !self.paused {
            return Err(Error::Failed("the process has exited".into()));
        }
        // Answering an input request with nothing means end of file.
        let input = std::mem::take(&mut self.stdin);
        if self.wants_input && input.is_empty() {
            self.stdin_closed = true;
        }
        self.paused = false;
        let status = unsafe { sys::resume(input.as_ptr(), input.len()) };
        self.take(status)?;
        self.events.pop_front().ok_or_else(|| Error::UnexpectedReply("no process event".into()))
    }

    /// Queues bytes for the child's stdin, delivered when it next runs.
    pub fn write_stdin(&mut self, bytes: &[u8]) {
        self.stdin.extend_from_slice(bytes);
    }

    /// Ends the child's stdin: its next read past what is queued gets end of file.
    pub fn close_stdin(&mut self) {
        self.stdin_closed = true;
    }

    /// Stops the child now.
    pub fn kill(&mut self) {
        if !self.finished {
            unsafe { sys::kill() };
            self.finished = true;
            self.paused = false;
            self.events.clear();
        }
    }

    /// Relays the child's output to this program's stdout and its reads to
    /// this program's stdin, until it exits.
    pub fn wait(mut self) -> Result<ExitStatus> {
        loop {
            match self.next_event()? {
                Event::Output(bytes) => {
                    let mut out = std::io::stdout().lock();
                    let _ = out.write_all(&bytes);
                    let _ = out.flush();
                }
                Event::Input => {
                    if self.stdin_closed {
                        continue;
                    }
                    let mut buf = [0u8; 4096];
                    match std::io::stdin().read(&mut buf) {
                        Ok(n) if n > 0 => self.write_stdin(&buf[..n]),
                        _ => self.close_stdin(),
                    }
                }
                Event::Exit(status) => return Ok(status),
            }
        }
    }

    /// Collects the child's output until it exits. Its reads get whatever was
    /// queued with [`Child::write_stdin`], then end of file.
    pub fn wait_with_output(mut self) -> Result<Output> {
        let mut stdout = Vec::new();
        loop {
            match self.next_event()? {
                Event::Output(bytes) => stdout.extend(bytes),
                Event::Input => self.close_stdin(),
                Event::Exit(status) => return Ok(Output { status, stdout }),
            }
        }
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        self.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_programs_up_on_path() {
        let mut cmd = Command::new("cat");
        cmd.env("PATH", "/bin:/usr/bin/");
        assert_eq!(cmd.candidate_paths(), ["/bin/cat.wasm", "/bin/cat", "/usr/bin/cat.wasm", "/usr/bin/cat"]);
        assert_eq!(Command::new("./tools/x.wasm").candidate_paths(), ["./tools/x.wasm"]);
        let mut bare = Command::new("ls");
        bare.env_clear();
        assert_eq!(bare.candidate_paths(), ["/bin/ls.wasm", "/bin/ls"]);
    }

    #[test]
    fn builds_the_environment() {
        let mut cmd = Command::new("x");
        cmd.env_clear().env("A", "1").env("B", "2").env("A", "3").env_remove("B");
        assert_eq!(cmd.environment(), ["A=3"]);
    }
}
