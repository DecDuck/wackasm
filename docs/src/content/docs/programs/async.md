---
title: Async programs
description: Wait on several component calls, timers, signals and downloads at once with the async feature.
sidebar:
  order: 7
---

Every component call normally blocks: your program stops until the answer
comes back, and an indirect call (a file read, a robot move, an internet
read) takes a whole server tick. That's how Lua works too.

With the `async` feature, `wackasm-opencomputers` adds an executor and a
non-blocking version of every call. The call is queued, your program keeps
running, and the result arrives later. That lets one program wait on many
things at once: several downloads, a timer, the keyboard and a redstone
change.

```toml
[dependencies]
wackasm-opencomputers = { version = "0.1", features = ["async"] }
```

The feature is optional. Without it nothing changes, and with it the
blocking API, `std::fs`, `println!` and everything else still work as
before.

## A first async program

```rust
use std::time::Duration;
use wackasm_opencomputers::{prelude::*, task};

fn main() {
    task::run(async {
        // A background task: blink a cursor in the corner.
        task::spawn(async {
            let gpu = Gpu::first().expect("no GPU");
            let mut on = false;
            loop {
                on = !on;
                let _ = gpu.set_async(1, 1, if on { "_" } else { " " }, None).await;
                task::sleep(Duration::from_millis(500)).await;
            }
        });

        // Meanwhile, wait for a key.
        let key = task::signal(|s| matches!(s, Signal::KeyDown { .. })).await;
        println!("pressed {key:?}");
    });
}
```

`task::run` runs a future, and the tasks it spawns, until that future
finishes. Inside it:

| | does |
|---|---|
| `gpu.set_async(...)`, `robot.move_async(...)`, ... | every component method has an `_async` twin returning a future |
| `component::invoke_async(address, method, args)` | an untyped call, for components the wrappers don't know |
| `task::spawn(future)` | runs a task alongside this one; await the `JoinHandle` for its result |
| `task::join(a, b)`, `task::join_all(futures)` | waits for several futures at once |
| `task::sleep(duration)` | waits game time without holding up other tasks |
| `task::timeout(duration, future)` | gives up on a future after a while |
| `task::signal(filter)`, `task::next_signal()` | the next signal that matches; see below |
| `task::yield_now()` | lets other tasks run |

## Internet

The [internet API](/wackasm/programs/internet/) has async versions too:
`internet::get_async(url)`, `Request::send_async()`,
`Response::read_async(buf)` and `read_to_end_async(vec)`,
`TcpStream::connect_async`, `read_async`, `write_async` and
`write_all_async`. Since the card downloads in the background on the server,
several requests really are in flight at once:

```rust
let pages = ["https://example.com/a", "https://example.com/b", "https://example.com/c"];
let sizes = task::join_all(pages.iter().map(|url| async move {
    let mut response = internet::get_async(*url).await?;
    let mut body = Vec::new();
    response.read_to_end_async(&mut body).await?;
    Ok::<_, Box<dyn std::error::Error>>(body.len())
}))
.await;
```

The `fetch` program on the shell floppy does this: `fetch URL URL URL`.

## How it runs

- Calls run on the server **in the order they were made**, so three
  `gpu.set_async` calls draw in order. A blocking call made after async ones
  waits for them.
- Direct calls run as soon as the program next waits. Indirect calls run on
  the server tick, as many per tick as the server's
  [`asyncCallsPerTick`](/wackasm/reference/configuration/) allows. The default
  of 1 matches Lua; servers can raise it so programs overlap several indirect
  calls in one tick.
- A process can have **64** calls outstanding at once. Past that, a call
  fails with an error instead of queuing.
- Dropping a future cancels its call: if it hasn't run yet, it never will.
- Everything survives a world save, including calls still in the queue.

## Signals

Signals are one queue shared by the whole program, and taking a signal
removes it. Inside `task::run`, use `task::signal`: each signal goes to the
first task waiting for one that matches, and signals nobody is waiting for
are kept (up to 256) for whoever asks next. Don't call `signal::pull` inside
`task::run`, because it would take signals other tasks are waiting for.

## Blocking calls inside tasks

Blocking calls still work inside tasks: the blocking component methods,
`std::fs`, `println!`, reading stdin, and [running other
programs](/wackasm/programs/processes/). While one runs, no other task does,
so in code with other tasks to keep going, prefer the `_async` versions.

The full API is in the [reference for `task`](/wackasm/rustdoc/wackasm_opencomputers/task/index.html).

## The ABI underneath

The executor is built on five [`oc` imports](/wackasm/reference/abi/#async-calls):
`invoke_async`, `invoke_value_async`, `completion_next`, `cancel` and
`wait_ms`. Programs in other languages can use them directly.
