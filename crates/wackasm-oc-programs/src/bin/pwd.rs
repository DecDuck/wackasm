//! `pwd`: prints the current directory, which starts as the one this program
//! was run from (see `wackasm_opencomputers::process`).

// Linking the crate is what moves the program into `PWD` before `main`.
use wackasm_opencomputers as _;

fn main() {
    match std::env::current_dir() {
        Ok(dir) => println!("{}", dir.display()),
        Err(e) => {
            eprintln!("pwd: {e}");
            std::process::exit(1);
        }
    }
}
