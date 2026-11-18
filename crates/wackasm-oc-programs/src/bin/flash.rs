//! `flash`: writes a WebAssembly BIOS to the EEPROM, like OpenOS's `flash`.
//!
//! * `flash FILE [LABEL]`: checks FILE, asks for confirmation, writes it to
//!   the EEPROM, and sets the label if one is given. The EEPROM's data (the
//!   remembered boot disk) is kept.
//! * `flash -r FILE`: saves the EEPROM's code to FILE, as a backup.
//! * `flash -i`: shows the EEPROM's label, size, capacity and checksum.
//! * `-y` skips the confirmation.
//!
//! An EEPROM holds 4096 bytes, so its code has to be a small `oc_main`
//! program (`wasm32-unknown-unknown`, `no_std`, like
//! `crates/wackasm-oc-bios`). Ordinary WASI programs are far too big; put
//! those in `/bin` on a disk instead. The shell floppy has the stock BIOS as
//! `/bios.wasm`, so `flash /bios.wasm` restores it.
//!
//! The code only runs on a WebAssembly CPU. Flashing an EEPROM in a Lua
//! computer works, but that computer won't boot from it.

use std::{
    io::{self, BufRead, Write},
    process::exit,
};

use wackasm_opencomputers::{Component, components::Eeprom};

fn usage() -> ! {
    eprintln!("usage: flash [-y] FILE [LABEL] | flash -r FILE | flash -i");
    exit(2);
}

fn fail(message: impl std::fmt::Display) -> ! {
    eprintln!("flash: {message}");
    exit(1);
}

fn main() {
    let (mut yes, mut read, mut info, mut rest) = (false, false, false, Vec::new());
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "-y" => yes = true,
            "-r" => read = true,
            "-i" => info = true,
            flag if flag.starts_with('-') => usage(),
            _ => rest.push(arg),
        }
    }
    let eeprom = Eeprom::first().unwrap_or_else(|| fail("no EEPROM installed"));
    let capacity = eeprom.get_size().map(|n| n.as_i64() as usize).unwrap_or(4096);

    if info {
        let code = eeprom.get().unwrap_or_else(|e| fail(e));
        let label = eeprom.get_label().map(|l| String::from_utf8_lossy(&l.0).into_owned()).unwrap_or_default();
        let checksum = eeprom.get_checksum().map(|c| String::from_utf8_lossy(&c.0).into_owned()).unwrap_or_default();
        println!("label:    {label}");
        println!("code:     {} of {capacity} bytes ({})", code.0.len(), describe(&code.0));
        println!("checksum: {checksum}");
        return;
    }

    if read {
        let [file] = rest.as_slice() else { usage() };
        let code = eeprom.get().unwrap_or_else(|e| fail(e));
        std::fs::write(file, &code.0).unwrap_or_else(|e| fail(format!("{file}: {e}")));
        println!("saved {} bytes of EEPROM code to {file}", code.0.len());
        return;
    }

    let (file, label) = match rest.as_slice() {
        [file] => (file, None),
        [file, label] => (file, Some(label)),
        _ => usage(),
    };
    // Check the size before reading: a typical WASI program is bigger than
    // the RAM this one has to spare.
    let size = std::fs::metadata(file).map(|m| m.len()).unwrap_or_else(|e| fail(format!("{file}: {e}")));
    if size > capacity as u64 {
        fail(format!("{file}: {}", too_big(size, capacity)));
    }
    let code = std::fs::read(file).unwrap_or_else(|e| fail(format!("{file}: {e}")));
    if let Err(problem) = check(&code, capacity) {
        fail(format!("{file}: {problem}"));
    }

    if !yes {
        print!("write {file} ({} bytes) to the EEPROM? [y/N] ", code.len());
        let _ = io::stdout().flush();
        let mut answer = String::new();
        let _ = io::stdin().lock().read_line(&mut answer);
        if !answer.trim().eq_ignore_ascii_case("y") {
            println!("cancelled");
            exit(1);
        }
    }
    eeprom.set(&code).unwrap_or_else(|e| fail(format!("writing the EEPROM: {e}")));
    if let Some(label) = label {
        eeprom.set_label(label.as_bytes()).unwrap_or_else(|e| fail(format!("setting the label: {e}")));
    }
    // Read it back: a read-only EEPROM can fail quietly on some versions.
    match eeprom.get() {
        Ok(written) if written.0 == code => println!("flashed {} bytes; reboot to run it", code.len()),
        Ok(_) => fail("the EEPROM kept its old code (is it read-only?)"),
        Err(e) => fail(format!("reading back: {e}")),
    }
}

/// What `code` looks like, for `-i`.
fn describe(code: &[u8]) -> String {
    if code.is_empty() {
        return "empty".into();
    }
    match exports(code) {
        Ok(exports) if exports.iter().any(|e| e == "oc_main") => "wasm, oc_main".into(),
        Ok(exports) if exports.iter().any(|e| e == "_start") => "wasm, _start".into(),
        Ok(_) => "wasm, no entry point".into(),
        Err(_) => "not wasm (a Lua BIOS?)".into(),
    }
}

fn too_big(size: u64, capacity: usize) -> String {
    format!(
        "{size} bytes is too big for the EEPROM ({capacity} bytes). EEPROM code must be a tiny `oc_main` \
         program; put larger programs in /bin on a disk"
    )
}

/// Refuses what can't work as EEPROM code.
fn check(code: &[u8], capacity: usize) -> Result<(), String> {
    if code.len() > capacity {
        return Err(too_big(code.len() as u64, capacity));
    }
    let exports = exports(code)?;
    if !exports.iter().any(|e| e == "oc_main" || e == "_start") {
        return Err("it exports neither `oc_main` nor `_start`, so there is nothing to run".into());
    }
    Ok(())
}

/// The names of a wasm module's exports, checking the header on the way.
fn exports(code: &[u8]) -> Result<Vec<String>, String> {
    if code.get(..8) != Some(b"\0asm\x01\0\0\0".as_slice()) {
        return Err("not a WebAssembly module".into());
    }
    let mut r = Reader { code, pos: 8 };
    let mut names = Vec::new();
    while r.pos < code.len() {
        let id = r.byte()?;
        let size = r.leb()? as usize;
        let end = r.pos.checked_add(size).filter(|&e| e <= code.len()).ok_or("truncated module")?;
        if id == 7 {
            for _ in 0..r.leb()? {
                let len = r.leb()? as usize;
                let name = code.get(r.pos..r.pos + len).ok_or("truncated module")?;
                names.push(String::from_utf8_lossy(name).into_owned());
                r.pos += len;
                r.byte()?; // kind
                r.leb()?; // index
            }
        }
        r.pos = end;
    }
    Ok(names)
}

struct Reader<'a> {
    code: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn byte(&mut self) -> Result<u8, String> {
        let b = *self.code.get(self.pos).ok_or("truncated module")?;
        self.pos += 1;
        Ok(b)
    }

    fn leb(&mut self) -> Result<u64, String> {
        let (mut value, mut shift) = (0u64, 0);
        loop {
            let b = self.byte()?;
            value |= u64::from(b & 0x7f) << shift;
            if b & 0x80 == 0 {
                return Ok(value);
            }
            shift += 7;
            if shift > 63 {
                return Err("malformed module".into());
            }
        }
    }
}
