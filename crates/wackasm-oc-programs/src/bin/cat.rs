//! `cat FILE...`: prints files from the disk, via ordinary `std::fs`.

// Starts the program in the directory it was run from (see `opencomputers::process`).
use opencomputers as _;

use std::io::Write;

fn main() {
    let mut status = 0;
    for path in std::env::args().skip(1) {
        match std::fs::read(&path) {
            Ok(bytes) => std::io::stdout().write_all(&bytes).unwrap(),
            Err(e) => {
                eprintln!("cat: {path}: {e}");
                status = 1;
            }
        }
    }
    std::process::exit(status);
}
