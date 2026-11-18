---
title: Your first computer
description: Build a WebAssembly computer and boot the wackasm shell.
sidebar:
  order: 2
---

A wasm computer is built like a Lua one. The EEPROM holds a BIOS, and the
BIOS boots an operating system from a disk.

1. **Build a computer as usual:** a case, CPU, RAM, GPU, screen and keyboard.
   For running programs from the shell, use at least **384 KiB of RAM** (one
   tier 2 stick). See [Hardware](/wackasm/reference/hardware/).
2. **Switch the CPU to WebAssembly.** Sneak and right-click the CPU item
   until its tooltip says **WebAssembly (wackasm)**, then put it in the case.
3. **Insert an EEPROM (WebAssembly BIOS).** Craft a blank EEPROM with a piece
   of paper (shapeless), or take one from the OpenComputers creative tab.
4. **Insert the wackasm shell floppy.** Craft a floppy disk with a piece of
   paper, or take it from the creative tab.
5. **Turn it on.** The BIOS binds the GPU to the screen, finds `/init.wasm`,
   beeps, and starts the shell.

```
wackasm shell - WebAssembly on OpenComputers
type 'help' for commands, 'ls /bin' for programs
/> _
```

Add a hard drive if you want to save files: the shell floppy is read-only, so
programs write to the first writable disk. See
[Files and directories](/wackasm/using/files/).

## Boot errors

The errors are the ones Lua players know:

| message | meaning |
|---|---|
| `no bios found; install a configured EEPROM` | there is no EEPROM in the computer |
| `EEPROM does not contain a WebAssembly BIOS` | the EEPROM holds something else, such as a Lua BIOS |
| `no bootable medium found: <reason>` | no disk has a `/init.wasm` that loads |

Like the Lua BIOS, the wasm BIOS remembers which disk it booted from (in the
EEPROM's data) and tries that disk first next time. Any disk with a
`/init.wasm` boots, so you can replace the shell with your own program; see
[Custom BIOS and init](/wackasm/programs/bios-and-init/).
