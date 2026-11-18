//! `write FILE [TEXT...]`: writes TEXT to FILE, or, without TEXT, the lines
//! typed until Ctrl+D.

// Starts the program in the directory it was run from (see `opencomputers::process`).
use opencomputers as _;

use std::io::Read;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(path) = args.get(1) else {
        eprintln!("usage: write FILE [TEXT...]");
        std::process::exit(2);
    };
    let contents = if args.len() > 2 {
        args[2..].join(" ") + "\n"
    } else {
        let mut typed = String::new();
        std::io::stdin().read_to_string(&mut typed).unwrap();
        typed
    };
    if let Err(e) = std::fs::write(path, &contents) {
        eprintln!("write: {path}: {e}");
        std::process::exit(1);
    }
    println!("wrote {} bytes to {path}", contents.len());
}
