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
    AsyncTarget, OcMachine, OcValue, Request, RunOutcome, WorkDir,
    abi::{STATUS_ERROR, STATUS_NO_SUCH_COMPONENT, STATUS_OK, STATUS_TIMEOUT},
};

const GPU: &str = "gpu-0000";
const SCREEN: &str = "screen-0000";
const KEYBOARD: &str = "keyboard-0000";
const EEPROM: &str = "eeprom-0000";
const DISK: &str = "disk-0000";
const INTERNET: &str = "internet-0000";
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

/// One character cell: the character and its colours (24-bit RGB).
#[derive(Clone, Copy, PartialEq)]
struct Cell {
    ch: char,
    fg: u32,
    bg: u32,
}

const BLANK: Cell = Cell { ch: ' ', fg: 0xFFFFFF, bg: 0x000000 };

/// A screen-sized or allocated GPU buffer.
#[derive(Clone)]
struct Buffer {
    w: usize,
    h: usize,
    cells: Vec<Cell>,
}

impl Buffer {
    fn new(w: usize, h: usize) -> Buffer {
        Buffer { w, h, cells: vec![BLANK; w * h] }
    }

    fn get(&self, x: i64, y: i64) -> Option<Cell> {
        ((1..=self.w as i64).contains(&x) && (1..=self.h as i64).contains(&y))
            .then(|| self.cells[(y as usize - 1) * self.w + x as usize - 1])
    }

    fn put(&mut self, x: i64, y: i64, cell: Cell) {
        if (1..=self.w as i64).contains(&x) && (1..=self.h as i64).contains(&y) {
            self.cells[(y as usize - 1) * self.w + x as usize - 1] = cell;
        }
    }
}

/// A tier 3 GPU: 24-bit colours kept exactly (a real GPU maps them to its
/// palette), a 16-colour palette, and video buffers. Buffer 0 is the screen.
struct GpuSim {
    buffers: std::collections::BTreeMap<i64, Buffer>,
    active: i64,
    next_buffer: i64,
    fg: u32,
    bg: u32,
    depth: i64,
    palette: [u32; 16],
    /// Video memory, in cells, for allocated buffers.
    memory: usize,
}

fn num(args: &[OcValue], i: usize, default: i64) -> i64 {
    match args.get(i) {
        Some(OcValue::Int(n)) => *n,
        Some(OcValue::Float(f)) => *f as i64,
        _ => default,
    }
}

impl GpuSim {
    fn new(w: usize, h: usize) -> GpuSim {
        let mut buffers = std::collections::BTreeMap::new();
        buffers.insert(0, Buffer::new(w, h));
        // OpenComputers' tier 3 default palette: 16 greys.
        let palette = std::array::from_fn(|i| {
            let v = (i as u32 + 1) * 0x0F;
            (v << 16) | (v << 8) | v
        });
        GpuSim { buffers, active: 0, next_buffer: 1, fg: 0xFFFFFF, bg: 0x000000, depth: 8, palette, memory: 3 * w * h }
    }

    fn screen_size(&self) -> (usize, usize) {
        let s = &self.buffers[&0];
        (s.w, s.h)
    }

    fn used_memory(&self) -> usize {
        self.buffers.iter().filter(|(id, _)| **id != 0).map(|(_, b)| b.w * b.h).sum()
    }

    fn active(&mut self) -> &mut Buffer {
        self.buffers.get_mut(&self.active).expect("active buffer exists")
    }

    fn call(&mut self, method: &str, args: &[OcValue]) -> Result<Vec<OcValue>, String> {
        let (sw, sh) = self.screen_size();
        let int = |v: i64| OcValue::Int(v);
        Ok(match method {
            "bind" => vec![OcValue::Bool(true)],
            "getScreen" => vec![OcValue::str(SCREEN)],
            "getResolution" | "maxResolution" | "getViewport" => vec![int(sw as i64), int(sh as i64)],
            "setResolution" | "setViewport" => vec![OcValue::Bool(num(args, 0, 0) as usize == sw && num(args, 1, 0) as usize == sh)],
            "getDepth" | "maxDepth" => vec![int(self.depth)],
            "setDepth" => {
                let old = self.depth;
                self.depth = num(args, 0, 8);
                vec![int(old)]
            }
            "getForeground" => vec![int(self.fg as i64), OcValue::Bool(false)],
            "getBackground" => vec![int(self.bg as i64), OcValue::Bool(false)],
            "setForeground" | "setBackground" => {
                let value = num(args, 0, 0);
                let palette = matches!(args.get(1), Some(OcValue::Bool(true)));
                let colour = if palette {
                    *self.palette.get(value as usize).ok_or("invalid palette index")?
                } else {
                    (value as u32) & 0xFFFFFF
                };
                let slot = if method == "setForeground" { &mut self.fg } else { &mut self.bg };
                let old = std::mem::replace(slot, colour);
                vec![int(old as i64), OcValue::Nil]
            }
            "getPaletteColor" => vec![int(*self.palette.get(num(args, 0, 0) as usize).ok_or("invalid palette index")? as i64)],
            "setPaletteColor" => {
                let i = num(args, 0, 0) as usize;
                let slot = self.palette.get_mut(i).ok_or("invalid palette index")?;
                let old = std::mem::replace(slot, num(args, 1, 0) as u32 & 0xFFFFFF);
                vec![int(old as i64)]
            }
            "set" => {
                let (x, y) = (num(args, 0, 1), num(args, 1, 1));
                let vertical = matches!(args.get(3), Some(OcValue::Bool(true)));
                let (fg, bg) = (self.fg, self.bg);
                let text = String::from_utf8_lossy(&text(args, 2)).into_owned();
                let buffer = self.active();
                for (i, ch) in text.chars().enumerate() {
                    let (cx, cy) = if vertical { (x, y + i as i64) } else { (x + i as i64, y) };
                    buffer.put(cx, cy, Cell { ch, fg, bg });
                }
                vec![OcValue::Bool(true)]
            }
            "get" => {
                let cell = self.buffers[&self.active].get(num(args, 0, 1), num(args, 1, 1)).ok_or("index out of bounds")?;
                vec![OcValue::str(cell.ch.to_string().as_str()), int(cell.fg as i64), int(cell.bg as i64), OcValue::Nil, OcValue::Nil]
            }
            "fill" => {
                let (x, y, w, h) = (num(args, 0, 1), num(args, 1, 1), num(args, 2, 0), num(args, 3, 0));
                let ch = String::from_utf8_lossy(&text(args, 4)).chars().next().unwrap_or(' ');
                let cell = Cell { ch, fg: self.fg, bg: self.bg };
                let buffer = self.active();
                for yy in y..y + h {
                    for xx in x..x + w {
                        buffer.put(xx, yy, cell);
                    }
                }
                vec![OcValue::Bool(true)]
            }
            "copy" => {
                let (x, y, w, h, tx, ty) =
                    (num(args, 0, 1), num(args, 1, 1), num(args, 2, 0), num(args, 3, 0), num(args, 4, 0), num(args, 5, 0));
                let snapshot = self.buffers[&self.active].clone();
                let buffer = self.active();
                for yy in y..y + h {
                    for xx in x..x + w {
                        if let Some(cell) = snapshot.get(xx, yy) {
                            buffer.put(xx + tx, yy + ty, cell);
                        }
                    }
                }
                vec![OcValue::Bool(true)]
            }
            "allocateBuffer" => {
                let (w, h) = (num(args, 0, sw as i64).max(1) as usize, num(args, 1, sh as i64).max(1) as usize);
                if self.used_memory() + w * h > self.memory {
                    return Ok(vec![OcValue::Nil, OcValue::str("not enough video memory")]);
                }
                let id = self.next_buffer;
                self.next_buffer += 1;
                self.buffers.insert(id, Buffer::new(w, h));
                vec![int(id)]
            }
            "freeBuffer" => {
                let id = num(args, 0, self.active);
                let freed = id != 0 && self.buffers.remove(&id).is_some();
                if freed && self.active == id {
                    self.active = 0;
                }
                vec![OcValue::Bool(freed)]
            }
            "freeAllBuffers" => {
                let count = self.buffers.len() - 1;
                self.buffers.retain(|id, _| *id == 0);
                self.active = 0;
                vec![int(count as i64)]
            }
            "buffers" => vec![OcValue::Array(self.buffers.keys().filter(|id| **id != 0).map(|id| int(*id)).collect())],
            "getActiveBuffer" => vec![int(self.active)],
            "setActiveBuffer" => {
                let id = num(args, 0, 0);
                if !self.buffers.contains_key(&id) {
                    return Ok(vec![OcValue::Nil, OcValue::str("invalid buffer index")]);
                }
                let old = std::mem::replace(&mut self.active, id);
                vec![int(old)]
            }
            "getBufferSize" => {
                let b = self.buffers.get(&num(args, 0, self.active)).ok_or("invalid buffer index")?;
                vec![int(b.w as i64), int(b.h as i64)]
            }
            "totalMemory" => vec![int(self.memory as i64)],
            "freeMemory" => vec![int((self.memory - self.used_memory()) as i64)],
            // bitblt(dst=0, col=1, row=1, width, height, src=active, fromCol=1, fromRow=1)
            "bitblt" => {
                let dst = num(args, 0, 0);
                let src = num(args, 5, self.active);
                let source = self.buffers.get(&src).ok_or("invalid source buffer")?.clone();
                let (col, row) = (num(args, 1, 1), num(args, 2, 1));
                let (w, h) = (num(args, 3, source.w as i64), num(args, 4, source.h as i64));
                let (from_col, from_row) = (num(args, 6, 1), num(args, 7, 1));
                let target = self.buffers.get_mut(&dst).ok_or("invalid destination buffer")?;
                for dy in 0..h {
                    for dx in 0..w {
                        if let Some(cell) = source.get(from_col + dx, from_row + dy) {
                            target.put(col + dx, row + dy, cell);
                        }
                    }
                }
                vec![OcValue::Bool(true)]
            }
            other => return Err(format!("no such method: {other}")),
        })
    }

    /// The screen as a PNG, 8x16 pixels per cell. Block elements, shades and
    /// braille are drawn as their shapes; other characters as a bar in the
    /// foreground colour (the text printout shows what they say).
    fn png(&self) -> Vec<u8> {
        const CW: usize = 8;
        const CH: usize = 16;
        let screen = &self.buffers[&0];
        let (width, height) = (screen.w * CW, screen.h * CH);
        let mut rgb = vec![0u8; width * height * 3];
        for (i, cell) in screen.cells.iter().enumerate() {
            let (cx, cy) = (i % screen.w, i / screen.w);
            for py in 0..CH {
                for px in 0..CW {
                    let on = glyph_pixel(cell.ch, px, py, CW, CH);
                    let c = if on { cell.fg } else { cell.bg };
                    let at = ((cy * CH + py) * width + cx * CW + px) * 3;
                    rgb[at..at + 3].copy_from_slice(&[(c >> 16) as u8, (c >> 8) as u8, c as u8]);
                }
            }
        }
        encode_png(width, height, &rgb)
    }
}

/// Whether pixel (px, py) of a cw x ch cell is in `ch`'s foreground.
fn glyph_pixel(ch: char, px: usize, py: usize, cw: usize, chh: usize) -> bool {
    let (fx, fy) = ((px as f64 + 0.5) / cw as f64, (py as f64 + 0.5) / chh as f64);
    let (left, top) = (fx < 0.5, fy < 0.5);
    let code = ch as u32;
    match code {
        0x20 => false,
        0x2580 => top,
        0x2581..=0x2587 => fy >= 1.0 - (code - 0x2580) as f64 / 8.0,
        0x2588 => true,
        0x2589..=0x258F => fx < (0x2590 - code) as f64 / 8.0,
        0x2590 => !left,
        0x2591 => px % 2 == 0 && py % 2 == 0,
        0x2592 => (px + py) % 2 == 0,
        0x2593 => !(px % 2 == 1 && py % 2 == 1),
        0x2594 => fy < 1.0 / 8.0,
        0x2595 => fx >= 7.0 / 8.0,
        0x2596..=0x259F => {
            // Quadrants: upper-left, upper-right, lower-left, lower-right.
            let quads: [bool; 4] = match code {
                0x2596 => [false, false, true, false],
                0x2597 => [false, false, false, true],
                0x2598 => [true, false, false, false],
                0x2599 => [true, false, true, true],
                0x259A => [true, false, false, true],
                0x259B => [true, true, true, false],
                0x259C => [true, true, false, true],
                0x259D => [false, true, false, false],
                0x259E => [false, true, true, false],
                _ => [false, true, true, true],
            };
            quads[match (top, left) {
                (true, true) => 0,
                (true, false) => 1,
                (false, true) => 2,
                (false, false) => 3,
            }]
        }
        0x2800..=0x28FF => {
            // Dots on a 2x4 grid, each a square in the middle of its sub-cell.
            let bits = code - 0x2800;
            const BITS: [[u32; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];
            let (col, row) = ((fx * 2.0) as usize, (fy * 4.0) as usize);
            let (sx, sy) = (fx * 2.0 - col as f64, fy * 4.0 - row as f64);
            bits & BITS[col.min(1)][row.min(3)] != 0 && (0.2..0.8).contains(&sx) && (0.2..0.8).contains(&sy)
        }
        _ => (0.15..0.85).contains(&fx) && (0.3..0.75).contains(&fy),
    }
}

/// An RGB PNG with stored (uncompressed) deflate blocks.
fn encode_png(width: usize, height: usize, rgb: &[u8]) -> Vec<u8> {
    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = !0u32;
        for &b in bytes {
            crc ^= b as u32;
            for _ in 0..8 {
                crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
            }
        }
        !crc
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc32(&body).to_be_bytes());
    }
    let mut raw = Vec::with_capacity((width * 3 + 1) * height);
    for row in rgb.chunks(width * 3) {
        raw.push(0); // no filter
        raw.extend_from_slice(row);
    }
    let mut zlib = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65_535).collect();
    for (i, block) in blocks.iter().enumerate() {
        zlib.push((i + 1 == blocks.len()) as u8);
        let len = block.len() as u16;
        zlib.extend_from_slice(&len.to_le_bytes());
        zlib.extend_from_slice(&(!len).to_le_bytes());
        zlib.extend_from_slice(block);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in &raw {
        a = (a + byte as u32) % 65_521;
        b = (b + a) % 65_521;
    }
    zlib.extend_from_slice(&((b << 16) | a).to_be_bytes());

    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(width as u32).to_be_bytes());
    ihdr.extend_from_slice(&(height as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit RGB
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &zlib);
    chunk(&mut out, b"IEND", &[]);
    out
}

struct Sim {
    gpu: GpuSim,
    gpu_calls: std::collections::BTreeMap<String, usize>,
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
    /// Calls that came through the async queue.
    async_calls: usize,
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
    fn gpu(&mut self, method: &str, args: &[OcValue]) -> Result<Vec<OcValue>, String> {
        *self.gpu_calls.entry(method.to_owned()).or_default() += 1;
        self.gpu.call(method, args)
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
            // Served like the blocking calls; the machine routes the answer.
            Request::InvokeAsync { target, method, args, .. } => {
                self.async_calls += 1;
                let inner = match target {
                    AsyncTarget::Component(address) => {
                        Request::Invoke { address: address.clone(), method: method.clone(), args: args.clone() }
                    }
                    AsyncTarget::Value(value) => {
                        Request::InvokeValue { value: *value, method: method.clone(), args: args.clone() }
                    }
                };
                return self.serve(&inner);
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
    let (mut size, mut png) = ((50usize, 16usize), None::<PathBuf>);
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| anyhow!("{arg} needs a value"));
        match arg.as_str() {
            "--eeprom" => eeprom = Some(PathBuf::from(value()?)),
            "--disk" => disk = Some(PathBuf::from(value()?)),
            "--ram" => ram_kib = value()?.parse()?,
            "--keys" => keys = Some(value()?),
            "--internet" => internet = true,
            "--size" => {
                let v = value()?;
                let (w, h) = v.split_once('x').ok_or_else(|| anyhow!("--size takes WxH, like 80x25"))?;
                size = (w.parse()?, h.parse()?);
            }
            "--png" => png = Some(PathBuf::from(value()?)),
            other => bail!("unknown argument {other}; usage: headless --eeprom <bios.wasm> [--disk <dir>] [--ram <KiB>] [--keys <text>] [--internet] [--size WxH] [--png <file>]"),
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
        gpu: GpuSim::new(size.0, size.1),
        gpu_calls: Default::default(),
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
        async_calls: 0,
    };
    machine.boot(Some(&sim.eeprom_code))?;

    let (mut slices, mut requests, mut reloads) = (0u64, 0u64, 0u64);
    let mut uptime = 0.0;
    // Game time moves a tick (50 ms) when the machine would wait for one: at
    // the end of a slice, or while waiting for a signal. Component calls are
    // served within the tick, as direct calls are in OpenComputers.
    let mut next_tick = true;
    let outcome = loop {
        if std::mem::take(&mut next_tick) {
            uptime += 0.05;
        }
        let outcome = machine.run(uptime, 200_000)?;
        next_tick = matches!(outcome, RunOutcome::Yield | RunOutcome::Request(Request::PullSignal { .. }));
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

    let screen = &sim.gpu.buffers[&0];
    println!("+{}+", "-".repeat(screen.w));
    for row in screen.cells.chunks(screen.w).map(|r| r.iter().map(|c| c.ch).collect::<Vec<char>>()) {
        println!("|{}|", row.iter().collect::<String>());
    }
    println!("+{}+", "-".repeat(screen.w));
    println!("stopped with {outcome:?}");
    println!(
        "{slices} slices, {requests} requests served, {reloads} save/load cycles, {} beeps, {ram_kib} KiB RAM",
        sim.beeps
    );
    println!("eeprom data (boot address): {:?}", String::from_utf8_lossy(&sim.eeprom_data));
    println!("eeprom: {:?}, {} bytes of code", sim.eeprom_label, sim.eeprom_code.len());
    let total: usize = sim.fs_calls.values().sum();
    println!("filesystem calls: {total} {:?}", sim.fs_calls);
    if !sim.gpu_calls.is_empty() {
        let total: usize = sim.gpu_calls.values().sum();
        println!("gpu calls: {total} {:?}", sim.gpu_calls);
    }
    if let Some(path) = &png {
        std::fs::write(path, sim.gpu.png())?;
        println!("screenshot: {}", path.display());
    }
    if sim.async_calls > 0 {
        println!("async calls served: {}", sim.async_calls);
    }
    if sim.internet {
        println!("internet value calls: {:?}, values left unreleased: {}", sim.net_calls, sim.net.len());
    }
    if let Ok(log) = std::fs::read_to_string(dir.join("logs/headless.log")) {
        print!("guest log:\n{log}");
    }
    let _ = std::fs::remove_dir_all(dir);
    Ok(())
}
