---
title: Components and signals
description: Talk to GPUs, robots, redstone and every other component with typed wrappers.
sidebar:
  order: 3
---

`wackasm-opencomputers` has a struct for every OpenComputers component type
(52 of them, 375 methods), generated from OpenComputers' own callbacks. Each
method keeps OpenComputers' documentation, its Lua signature, and whether it
is a direct call.

```rust
use std::time::Duration;
use wackasm_opencomputers::prelude::*;

fn main() -> Result<()> {
    let gpu = Gpu::first().ok_or(Error::NoSuchComponent)?;
    let (w, h) = gpu.get_resolution()?;
    gpu.fill(1, 1, w, h, " ")?;
    gpu.set(1, 1, "hello from Rust", None)?;

    // Wait up to 10 seconds for a key.
    if let Some(Signal::KeyDown { char, player, .. }) =
        signal::pull_filtered(Some(Duration::from_secs(10)), |s| matches!(s, Signal::KeyDown { .. }))
    {
        println!("{player} typed {:?}", char::from_u32(char));
    }

    let robot = Robot::first().ok_or(Error::NoSuchComponent)?;
    robot.r#move(Side::Front)?;
    Ok(())
}
```

With the crate's `async` feature, every method also has an `_async` twin
(`gpu.set_async(...)`) that doesn't block; see
[Async programs](/wackasm/programs/async/).

## Finding components

- `Gpu::first()`: the first one, by address;
- `Gpu::all()`: every one;
- `Gpu::at(address)`: a specific one.

For a component the wrappers don't know (another mod's), use
`component::Proxy` or `component::invoke(address, method, args)`, and
`component::list(filter)` / `component::methods(address)` to explore.

## Types

| Lua | parameter | optional parameter | return |
|---|---|---|---|
| `number` | `impl Into<Number>` (ints, `f64`, `Side`, `Color`) | `Option<Number>` | `Number` |
| `string` | `impl AsRef<[u8]>` (`&str`, bytes, `Address`) | `Option<&str>` | `OcString` |
| `boolean` | `bool` | `Option<bool>` | `bool` |
| `userdata` | `Handle` | `Option<Handle>` | `Handle` |
| `table`, unions | `impl Into<Value>` | `Option<Value>` | `Value` |
| `data...` | `&[Value]` | | |
| `X or nil` | | | `Option<X>` |
| `a, b` | | | `(A, B)` |

`Number` is an integer or a float, as OpenComputers sent it, so integers stay
exact. A reply of `nil, "reason"` becomes `Err(Error::Failed(reason))`.

## Signals

`signal::pull(timeout)` returns the next signal, typed: `KeyDown`, `KeyUp`,
`Clipboard`, `Touch`, `Drag`, `Drop`, `Scroll`, `Walk`, `ComponentAdded`,
`ComponentRemoved`, `ComponentAvailable`, `ComponentUnavailable`,
`RedstoneChanged`, `Motion`, `ModemMessage`, `InventoryChanged`, and `Other`
for anything else. `signal::push` queues one.

## This computer

`computer::address()`, `uptime()`, `memory_limit()`, `beep()`,
`shutdown()`, `reboot()`, `crash()`, `log()` (written to a log file on the
server), and `files_disk()`.

## Cost

Calls cost what they cost in Lua: direct calls draw on the per-tick call
budget, and other calls wait for the next server tick. See
[Hardware](/wackasm/reference/hardware/).
