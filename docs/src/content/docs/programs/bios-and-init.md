---
title: Custom BIOS and init
description: Replace the shell, or write your own EEPROM code.
sidebar:
  order: 8
---

A computer boots in two stages, like a Lua one:

1. The **EEPROM** runs first. The stock BIOS binds the GPU to the screen,
   finds a disk with `/init.wasm` (trying the remembered boot disk first),
   beeps, and hands over to it.
2. **`/init.wasm`** runs from then on. On the shell floppy, that's the shell.

## Replacing the shell

Put your program on a disk as `/init.wasm`. There's no 4 KiB limit here,
only the computer's RAM, and it can be either kind of program:

- a WASI program (`wasm32-wasip1`, with a `main`). As the first process it
  has no terminal: stdin is at end of file, and its output goes to the
  machine's log file on the server (see
  [Configuration](/wackasm/reference/configuration/)). Draw on the GPU with
  `wackasm-opencomputers` instead;
- an `oc_main` program, like the shell
  ([`crates/wackasm-oc-guest`](https://github.com/DecDuck/wackasm/tree/master/crates/wackasm-oc-guest)).

Either kind can run other programs with the
[process API](/wackasm/programs/processes/), and is then their terminal.

## Writing EEPROM code

EEPROM code must fit in **4096 bytes**. In practice that means:

- `#![no_std]`, built for `wasm32-unknown-unknown`;
- exporting `oc_main`, and calling the [`oc` imports](/wackasm/reference/abi/)
  directly;
- `opt-level = "z"`, LTO, `panic = "abort"`, no allocator if you can help it.

The stock BIOS,
[`crates/wackasm-oc-bios`](https://github.com/DecDuck/wackasm/tree/master/crates/wackasm-oc-bios),
is a working example at 3959 bytes. The smallest possible BIOS, in WAT:

```wat
(module
  (import "oc" "crash" (func $crash (param i32 i32)))
  (memory 1)
  (data (i32.const 0) "hello from my own BIOS")
  (func (export "oc_main") (call $crash (i32.const 0) (i32.const 22))))
```

```sh
wasm-tools parse mybios.wat -o mybios.wasm   # 114 bytes
```

Flash it with [`flash`](/wackasm/using/flashing/). To boot a disk the way the
stock BIOS does, read the file and call `oc.exec(image, len)`, which replaces
the running program with that one.
