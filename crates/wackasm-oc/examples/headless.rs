//! Runs a machine against fake OpenComputers components and prints the screen.
//!
//! ```text
//! cargo run -p wackasm-oc --example headless -- --eeprom <bios.wasm> [--disk <dir>] [--ram <KiB>] [--keys "help\r"]
//! ```
//!
//! The simulated computer has:
//! * an EEPROM whose code is `--eeprom` (normally the wasm BIOS);
//! * a GPU bound to a 50x16 screen;
//! * a keyboard that types `--keys` (`\r` = enter, `\b` = backspace, `\d` = Ctrl+D);
//! * with `--disk`, a filesystem backed by that host directory, whose
//!   `open`/`read`/`close` hand out userdata handles like OpenComputers does.
//!
//! RAM defaults to 192 KiB, one tier 1 stick. Every few slices the machine is
//! also saved and reloaded, which exercises suspension.

use std::{collections::{HashMap, VecDeque}, path::PathBuf};

use anyhow::{Result, anyhow, bail};
use wackasm_oc::{
    OcMachine, OcValue, Request, RunOutcome, WorkDir,
    abi::{STATUS_ERROR, STATUS_NO_SUCH_COMPONENT, STATUS_OK, STATUS_TIMEOUT},
};

const GPU: &str = "gpu-0000";
const SCREEN: &str = "screen-0000";
const KEYBOARD: &str = "keyboard-0000";
const EEPROM: &str = "eeprom-0000";
const DISK: &str = "disk-0000";
const W: usize = 50;
const H: usize = 16;
/// OpenComputers' default `maxReadBuffer`.
const MAX_READ: usize = 2048;

/// An open file on the simulated disk; writes go straight through to the host.
struct Handle {
    path: String,
    data: Vec<u8>,
    pos: usize,
    writable: bool,
}

struct Sim {
    grid: Vec<Vec<char>>,
    keys: VecDeque<char>,
    beeps: usize,
    eeprom_code: Vec<u8>,
    eeprom_data: Vec<u8>,
    disk: Option<PathBuf>,
    handles: HashMap<u32, Handle>,
    next_handle: u32,
    /// Filesystem calls by method, to see what programs cost.
    fs_calls: std::collections::BTreeMap<String, usize>,
}

fn int(args: &[OcValue], i: usize) -> i64 {
    match args.get(i) {
        Some(OcValue::Int(v)) => *v,
        Some(OcValue::Float(v)) => *v as i64,
        _ => 0,
    }
}

fn text(args: &[OcValue], i: usize) -> Vec<u8> {
    match args.get(i) {
        Some(OcValue::String(s)) => s.as_bytes().to_vec(),
        Some(OcValue::Bytes(b)) => b.clone(),
        _ => Vec::new(),
    }
}

impl Sim {
    fn put(&mut self, x: i64, y: i64, c: char) {
        if (1..=W as i64).contains(&x) && (1..=H as i64).contains(&y) {
            self.grid[y as usize - 1][x as usize - 1] = c;
        }
    }

    fn gpu(&mut self, method: &str, args: &[OcValue]) -> Result<Vec<OcValue>, String> {
        Ok(match method {
            "bind" => vec![OcValue::Bool(true)],
            "getResolution" => vec![OcValue::Int(W as i64), OcValue::Int(H as i64)],
            "set" => {
                let (x, y) = (int(args, 0), int(args, 1));
                for (i, c) in String::from_utf8_lossy(&text(args, 2)).chars().enumerate() {
                    self.put(x + i as i64, y, c);
                }
                vec![OcValue::Bool(true)]
            }
            "fill" => {
                let (x, y, w, h) = (int(args, 0), int(args, 1), int(args, 2), int(args, 3));
                let c = String::from_utf8_lossy(&text(args, 4)).chars().next().unwrap_or(' ');
                for yy in y..y + h {
                    for xx in x..x + w {
                        self.put(xx, yy, c);
                    }
                }
                vec![OcValue::Bool(true)]
            }
            "copy" => {
                let (x, y, w, h, tx, ty) = (int(args, 0), int(args, 1), int(args, 2), int(args, 3), int(args, 4), int(args, 5));
                let snapshot = self.grid.clone();
                for yy in y..y + h {
                    for xx in x..x + w {
                        if (1..=W as i64).contains(&xx) && (1..=H as i64).contains(&yy) {
                            self.put(xx + tx, yy + ty, snapshot[yy as usize - 1][xx as usize - 1]);
                        }
                    }
                }
                vec![OcValue::Bool(true)]
            }
            other => return Err(format!("no such method: {other}")),
        })
    }

    fn eeprom(&mut self, method: &str, args: &[OcValue]) -> Result<Vec<OcValue>, String> {
        Ok(match method {
            "get" => vec![OcValue::Bytes(self.eeprom_code.clone())],
            "getData" => vec![OcValue::Bytes(self.eeprom_data.clone())],
            "setData" => {
                self.eeprom_data = text(args, 0);
                Vec::new()
            }
            other => return Err(format!("no such method: {other}")),
        })
    }

    fn filesystem(&mut self, method: &str, args: &[OcValue]) -> Result<Vec<OcValue>, String> {
        let root = self.disk.clone().expect("only listed when a disk exists");
        let path_arg = |i: usize| String::from_utf8_lossy(&text(args, i)).into_owned();
        let host = |path: &str| root.join(path.trim_start_matches('/'));
        let handle = || match args.first() {
            Some(OcValue::Userdata(id)) => Ok(*id),
            _ => Err("bad file descriptor".to_string()),
        };
        Ok(match method {
            "exists" => vec![OcValue::Bool(host(&path_arg(0)).exists())],
            "isDirectory" => vec![OcValue::Bool(host(&path_arg(0)).is_dir())],
            "size" => vec![OcValue::Int(std::fs::metadata(host(&path_arg(0))).map_or(0, |m| m.len()) as i64)],
            "lastModified" => vec![OcValue::Int(0)],
            "makeDirectory" => vec![OcValue::Bool(std::fs::create_dir_all(host(&path_arg(0))).is_ok())],
            "remove" => {
                let target = host(&path_arg(0));
                let ok = if target.is_dir() { std::fs::remove_dir_all(&target) } else { std::fs::remove_file(&target) };
                vec![OcValue::Bool(ok.is_ok())]
            }
            "rename" => vec![OcValue::Bool(std::fs::rename(host(&path_arg(0)), host(&path_arg(1))).is_ok())],
            "open" => {
                let path = path_arg(0);
                let mode = path_arg(1);
                // OC throws FileNotFoundException, which becomes an error status with the path.
                let data = if mode.starts_with('w') {
                    std::fs::write(host(&path), b"").map_err(|_| path.clone())?;
                    Vec::new()
                } else if mode.starts_with('a') {
                    std::fs::read(host(&path)).unwrap_or_default()
                } else {
                    std::fs::read(host(&path)).map_err(|_| path.clone())?
                };
                let pos = if mode.starts_with('a') { data.len() } else { 0 };
                let id = self.next_handle;
                self.next_handle += 1;
                self.handles.insert(id, Handle { path, data, pos, writable: !mode.starts_with('r') });
                vec![OcValue::Userdata(id)]
            }
            "read" => {
                let file = self.handles.get_mut(&handle()?).ok_or("bad file descriptor")?;
                let n = (int(args, 1).max(0) as usize).min(MAX_READ).min(file.data.len().saturating_sub(file.pos));
                if n == 0 {
                    vec![OcValue::Nil]
                } else {
                    file.pos += n;
                    vec![OcValue::Bytes(file.data[file.pos - n..file.pos].to_vec())]
                }
            }
            "write" => {
                let file = self.handles.get_mut(&handle()?).ok_or("bad file descriptor")?;
                if !file.writable {
                    return Err("file is not open for writing".into());
                }
                let bytes = text(args, 1);
                let end = file.pos + bytes.len();
                if file.data.len() < end {
                    file.data.resize(end, 0);
                }
                file.data[file.pos..end].copy_from_slice(&bytes);
                file.pos = end;
                std::fs::write(host(&file.path), &file.data).map_err(|e| e.to_string())?;
                vec![OcValue::Bool(true)]
            }
            "seek" => {
                let file = self.handles.get_mut(&handle()?).ok_or("bad file descriptor")?;
                let base = match String::from_utf8_lossy(&text(args, 1)).as_ref() {
                    "cur" => file.pos as i64,
                    "end" => file.data.len() as i64,
                    _ => 0,
                };
                file.pos = (base + int(args, 2)).max(0) as usize;
                vec![OcValue::Int(file.pos as i64)]
            }
            "close" => {
                self.handles.remove(&handle()?);
                Vec::new()
            }
            "list" => {
                let path = path_arg(0);
                let dir = std::fs::read_dir(host(&path)).map_err(|_| path)?;
                // Directories end in '/', as OpenComputers lists them.
                let mut names: Vec<String> = dir
                    .filter_map(|e| e.ok())
                    .map(|e| {
                        let slash = if e.path().is_dir() { "/" } else { "" };
                        format!("{}{slash}", e.file_name().to_string_lossy())
                    })
                    .collect();
                names.sort();
                vec![OcValue::Array(names.into_iter().map(OcValue::String).collect())]
            }
            other => return Err(format!("no such method: {other}")),
        })
    }

    /// Returns (status, payload) for a request, or `None` to leave it pending.
    fn serve(&mut self, request: &Request) -> Option<(i32, Vec<u8>)> {
        Some(match request {
            Request::Components { filter } => {
                let mut all = vec![(GPU, "gpu"), (SCREEN, "screen"), (KEYBOARD, "keyboard"), (EEPROM, "eeprom")];
                if self.disk.is_some() {
                    all.push((DISK, "filesystem"));
                }
                let map = all
                    .iter()
                    .filter(|(_, t)| t.contains(filter.as_str()))
                    .map(|(a, t)| (OcValue::str(*a), OcValue::str(*t)))
                    .collect();
                (STATUS_OK, OcValue::Map(map).encode())
            }
            Request::Invoke { address, method, args } => {
                let result = match address.as_str() {
                    GPU => self.gpu(method, args),
                    EEPROM => self.eeprom(method, args),
                    DISK if self.disk.is_some() => {
                        *self.fs_calls.entry(method.clone()).or_default() += 1;
                        self.filesystem(method, args)
                    }
                    _ => return Some((STATUS_NO_SUCH_COMPONENT, b"no such component".to_vec())),
                };
                match result {
                    Ok(results) => (STATUS_OK, OcValue::Array(results).encode()),
                    Err(e) => (STATUS_ERROR, e.into_bytes()),
                }
            }
            Request::Methods { .. } => (STATUS_NO_SUCH_COMPONENT, b"no such component".to_vec()),
            Request::PullSignal { .. } => {
                let c = self.keys.pop_front()?;
                let signal = vec![
                    OcValue::str("key_down"),
                    OcValue::str(KEYBOARD),
                    OcValue::Float(c as u32 as f64),
                    OcValue::Float(0.0),
                    OcValue::str("Developer"),
                ];
                (STATUS_OK, OcValue::Array(signal).encode())
            }
            Request::PushSignal { .. } => (STATUS_OK, Vec::new()),
            Request::DefaultDisk => {
                let disk = if self.disk.is_some() { vec![OcValue::str(DISK)] } else { Vec::new() };
                (STATUS_OK, OcValue::Array(disk).encode())
            }
            Request::Beep { .. } => {
                self.beeps += 1;
                (STATUS_OK, Vec::new())
            }
        })
    }
}

fn main() -> Result<()> {
    let (mut eeprom, mut disk, mut ram_kib, mut keys) = (None, None, 192u64, None);
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| anyhow!("{arg} needs a value"));
        match arg.as_str() {
            "--eeprom" => eeprom = Some(PathBuf::from(value()?)),
            "--disk" => disk = Some(PathBuf::from(value()?)),
            "--ram" => ram_kib = value()?.parse()?,
            "--keys" => keys = Some(value()?),
            other => bail!("unknown argument {other}; usage: headless --eeprom <bios.wasm> [--disk <dir>] [--ram <KiB>] [--keys <text>]"),
        }
    }
    let eeprom = eeprom.ok_or_else(|| anyhow!("--eeprom is required"))?;
    let keys = keys
        .unwrap_or_else(|| "help\\rinfo\\rcomponents\\rbeep\\rbogus\\r".into())
        .replace("\\r", "\r")
        .replace("\\b", "\u{8}")
        .replace("\\d", "\u{4}");

    let dir = std::env::temp_dir().join(format!("wackasm-oc-headless-{}", std::process::id()));
    let work = WorkDir::open(&dir)?;
    let new_machine = || -> Result<OcMachine> {
        let mut m = OcMachine::new(work.clone(), "headless", "computer-0000")?;
        m.set_memory_limit(ram_kib * 1024);
        Ok(m)
    };
    let mut machine = new_machine()?;
    let mut sim = Sim {
        grid: vec![vec![' '; W]; H],
        keys: keys.chars().collect(),
        beeps: 0,
        eeprom_code: std::fs::read(&eeprom)?,
        eeprom_data: Vec::new(),
        disk,
        handles: HashMap::new(),
        next_handle: 1,
        fs_calls: Default::default(),
    };
    machine.boot(Some(&sim.eeprom_code))?;

    let (mut slices, mut requests, mut reloads) = (0u64, 0u64, 0u64);
    let mut uptime = 0.0;
    let outcome = loop {
        uptime += 0.05;
        let outcome = machine.run(uptime, 200_000)?;
        slices += 1;
        if slices % 7 == 0 {
            machine.save()?;
            machine = new_machine()?;
            machine.load()?;
            reloads += 1;
        }
        match outcome {
            RunOutcome::Yield => continue,
            RunOutcome::Request(request) => match sim.serve(&request) {
                Some((status, payload)) => {
                    requests += 1;
                    machine.respond(status, payload)?;
                }
                None if matches!(request, Request::PullSignal { remaining: Some(_) }) => {
                    machine.respond(STATUS_TIMEOUT, Vec::new())?;
                }
                // Out of keys and waiting forever: done.
                None => break RunOutcome::Request(request),
            },
            done => break done,
        }
    };

    println!("+{}+", "-".repeat(W));
    for row in &sim.grid {
        println!("|{}|", row.iter().collect::<String>());
    }
    println!("+{}+", "-".repeat(W));
    println!("stopped with {outcome:?}");
    println!(
        "{slices} slices, {requests} requests served, {reloads} save/load cycles, {} beeps, {ram_kib} KiB RAM",
        sim.beeps
    );
    println!("eeprom data (boot address): {:?}", String::from_utf8_lossy(&sim.eeprom_data));
    let total: usize = sim.fs_calls.values().sum();
    println!("filesystem calls: {total} {:?}", sim.fs_calls);
    if let Ok(log) = std::fs::read_to_string(dir.join("logs/headless.log")) {
        print!("guest log:\n{log}");
    }
    let _ = std::fs::remove_dir_all(dir);
    Ok(())
}
