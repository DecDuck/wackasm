//! `env [NAME=VALUE...] [PROGRAM [ARGS...]]`: prints the environment, or runs
//! a program with those variables added.

use wackasm_opencomputers::process::Command;

fn main() {
    let mut args = std::env::args().skip(1).peekable();
    let mut vars = Vec::new();
    while let Some((name, value)) = args.peek().and_then(|a| a.split_once('=')).map(|(n, v)| (n.to_owned(), v.to_owned())) {
        vars.push((name, value));
        args.next();
    }
    match args.next() {
        Some(program) => match Command::new(&program).args(args).envs(vars).status() {
            Ok(status) => std::process::exit(status.code().unwrap_or(1)),
            Err(e) => {
                eprintln!("env: {e}");
                std::process::exit(127);
            }
        },
        None => {
            let mut all: Vec<(String, String)> = std::env::vars().collect();
            for (name, value) in vars {
                all.retain(|(n, _)| *n != name);
                all.push((name, value));
            }
            for (name, value) in all {
                println!("{name}={value}");
            }
        }
    }
}
