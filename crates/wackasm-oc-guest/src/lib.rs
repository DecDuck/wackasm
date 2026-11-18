//! The "wackasm shell" loot floppy's `/init.wasm`: a tiny shell on the first
//! GPU and screen that runs programs from `/bin`.
//!
//! Built-ins: `help`, `info`, `components`, `cd [dir]`, `pwd`, `ls [dir]`,
//! `beep`, `clear`, `reboot`, `shutdown`. Any other command `name args...`
//! runs `/bin/<name>.wasm` (or a path given directly, relative to the current
//! directory) as a child process, looking on the boot disk first and then on
//! the others.
//!
//! The current directory (`cd`, `pwd`, `ls`, and where programs start) is on
//! the disk programs see as `/`, which is not necessarily the boot disk: with
//! the read-only shell floppy booted, it is the first writable disk. Programs are ordinary WASI
//! preview 1 binaries (`--target wasm32-wasip1`): argv comes from the command
//! line, stdout/stderr go to the screen, and stdin reads lines typed here.
//! Ctrl+D at a program's input sends end of file.

#[path = "../../wackasm-oc/src/value.rs"]
#[allow(dead_code)]
mod value;

pub mod oc;

use oc::ProcEvent;

/// The environment programs start with.
const ENV: &[&str] = &["PATH=/bin", "SHELL=/init.wasm"];
use value::OcValue;

/// Cursor-based text output on a GPU, wrapping and scrolling like a terminal.
struct Terminal {
    gpu: String,
    width: i64,
    height: i64,
    /// 1-based cursor position.
    col: i64,
    row: i64,
}

impl Terminal {
    fn call(&self, method: &str, args: Vec<OcValue>) -> Vec<OcValue> {
        oc::invoke(&self.gpu, method, args).unwrap_or_else(|e| oc::crash(&format!("gpu.{method}: {e}")))
    }

    fn set(&self, x: i64, y: i64, text: &str) {
        self.call("set", vec![OcValue::Int(x), OcValue::Int(y), OcValue::str(text)]);
    }

    fn fill(&self, x: i64, y: i64, w: i64, h: i64) {
        self.call("fill", vec![OcValue::Int(x), OcValue::Int(y), OcValue::Int(w), OcValue::Int(h), OcValue::str(" ")]);
    }

    fn clear(&mut self) {
        self.fill(1, 1, self.width, self.height);
        self.col = 1;
        self.row = 1;
    }

    fn newline(&mut self) {
        self.col = 1;
        self.row += 1;
        if self.row > self.height {
            self.call(
                "copy",
                vec![OcValue::Int(1), OcValue::Int(2), OcValue::Int(self.width), OcValue::Int(self.height - 1), OcValue::Int(0), OcValue::Int(-1)],
            );
            self.fill(1, self.height, self.width, 1);
            self.row = self.height;
        }
    }

    fn flush_run(&self, run: &mut String, run_col: i64) {
        if !run.is_empty() {
            self.set(run_col, self.row, run);
            run.clear();
        }
    }

    /// Writes text at the cursor. One `gpu.set` per run of plain characters.
    fn write(&mut self, text: &str) {
        let mut run = String::new();
        let mut run_col = self.col;
        for ch in text.chars() {
            match ch {
                '\n' => {
                    self.flush_run(&mut run, run_col);
                    self.newline();
                    run_col = self.col;
                }
                '\r' => {
                    self.flush_run(&mut run, run_col);
                    self.col = 1;
                    run_col = 1;
                }
                '\t' => {
                    for _ in 0..4 - (self.col - 1) % 4 {
                        self.write_char(&mut run, &mut run_col, ' ');
                    }
                }
                c if c.is_control() => {}
                c => self.write_char(&mut run, &mut run_col, c),
            }
        }
        self.flush_run(&mut run, run_col);
    }

    fn write_char(&mut self, run: &mut String, run_col: &mut i64, c: char) {
        if self.col > self.width {
            self.flush_run(run, *run_col);
            self.newline();
            *run_col = self.col;
        }
        run.push(c);
        self.col += 1;
    }

    fn println(&mut self, text: &str) {
        self.write(text);
        self.write("\n");
    }

    /// Reads a line typed at the cursor, echoing it. `None` on Ctrl+D.
    fn read_line(&mut self) -> Option<String> {
        let start = self.col;
        let room = (self.width - start).max(0) as usize;
        let mut line = String::new();
        loop {
            // Redraw the input in place, ending in a cursor block.
            let len = line.chars().count();
            let shown: String = line.chars().skip(len.saturating_sub(room)).collect();
            let pad = room.saturating_sub(shown.chars().count());
            self.set(start, self.row, &format!("{shown}_{}", " ".repeat(pad)));

            let Some(signal) = oc::pull_signal(-1.0) else { continue };
            if signal.name != "key_down" {
                continue;
            }
            // key_down(keyboard, char, code, player)
            match signal.args.get(1).and_then(oc::number).unwrap_or(0.0) as u32 {
                13 => {
                    self.set(start, self.row, &format!("{shown} "));
                    self.newline();
                    return Some(line);
                }
                4 => {
                    self.set(start, self.row, &" ".repeat(room + 1));
                    return None;
                }
                8 => {
                    line.pop();
                }
                c @ 32..=126 => line.push(char::from_u32(c).unwrap()),
                _ => {}
            }
        }
    }
}

struct Shell {
    term: Terminal,
    /// Where programs are looked up, boot disk first.
    disks: Vec<String>,
    /// The current directory, always absolute and normalized.
    cwd: String,
}

/// `path` relative to `base`, with `.` and `..` resolved. `None` if it climbs
/// above `/`.
fn join_path(base: &str, path: &str) -> Option<String> {
    let mut parts: Vec<&str> = if path.starts_with('/') { Vec::new() } else { base.split('/').filter(|p| !p.is_empty()).collect() };
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            part => parts.push(part),
        }
    }
    Some(format!("/{}", parts.join("/")))
}

/// Splits a command line on whitespace, keeping "double quoted" words together.
fn split_args(line: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let (mut quoted, mut started) = (false, false);
    for c in line.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                started = true;
            }
            c if c.is_whitespace() && !quoted => {
                if started {
                    args.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            c => {
                current.push(c);
                started = true;
            }
        }
    }
    if started {
        args.push(current);
    }
    args
}

impl Shell {
    fn command(&mut self, line: &str) {
        let args = split_args(line);
        let Some(name) = args.first() else { return };
        match name.as_str() {
            "help" => {
                self.term.println("built-ins: help info components cd pwd ls beep");
                self.term.println("           clear reboot shutdown");
                self.term.println("anything else runs /bin/<name>.wasm");
            }
            "info" => {
                self.term.println(&format!("address: {}", oc::address()));
                self.term.println(&format!("memory:  {} KiB", oc::memory_limit() / 1024));
                self.term.println(&format!("uptime:  {:.2}s", oc::uptime()));
                self.term.println(&format!("disks:   {}", self.disks.join(", ")));
                self.term.println(&format!("files:   {}", oc::files_disk().unwrap_or_else(|| "none".into())));
            }
            "components" => {
                for (address, kind) in oc::components("") {
                    self.term.println(&format!("{kind:<12} {address}"));
                }
            }
            "pwd" => {
                let cwd = self.cwd.clone();
                self.term.println(&cwd);
            }
            "cd" => {
                let target = args.get(1).map_or("/", String::as_str);
                match (join_path(&self.cwd, target), oc::files_disk()) {
                    (None, _) => self.term.println(&format!("cd: {target}: outside the disk")),
                    (_, None) => self.term.println("cd: no disk"),
                    (Some(dir), Some(fs)) if oc::is_dir(&fs, &dir) => self.cwd = dir,
                    (Some(_), Some(_)) => self.term.println(&format!("cd: {target}: not a directory")),
                }
            }
            "ls" => {
                let target = args.get(1).map_or(".", String::as_str);
                let listing = match (join_path(&self.cwd, target), oc::files_disk()) {
                    (None, _) => Err("outside the disk".to_owned()),
                    (_, None) => Err("no disk".to_owned()),
                    (Some(dir), Some(fs)) => oc::list_dir(&fs, &dir),
                };
                match listing {
                    Ok(mut entries) => {
                        entries.sort();
                        self.term.println(&entries.join("  "));
                    }
                    Err(e) => self.term.println(&format!("ls: {target}: {e}")),
                }
            }
            "beep" => oc::beep(1000, 200),
            "clear" => self.term.clear(),
            "reboot" => oc::shutdown(true),
            "shutdown" => oc::shutdown(false),
            _ => self.run_program(&args),
        }
    }

    /// Starts the program a command names: the first candidate path, on the
    /// first disk, that exists. The host loads it, from its cache if it ran
    /// before.
    fn spawn(&self, args: &[String]) -> Option<Result<ProcEvent, String>> {
        let name = &args[0];
        let paths = if name.contains('/') {
            vec![name.to_owned()]
        } else {
            vec![format!("/bin/{name}.wasm"), format!("/bin/{name}")]
        };
        self.disks
            .iter()
            .find_map(|fs| paths.iter().find_map(|path| oc::spawn_file(fs, path, args, ENV, &self.cwd)))
    }

    fn run_program(&mut self, args: &[String]) {
        let Some(mut event) = self.spawn(args) else {
            self.term.println(&format!("{}: command not found", args[0]));
            return;
        };
        loop {
            match event {
                Err(e) => {
                    self.term.println(&format!("{}: {e}", args[0]));
                    return;
                }
                Ok(ProcEvent::Output(bytes)) => {
                    self.term.write(&String::from_utf8_lossy(&bytes));
                    event = oc::resume(&[]);
                }
                Ok(ProcEvent::Input) => {
                    let input = match self.term.read_line() {
                        Some(line) => line + "\n",
                        None => String::new(), // Ctrl+D: end of file
                    };
                    event = oc::resume(input.as_bytes());
                }
                Ok(ProcEvent::Exited { code, output }) => {
                    self.term.write(&String::from_utf8_lossy(&output));
                    self.end_line();
                    if code != 0 {
                        self.term.println(&format!("[exit {code}]"));
                    }
                    return;
                }
                Ok(ProcEvent::Crashed { message, output }) => {
                    self.term.write(&String::from_utf8_lossy(&output));
                    self.end_line();
                    self.term.println(&format!("[{} crashed: {message}]", args[0]));
                    return;
                }
            }
        }
    }

    /// Moves to a fresh line if the program left the cursor mid-line.
    fn end_line(&mut self) {
        if self.term.col > 1 {
            self.term.newline();
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn oc_main() {
    let (Some(gpu), Some(display)) = (oc::component("gpu"), oc::component("screen")) else {
        oc::crash("wackasm shell needs a GPU and a screen");
    };
    oc::invoke(&gpu, "bind", vec![OcValue::str(&display)]).unwrap_or_else(|e| oc::crash(&format!("gpu.bind: {e}")));

    let resolution = oc::invoke(&gpu, "getResolution", Vec::new()).unwrap_or_default();
    let dim = |i: usize, default: i64| resolution.get(i).and_then(oc::number).map_or(default, |n| n as i64);
    let term = Terminal { gpu, width: dim(0, 50), height: dim(1, 16), col: 1, row: 1 };

    let mut disks: Vec<String> = oc::boot_address().into_iter().collect();
    for (address, _) in oc::components("filesystem") {
        if !disks.contains(&address) {
            disks.push(address);
        }
    }

    let mut shell = Shell { term, disks, cwd: "/".into() };
    shell.term.clear();
    shell.term.println("wackasm shell - WebAssembly on OpenComputers");
    shell.term.println("type 'help' for commands, 'ls /bin' for programs");
    oc::log("shell started");

    loop {
        let prompt = format!("{}> ", shell.cwd);
        shell.term.write(&prompt);
        match shell.term.read_line() {
            Some(line) => shell.command(&line),
            None => shell.term.newline(),
        }
    }
}
