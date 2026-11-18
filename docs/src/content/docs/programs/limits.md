---
title: Limits
description: What programs can't do yet.
sidebar:
  order: 7
---

## WebAssembly

- **No SIMD.** Don't enable `simd128`.
- No reference-type or table instructions (`table.get`, `ref.null`, ...) and
  no tail calls. Ordinary Rust code doesn't use these instructions. A
  program that executes one crashes with "unsupported operator".
- Every scalar integer and float instruction works.

## WASI

These return `ENOSYS`:

- `poll_oneoff`, so `std::thread::sleep` and anything else that waits on a
  timer. Use `wackasm_opencomputers::signal::pull(Some(duration))` to wait;
- sockets (use the [internet card](/wackasm/programs/internet/) instead);
- symlinks, and `fd_pread`/`fd_pwrite`.

There are no threads.

## Memory

All programs on a computer share its RAM, and every wasm page is 64 KiB. A
program that asks for more than is free gets an allocation failure. Keep the
stack small (`-zstack-size=65536`); see
[Your first program](/wackasm/programs/first-program/).

## Running

- One child process at a time per process.
- A program that loops forever can't be interrupted from the shell; turn the
  computer off and on.
- Each slice of execution is limited (`fuelPerSlice`), so long computations
  spread over several ticks. That's invisible to the program.
