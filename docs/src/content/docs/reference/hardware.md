---
title: Hardware
description: RAM, the call budget, and what parts a wasm computer needs.
sidebar:
  order: 1
---

## RAM

Installed sticks add up, using OpenComputers' `ramSizes`, and the total is
multiplied by `memoryMultiplier` (default **1.8**, the same factor
OpenComputers gives Lua on a 64-bit server). The result is the memory budget
for every program on the computer. Memory comes in 64 KiB wasm pages, so the
usable amount is the budget rounded down to whole pages.

| stick | installed KiB | at 1.8x | whole pages |
|---|---|---|---|
| Tier 1 | 192 | 345 | 5 (320 KiB) |
| Tier 1.5 | 256 | 460 | 7 (448 KiB) |
| Tier 2 | 384 | 691 | 10 (640 KiB) |
| Tier 2.5 | 512 | 921 | 14 (896 KiB) |
| Tier 3 | 768 | 1382 | 21 (1344 KiB) |
| Tier 3.5 | 1024 | 1843 | 28 (1792 KiB) |

Sticks add up before the multiplier, so two tier 1 sticks give 691 KiB.

All processes share the memory, and each can use what the others don't. Each
running program needs its own stack and data, about **128 KiB** for the
included programs, and the shell takes 128 KiB too. So a tier 1 stick runs
the shell plus one program, and running a program under another (`time cat
notes.txt`) needs a tier 1.5 stick or two tier 1s. A program that asks for
more memory than is free gets an allocation failure, not a crashed computer.

### How this compares to Lua

Lua programs share one VM, so a running Lua program costs only what it
allocates. A wasm program has fixed costs (its stack, static data and
allocator), but its code costs no RAM (it is compiled on the server), and its
data is compact: 4-byte pointers, and no per-value overhead like Lua's tables
and strings. So a data-heavy program fits in less memory than in Lua, while
running many small programs at once takes more.

## CPU and call budget

The CPU tier sets the call budget, as for Lua (OpenComputers' `callBudgets`).
Direct component calls draw on a per-tick budget; once it is spent, the
computer waits for the next tick. Other calls take a tick each. Reading a
file costs one direct `filesystem.read` call per 2 KiB.

WASI programs tend to make more small calls than Lua scripts. The server can
stretch the budget for wasm computers with `callBudgetMultiplier`; see
[Configuration](/wackasm/reference/configuration/).

## Other parts

| part | needed for |
|---|---|
| GPU, screen, keyboard | the shell |
| EEPROM (WebAssembly BIOS) | booting |
| a disk with `/init.wasm` | booting; the shell floppy has one |
| a writable disk | saving files (the shell floppy is read-only) |
| internet card | `curl`, `nc` and `wackasm_opencomputers::internet` |
