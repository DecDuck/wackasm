//! `lines PROGRAM [ARGS...]`: runs a program, capturing its output, and
//! prints it with line numbers (like `PROGRAM | nl`).

use wackasm_opencomputers::process::Command;

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(program) = args.next() else {
        eprintln!("usage: lines PROGRAM [ARGS...]");
        std::process::exit(2);
    };
    match Command::new(&program).args(args).output() {
        Ok(output) => {
            let text = String::from_utf8_lossy(&output.stdout);
            for (n, line) in text.lines().enumerate() {
                println!("{:>4}  {line}", n + 1);
            }
            if !output.status.success() {
                println!("({program}: {})", output.status);
            }
        }
        Err(e) => {
            eprintln!("lines: {e}");
            std::process::exit(127);
        }
    }
}
