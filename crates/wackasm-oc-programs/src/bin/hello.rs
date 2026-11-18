//! `hello [args...]`: shows argv, asks for a name on stdin, and exits with
//! the number of arguments, to show exit codes reaching the shell.

use std::io::{self, BufRead, Write};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    println!("hello from WASI! argv = {args:?}");

    print!("what's your name? ");
    io::stdout().flush().unwrap();
    let mut name = String::new();
    io::stdin().lock().read_line(&mut name).unwrap();
    let name = name.trim();
    if name.is_empty() {
        println!("no name? fine.");
    } else {
        println!("nice to meet you, {name}!");
    }

    std::process::exit(args.len() as i32 - 1);
}
