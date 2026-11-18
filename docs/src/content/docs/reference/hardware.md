---
title: Hardware
description: RAM, the call budget, and what parts a wasm computer needs.
sidebar:
  order: 1
---

## RAM

RAM sticks give a wasm computer the same amounts as OpenComputers' `ramSizes`
config. Installed sticks add up, and the total is the memory budget for every
program on the computer. A wasm page is 64 KiB.

| stick | KiB | wasm pages |
|---|---|---|
| Tier 1 | 192 | 3 |
| Tier 1.5 | 256 | 4 |
| Tier 2 | 384 | 6 |
| Tier 2.5 | 512 | 8 |
| Tier 3 | 768 | 12 |
| Tier 3.5 | 1024 | 16 |

Unlike Lua, no 1.8x "64-bit" scaling is applied: wasm memory is the same size
on every server.

All processes share the RAM, and each can use what the others don't. The BIOS
and the shell run on one tier 1 stick, but running programs from the shell
needs about **384 KiB** (one tier 2 stick). That covers the shell (128 KiB),
a program's file while it loads, and the program itself (128 KiB for the
included programs). A program that asks for more memory than is free gets an
allocation failure, not a crashed computer.

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
