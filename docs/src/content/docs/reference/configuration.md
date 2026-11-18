---
title: Configuration
description: The mod's config file, JVM options, and the files it writes.
sidebar:
  order: 2
---

## config/wackasm.cfg

Created on first start.

| setting | default | meaning |
|---|---|---|
| `callBudgetMultiplier` | 1.0 | how much further the per-tick direct-call budget goes for wasm computers. OpenComputers charges each direct call `1/limit` of the budget (`filesystem.read` has limit 15); wasm computers pay that divided by this, so 2.0 gives them twice the direct calls per tick of a Lua computer with the same parts. Lua computers are unaffected. Range 0.1 to 16 |
| `memoryMultiplier` | 1.8 | memory for wasm computers, as a multiple of installed RAM. 1.8 is what OpenComputers gives Lua on a 64-bit server. Lua computers are unaffected. Takes effect when a computer's parts change or it restarts. Range 1.0 to 4.0 |
| `asyncCallsPerTick` | 1 | indirect component calls a wasm computer's [async queue](/wackasm/programs/async/) may run per tick. Blocking calls, and Lua computers, always get one per tick. Range 1 to 64 |
| `fuelPerSlice` | 5,000,000 | wasm instructions per execution slice before the computer yields to the next tick |

:::tip[Raise `asyncCallsPerTick`]
The default of 1 keeps wasm computers at Lua's throughput: async programs
keep running while their calls wait, but still get one indirect call per
tick. You'll probably want to raise it. **16** lets async programs overlap
several indirect calls in one tick (several transposers, internet reads or
robot actions at once), which is the point of the async API. Each queued
call still costs the server what it would cost a Lua computer.
:::

The budget itself is still OpenComputers' `callBudgets` (by CPU and memory
tier), which applies to Lua and wasm alike. Method calls on objects, such as
an internet request's `read`, are charged OpenComputers' usual rate, not the
multiplied one.

## JVM options

| option | effect |
|---|---|
| `-Dwackasm.native=<path>` | loads that build of the native library instead of the bundled one |
| `-Dwackasm.workdir=<dir>` | moves the native library's work directory |

## Files the mod writes

Everything is written by the native library, under `<world>/wackasm/` (or
`-Dwackasm.workdir`):

| path | contents |
|---|---|
| `machines/<key>.state` | suspended computers. The computer's NBT only stores `<key>` |
| `logs/<key>.log` | what programs log with `computer::log`, and the output of a WASI program running as the first process. Rotated past a size limit |

Snapshots of computers that were broken while running stay in `machines/`,
because OpenComputers has no removal hook. They're safe to delete while the
server is stopped.
