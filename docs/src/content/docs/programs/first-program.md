---
title: Your first program
description: Build a Rust program for a wackasm computer and run it from the shell.
sidebar:
  order: 1
---

Programs are ordinary **WASI preview 1** binaries. Anything that compiles to
`wasm32-wasip1` works; these pages use Rust.

## Set up

```sh
rustup target add wasm32-wasip1
cargo new --bin mytool
cd mytool
cargo add wackasm-opencomputers
```

[`wackasm-opencomputers`](https://crates.io/crates/wackasm-opencomputers) is
optional for plain programs, but it's what starts your program in the shell's
current directory, and it gives you components, signals, processes and the
internet card. Its [API reference](/wackasm/rustdoc/wackasm_opencomputers/) is built from the
latest sources; [docs.rs](https://docs.rs/wackasm-opencomputers) has the released versions.

## Write it

```rust
use std::io::{self, BufRead};
use wackasm_opencomputers::computer;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    println!("hello from wasm! args: {args:?}");
    println!("{} KiB of RAM free", computer::memory_limit() / 1024);

    print!("your name? ");
    let name = io::stdin().lock().lines().next().and_then(Result::ok).unwrap_or_default();
    println!("nice to meet you, {name}");
}
```

## Build it

Keep the stack small: rustc's default of 1 MiB is more RAM than most
computers have. In `.cargo/config.toml` in your project:

```toml
[build]
target = "wasm32-wasip1"

[target.wasm32-wasip1]
rustflags = ["-C", "link-arg=-zstack-size=65536"]
```

And in `Cargo.toml`, a release profile that keeps the binary small:

```toml
[profile.release]
opt-level = "s"
lto = true
codegen-units = 1
panic = "abort"
strip = true
```

```sh
cargo build --release
# target/wasm32-wasip1/release/mytool.wasm
```

## Run it

Get `mytool.wasm` into `/bin` on a disk in the computer:

- download it with an internet card: `cd /bin`, then
  `curl -O https://example.com/mytool.wasm`;
- or copy it into the disk's folder on the server,
  `<world>/opencomputers/<disk address>/bin/`.

Then type its name at the shell:

```
/> mytool a "b c"
hello from wasm! args: ["a", "b c"]
640 KiB of RAM free
your name? Ada
nice to meet you, Ada
```

## What works

- `println!`, `eprintln!` and reading stdin, through the shell;
- `std::env::args`, `std::env::vars` and `std::process::exit`;
- `std::fs`, on the [files disk](/wackasm/programs/files/);
- `std::time::SystemTime` and `Instant` (the computer's uptime), and random
  numbers (`getrandom`, so `rand` works);
- floats, integers, allocation and panics. Panics with `panic = "abort"`
  crash only your program, and the shell shows the message.

What doesn't is on the [Limits](/wackasm/programs/limits/) page.
