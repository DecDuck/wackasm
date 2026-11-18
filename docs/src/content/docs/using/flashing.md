---
title: Flashing the EEPROM
description: Write your own BIOS to the EEPROM with flash, back it up, and restore the stock one.
sidebar:
  order: 5
---

`flash` works like OpenOS's: it writes code to the computer's EEPROM.

| command | does |
|---|---|
| `flash FILE [LABEL]` | checks FILE, asks for confirmation, writes it, and sets the label if given |
| `flash -y FILE [LABEL]` | the same without asking |
| `flash -r FILE` | saves the EEPROM's current code to FILE |
| `flash -i` | shows the label, size, entry point and checksum |
| `flash /bios.wasm` | restores the stock BIOS from the shell floppy |

Reboot to run what you flashed. `flash` keeps the EEPROM's data, where the
BIOS remembers which disk to boot, and reads the code back to check the write
took (a read-only EEPROM keeps its old code).

Before writing, `flash` checks that the file:

- is a WebAssembly module;
- exports `oc_main` or `_start`, so there is something to run;
- fits: an EEPROM holds **4096 bytes**.

```
/> curl -O https://example.com/mybios.wasm
saved 114 bytes to mybios.wasm in 0.6s
/> flash mybios.wasm "My BIOS"
write mybios.wasm (114 bytes) to the EEPROM? [y/N] y
flashed 114 bytes; reboot to run it
```

## What fits on an EEPROM

4096 bytes is small. An ordinary Rust program built for WASI is 60 KB or
more, so it won't fit. EEPROM code has to be a tiny `no_std` program built for
`wasm32-unknown-unknown` that talks to the [`oc` imports](/wackasm/reference/abi/)
directly, like the stock BIOS. See
[Custom BIOS and init](/wackasm/programs/bios-and-init/).

To run your own program **instead of the shell**, you don't need to flash
anything: make it a disk's `/init.wasm`. The stock BIOS boots that, and there
is no size limit.

A flashed EEPROM only runs on a WebAssembly CPU. A Lua computer accepts the
EEPROM but won't boot from it.
