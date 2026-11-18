//! `time PROGRAM [ARGS...]`: runs a program with this terminal, then reports
//! how it exited and how long it took (in game time).

use wackasm_opencomputers::{computer, process::Command};

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(program) = args.next() else {
        eprintln!("usage: time PROGRAM [ARGS...]");
        std::process::exit(2);
    };
    let start = computer::uptime();
    match Command::new(&program).args(args).status() {
        Ok(status) => {
            let elapsed = computer::uptime() - start;
            eprintln!("{program}: {status} after {:.2}s", elapsed.as_secs_f64());
            std::process::exit(status.code().unwrap_or(1));
        }
        Err(e) => {
            eprintln!("time: {e}");
            std::process::exit(127);
        }
    }
}
