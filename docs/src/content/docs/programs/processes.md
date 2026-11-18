---
title: Running other programs
description: The process API, in the style of std::process.
sidebar:
  order: 4
---

WASI can't start processes, so `wackasm_opencomputers::process` provides an
API in the style of `std::process`:

```rust
use wackasm_opencomputers::process::Command;

// With this program's stdin and stdout.
let status = Command::new("hello").arg("world").status()?;
println!("hello exited with {status}");

// Captured.
let output = Command::new("cat").arg("notes.txt").output()?;
println!("{} bytes", output.stdout.len());

// Event by event.
let mut child = Command::new("hello").env("GREETING", "hi").spawn()?;
# Ok::<(), wackasm_opencomputers::Error>(())
```

## Command

- `arg`, `args`: arguments;
- `env`, `envs`, `env_remove`, `env_clear`: the environment, which children
  otherwise inherit;
- `current_dir`: where the child starts (default: this program's current
  directory);
- `disk`: load the program from this filesystem only;
- `Command::from_image(name, bytes)`: run a wasm binary held in memory.

Then `status()`, `output()` or `spawn()`.

A name is looked up in each directory of `PATH` (default `/bin`), as
`NAME.wasm` and then `NAME`. Disks are searched in order: this program's
own, then the boot disk, then the rest. A name containing `/` is a path,
relative to the child's directory. A missing program is
`Error::ProgramNotFound`.

## Child

- `next_event()` runs the child until it does something: `Event::Output(bytes)`,
  `Event::Input` (it wants stdin), or `Event::Exit(status)`;
- `write_stdin`, `close_stdin`: answer `Input`;
- `wait()`: relay the child's output and input through this program's own
  stdio until it exits. This works at any depth, which is how
  `time lines cat x.txt` reaches the screen;
- `wait_with_output()`: collect output until it exits;
- `kill()`. Dropping a running `Child` kills it.

`ExitStatus` is `Exited(code)` or `Crashed(message)`.

## Limits

- One child per process at a time. A pipeline has to run one stage after
  another, buffering output between them.
- stderr is merged into stdout.
- All processes share the computer's RAM.
