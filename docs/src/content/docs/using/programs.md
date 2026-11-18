---
title: Included programs
description: The programs on the shell floppy's /bin.
sidebar:
  order: 2
---

The shell floppy comes with these programs in `/bin`. Their source is in
[`crates/wackasm-oc-programs`](https://github.com/DecDuck/wackasm/tree/master/crates/wackasm-oc-programs),
and they double as examples for [writing your own](/wackasm/programs/first-program/).

## Files

| program | does |
|---|---|
| `cat FILE...` | prints files |
| `write FILE [TEXT...]` | writes TEXT to FILE; without TEXT, saves the lines you type until Ctrl+D |
| `dir [PATH]` | lists a directory with file sizes |
| `pwd` | prints the current directory |

## Running other programs

| program | does |
|---|---|
| `time PROGRAM ARGS...` | runs a program, then prints its exit status and how long it took |
| `env [NAME=VALUE...] [PROGRAM ARGS...]` | prints the environment, or runs a program with variables added |
| `lines PROGRAM ARGS...` | runs a program and numbers the lines of its output |

These nest: `time lines cat notes.txt` works.

## Internet

These need an [internet card](/wackasm/using/internet/).

| program | does |
|---|---|
| `curl [OPTIONS] URL` | an HTTP or HTTPS request |
| `nc [-C] [-w SECS] HOST PORT` | a TCP connection |

## EEPROM

| program | does |
|---|---|
| `flash ...` | writes, backs up or inspects the EEPROM; see [Flashing the EEPROM](/wackasm/using/flashing/) |

The floppy also holds the stock BIOS as `/bios.wasm`, for restoring it.

## Others

| program | does |
|---|---|
| `echo ARGS...` | prints its arguments |
| `hello [ARGS...]` | asks your name and greets you; shows argv and stdin |
| `demo` | the `wackasm-opencomputers` crate in action: lists components, draws on the screen and waits for a key |
