//! Runs a machine against fake OpenComputers components and prints the screen.
//!
//! ```text
//! cargo run -p wackasm-oc --example headless -- --eeprom <bios.wasm> [--disk <dir>] [--ram <KiB>] [--keys "help\r"] [--internet]
//! ```
//!
//! The simulated computer has:
//! * an EEPROM whose code is `--eeprom` (normally the wasm BIOS);
//! * a GPU bound to a 50x16 screen;
//! * a keyboard that types `--keys` (`\r` = enter, `\b` = backspace, `\d` = Ctrl+D);
//! * with `--disk`, a filesystem backed by that host directory, whose
//!   `open`/`read`/`close` hand out userdata handles like OpenComputers does;
//! * with `--internet`, an internet card making real connections from this
//!   host: `request` (plain `http://` only) and `connect`, whose userdata
//!   values answer `finishConnect`, `response`, `read`, `write` and `close`.
//!   Reads hand out at most 2 KiB, and the first read of a request finds
//!   nothing yet, as on a real server.
//!
//! RAM defaults to 192 KiB, one tier 1 stick. Every few slices the machine is
//! also saved and reloaded, which exercises suspension.

use std::{
    collections::{HashMap, VecDeque},
    io::{Read as _, Write as _},
    net::TcpStream,
    path::PathBuf,
};

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
const INTERNET: &str = "internet-0000";
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

/// A userdata value from the simulated internet card.
enum Net {
    Http {
        url: String,
        method: String,
        body: Vec<u8>,
        headers: Vec<(String, String)>,
        /// Status, message, headers and body, once fetched.
        response: Option<HttpReply>,
        pos: usize,
        polled: bool,
    },
    Tcp {
        address: String,
        stream: Option<TcpStream>,
        closed: bool,
    },
}

type HttpReply = (i64, String, Vec<(String, String)>, Vec<u8>);

/// A blocking HTTP/1.0 exchange.
fn http_fetch(url: &str, method: &str, body: &[u8], headers: &[(String, String)]) -> Result<HttpReply, String> {
    let rest = url.strip_prefix("http://").ok_or_else(|| format!("{url}: only http:// is simulated"))?;
    let (authority, path) = rest.split_once('/').map_or((rest, "/".to_owned()), |(a, p)| (a, format!("/{p}")));
    let host_port = if authority.contains(':') { authority.to_owned() } else { format!("{authority}:80") };
    let mut stream = TcpStream::connect(&host_port).map_err(|e| format!("{host_port}: {e}"))?;
    let mut head = format!("{method} {path} HTTP/1.0\r\nHost: {authority}\r\nUser-Agent: wackasm-headless\r\n");
    for (k, v) in headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    if !body.is_empty() {
        head.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).and_then(|_| stream.write_all(body)).map_err(|e| e.to_string())?;
    let mut reply = Vec::new();
    stream.read_to_end(&mut reply).map_err(|e| e.to_string())?;
    let split = reply.windows(4).position(|w| w == b"\r\n\r\n").ok_or("malformed response")?;
    let head = String::from_utf8_lossy(&reply[..split]).into_owned();
    let mut lines = head.split("\r\n");
    let status = lines.next().unwrap_or_default();
    let mut parts = status.splitn(3, ' ').skip(1);
    let code = parts.next().and_then(|c| c.parse().ok()).ok_or("malformed status line")?;
    let message = parts.next().unwrap_or_default().to_owned();
    let headers = lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned())).collect();
    Ok((code, message, headers, reply[split + 4..].to_vec()))
}

struct Sim {
    grid: Vec<Vec<char>>,
    keys: VecDeque<char>,
    beeps: usize,
    eeprom_code: Vec<u8>,
    eeprom_data: Vec<u8>,
    eeprom_label: String,
    disk: Option<PathBuf>,
    handles: HashMap<u32, Handle>,
    next_handle: u32,
    /// Filesystem calls by method, to see what programs cost.
    fs_calls: std::collections::BTreeMap<String, usize>,
    internet: bool,
    /// Userdata values of the internet card, by id (shared with `handles`).
    net: HashMap<u32, Net>,
    net_calls: std::collections::BTreeMap<String, usize>,
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
            "getSize" => vec![OcValue::Int(4096)],
            "getDataSize" => vec![OcValue::Int(256)],
            "set" => {
                let code = text(args, 0);
                if code.len() > 4096 {
                    return Err("not enough space".into());
                }
                self.eeprom_code = code;
                Vec::new()
            }
            "getLabel" => vec![OcValue::str(self.eeprom_label.as_str())],
            "setLabel" => {
                self.eeprom_label = String::from_utf8_lossy(&text(args, 0)).into_owned();
                vec![OcValue::str(self.eeprom_label.as_str())]
            }
            // Not OpenComputers' CRC32, just something that changes with the code.
            "getChecksum" => {
                let sum = self.eeprom_code.iter().fold(0u32, |h, &b| h.rotate_left(5) ^ u32::from(b));
                vec![OcValue::str(format!("{sum:08x}").as_str())]
            }
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

    fn internet(&mut self, method: &str, args: &[OcValue]) -> Result<Vec<OcValue>, String> {
        let string = |i: usize| String::from_utf8_lossy(&text(args, i)).into_owned();
        let value = match method {
            "isHttpEnabled" | "isTcpEnabled" => return Ok(vec![OcValue::Bool(true)]),
            "request" => {
                let body = text(args, 1);
                let headers = match args.get(2) {
                    Some(OcValue::Map(entries)) => entries
                        .iter()
                        .filter_map(|(k, v)| Some((k.as_str()?.to_owned(), v.as_str()?.to_owned())))
                        .collect(),
                    _ => Vec::new(),
                };
                let method = match args.get(3) {
                    Some(OcValue::String(m)) => m.clone(),
                    _ if !body.is_empty() => "POST".into(),
                    _ => "GET".into(),
                };
                Net::Http { url: string(0), method, body, headers, response: None, pos: 0, polled: false }
            }
            "connect" => {
                let address = match args.get(1) {
                    Some(port) if !matches!(port, OcValue::Nil) => format!("{}:{}", string(0), int(args, 1)),
                    _ => string(0),
                };
                Net::Tcp { address, stream: None, closed: false }
            }
            _ => return Err(format!("no such method: {method}")),
        };
        let id = self.next_handle;
        self.next_handle += 1;
        self.net.insert(id, value);
        Ok(vec![OcValue::Userdata(id)])
    }

    fn net_value(&mut self, id: u32, method: &str, args: &[OcValue]) -> Result<Vec<OcValue>, String> {
        *self.net_calls.entry(method.to_owned()).or_default() += 1;
        let net = self.net.get_mut(&id).ok_or("no such value")?;
        let max = match args.first() {
            Some(OcValue::Int(n)) => (*n).clamp(0, MAX_READ as i64) as usize,
            Some(OcValue::Float(n)) => n.clamp(0.0, MAX_READ as f64) as usize,
            _ => MAX_READ,
        };
        match net {
            Net::Http { url, method: verb, body, headers, response, pos, polled } => {
                if response.is_none() && matches!(method, "finishConnect" | "read" | "response") {
                    *response = Some(http_fetch(url, verb, body, headers)?);
                }
                let Some((code, message, headers, data)) = response else { return Ok(Vec::new()) };
                Ok(match method {
                    "finishConnect" => vec![OcValue::Bool(true)],
                    "response" => {
                        // OpenComputers gives each header as a list of values.
                        let headers = headers.iter().map(|(k, v)| (OcValue::str(k), OcValue::Array(vec![OcValue::str(v)]))).collect();
                        vec![OcValue::Int(*code), OcValue::str(message.as_str()), OcValue::Map(headers)]
                    }
                    "read" if !*polled => {
                        *polled = true;
                        vec![OcValue::Bytes(Vec::new())]
                    }
                    "read" if *pos >= data.len() => vec![OcValue::Nil],
                    "read" => {
                        let end = (*pos + max).min(data.len());
                        let chunk = data[*pos..end].to_vec();
                        *pos = end;
                        vec![OcValue::Bytes(chunk)]
                    }
                    "close" => Vec::new(),
                    _ => return Err(format!("no such method: {method}")),
                })
            }
            Net::Tcp { address, stream, closed } => {
                if *closed {
                    return Ok(vec![OcValue::Nil, OcValue::str("connection lost")]);
                }
                if stream.is_none() && method != "close" {
                    let s = TcpStream::connect(address.as_str()).map_err(|e| format!("{address}: {e}"))?;
                    s.set_nonblocking(true).map_err(|e| e.to_string())?;
                    *stream = Some(s);
                }
                Ok(match method {
                    "finishConnect" => vec![OcValue::Bool(true)],
                    "read" => {
                        let mut buf = vec![0u8; max.max(1)];
                        match stream.as_mut().unwrap().read(&mut buf) {
                            Ok(0) => vec![OcValue::Nil],
                            Ok(n) => vec![OcValue::Bytes(buf[..n].to_vec())],
                            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                                // Give the other side a moment, as a tick would.
                                std::thread::sleep(std::time::Duration::from_millis(50));
                                vec![OcValue::Bytes(Vec::new())]
                            }
                            Err(e) => return Err(e.to_string()),
                        }
                    }
                    "write" => {
                        let data = text(args, 0);
                        let s = stream.as_mut().unwrap();
                        s.set_nonblocking(false).map_err(|e| e.to_string())?;
                        s.write_all(&data).map_err(|e| e.to_string())?;
                        s.set_nonblocking(true).map_err(|e| e.to_string())?;
                        vec![OcValue::Int(data.len() as i64)]
                    }
                    "close" => {
                        *closed = true;
                        *stream = None;
                        Vec::new()
                    }
                    _ => return Err(format!("no such method: {method}")),
                })
            }
        }
    }

    /// Returns (status, payload) for a request, or `None` to leave it pending.
    fn serve(&mut self, request: &Request) -> Option<(i32, Vec<u8>)> {
        Some(match request {
            Request::Components { filter } => {
                let mut all = vec![(GPU, "gpu"), (SCREEN, "screen"), (KEYBOARD, "keyboard"), (EEPROM, "eeprom")];
                if self.disk.is_some() {
                    all.push((DISK, "filesystem"));
                }
                if self.internet {
                    all.push((INTERNET, "internet"));
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
                    INTERNET if self.internet => self.internet(method, args),
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
            Request::InvokeValue { value, method, args } => {
                if !self.net.contains_key(value) {
                    return Some((STATUS_NO_SUCH_COMPONENT, b"no such value".to_vec()));
                }
                match self.net_value(*value, method, args) {
                    Ok(results) => (STATUS_OK, OcValue::Array(results).encode()),
                    Err(e) => (STATUS_ERROR, e.into_bytes()),
                }
            }
            Request::ReleaseValue { value } => {
                self.net.remove(value);
                self.handles.remove(value);
                (STATUS_OK, Vec::new())
            }
        })
    }
}

fn main() -> Result<()> {
    let (mut eeprom, mut disk, mut ram_kib, mut keys, mut internet) = (None, None, 192u64, None, false);
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| anyhow!("{arg} needs a value"));
        match arg.as_str() {
            "--eeprom" => eeprom = Some(PathBuf::from(value()?)),
            "--disk" => disk = Some(PathBuf::from(value()?)),
            "--ram" => ram_kib = value()?.parse()?,
            "--keys" => keys = Some(value()?),
            "--internet" => internet = true,
            other => bail!("unknown argument {other}; usage: headless --eeprom <bios.wasm> [--disk <dir>] [--ram <KiB>] [--keys <text>] [--internet]"),
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
        eeprom_label: "EEPROM (WebAssembly BIOS)".into(),
        disk,
        handles: HashMap::new(),
        next_handle: 1,
        fs_calls: Default::default(),
        internet,
        net: HashMap::new(),
        net_calls: Default::default(),
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
    println!("eeprom: {:?}, {} bytes of code", sim.eeprom_label, sim.eeprom_code.len());
    let total: usize = sim.fs_calls.values().sum();
    println!("filesystem calls: {total} {:?}", sim.fs_calls);
    if sim.internet {
        println!("internet value calls: {:?}, values left unreleased: {}", sim.net_calls, sim.net.len());
    }
    if let Ok(log) = std::fs::read_to_string(dir.join("logs/headless.log")) {
        print!("guest log:\n{log}");
    }
    let _ = std::fs::remove_dir_all(dir);
    Ok(())
}
