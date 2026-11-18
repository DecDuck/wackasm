# Wackasm for OpenComputers

A Minecraft 1.21.1 Fabric mod that adds a **WebAssembly** architecture to
[OpenComputers: Modernized](https://github.com/habzg/OpenComputers-Modernized),
running guests on the wackasm interpreter. Machines are fully suspendable: a
computer can be saved and reloaded mid-instruction, even in the middle of a
component call.

Player and programmer documentation: <https://decduck.github.io/wackasm/>
(source in [`docs/`](../docs)). This file covers building the mod.

```
mc-mod/                     this Fabric mod (Java)
crates/wackasm-oc/          JNI library: suspension, snapshots, guest ABI
crates/wackasm-oc-bios/     the 4 KiB WebAssembly BIOS (EEPROM code)
crates/wackasm-oc-guest/    the "wackasm shell" floppy's /init.wasm
```

## Building

Requirements:

* a Rust **nightly** toolchain (wackasm uses nightly features) with the
  `wasm32-unknown-unknown` and `wasm32-wasip1` targets
  (`rustup target add wasm32-unknown-unknown wasm32-wasip1`)
* [`patch-crate`](https://crates.io/crates/patch-crate) (`cargo install
  patch-crate`), then `cargo patch-crate` once at the repository root
* JDK 25 or newer to run Gradle (Fabric Loom needs it); the mod is compiled
  with a Java 21 toolchain for Minecraft 1.21.1, which Gradle downloads if
  it isn't installed

```powershell
cd mc-mod
./gradlew build        # build/libs/wackasm-oc-<version>.jar
./gradlew runClient    # dev client with OpenComputers: Modernized (in runs/client)
./gradlew runServer    # dev server (in runs/server)
./gradlew test         # codec, call-budget, memory and config tests
```

`build` runs cargo for the JNI library, the BIOS, the shell and the
programs, then packages the library under `natives/<os>-<arch>/`, the BIOS as
`assets/wackasm/bios.wasm` and the floppy under `assets/wackasm/loot/shell/`.
It fails if the BIOS outgrows a 4096-byte EEPROM. Pass `-Pskip_cargo=true` to
package what is already built.

1.21 recipes are data, and the EEPROM recipe's result contains the BIOS's
bytes, so `generateRecipes` writes both recipes (EEPROM + paper, floppy +
paper) from the built BIOS. They must make the same items as the
`Items.registerEEPROM` and `Items.registerFloppy` calls in `WackasmMod`.

OpenComputers: Modernized sets up its API in its own main entrypoint, and
Fabric doesn't order main entrypoints by dependency, so `WackasmMod`
registers as soon as the API exists: in its main entrypoint if
OpenComputers' ran first, otherwise in the client or dedicated-server
entrypoint, which Fabric runs after every main entrypoint.

The jar only contains a library for the platform it was built on. For a
multi-platform jar, put cross-built libraries in
`mc-mod/natives-extra/<os>-<arch>/` (for example
`linux-x86_64/libwackasm_oc.so`) before building.

## Playing

It works like a Lua computer: the EEPROM holds a BIOS, and the BIOS boots an
OS from a disk.

1. Build a computer as usual: case, CPU, RAM, GPU, screen and keyboard.
2. Sneak and right-click the CPU item until its tooltip says
   **WebAssembly (wackasm)**, then put it in the case.
3. Insert an **EEPROM (WebAssembly BIOS)**: craft an EEPROM with a piece of
   paper (shapeless), or take one from the OpenComputers creative tab.
4. Insert the **wackasm shell** floppy: craft a floppy disk with a piece of
   paper, or take it from the creative tab. Any disk with a `/init.wasm` on it
   works too.
5. Turn it on. The BIOS binds the GPU to the screen, finds `/init.wasm`,
   beeps, and starts it. The shell understands `help`, `info`, `components`,
   `cd`, `pwd`, `ls`, `beep`, `clear`, `reboot` and `shutdown`.

The errors are the ones Lua players know: no EEPROM gives "no bios found;
install a configured EEPROM", an EEPROM without wasm gives "EEPROM does not
contain a WebAssembly BIOS", and no usable disk gives "no bootable medium
found: <reason>". Like the Lua BIOS, it remembers the disk that booted in the
EEPROM's data, and tries that disk first next time.

## Writing programs for the shell

The shell runs ordinary **WASI preview 1** binaries. Typing `name args...`
runs `/bin/<name>.wasm` from the boot disk, or from any other disk, or a path
given directly. The program gets `args` as its argv. Its stdout and stderr go
to the screen, and reading stdin waits for a line typed at the shell (Ctrl+D
sends end of file). A non-zero exit code is shown as `[exit N]`.

The first run of a program reads it from disk, which takes a few ticks for a
typical 60 KB binary. After that the server keeps it compiled, so running it
again, on any computer, starts almost immediately. Saving a new version
(different size or modification time) makes the next run load it fresh.

Files work through ordinary `std::fs`. A program sees one disk as `/`, and
relative paths start at the shell's current directory (`cd`, shown in the
prompt). The disk is chosen the first time the program touches a file:
1. the disk the computer booted from, if it's writable;
2. otherwise the first writable disk (so with the read-only shell floppy
   inserted, files go to your hard drive);
3. otherwise a read-only disk.

The computer's tmpfs is never used. The shell's `cd` and `ls` work on that
same disk.

A program starts in the directory it was run from (it's passed as `PWD`) if
it links the [`wackasm-opencomputers`](../crates/wackasm-opencomputers) crate, which changes
into it before `main`. `use wackasm_opencomputers as _;` is enough. A program that
doesn't link the crate starts in `/`.

The floppy ships examples
([`crates/wackasm-oc-programs`](../crates/wackasm-oc-programs)): `echo`,
`hello`, `gfx [SCENE]` (a graphics demo), `cat FILE...`, `write FILE [TEXT...]` (without TEXT it saves what you
type until Ctrl+D), `dir [PATH]`, `pwd`, `demo`, `flash` (see below), two that
need an internet card:
`curl [-i] [-I] [-f] [-X METHOD] [-H "Name: value"] [-d DATA|@FILE] [-o FILE | -O] URL`
and `nc [-C] [-w SECS] HOST PORT` (sends the lines you type, then prints
replies until the connection closes or goes quiet), `fetch URL...` (downloads
several URLs at once with the crate's `async` feature), and three that run other programs:
`time PROGRAM ARGS...`, `env [NAME=VALUE...] [PROGRAM ARGS...]` and
`lines PROGRAM ARGS...` (numbers a program's output). Programs start with
`PATH=/bin` and can run others through `wackasm_opencomputers::process::Command`.

```powershell
cargo new --bin mytool; cd mytool
# Keep the stack small: rustc's default 1 MiB is more RAM than most computers have.
$env:RUSTFLAGS = "-C link-arg=-zstack-size=65536"
cargo build --release --target wasm32-wasip1
# then copy target/wasm32-wasip1/release/mytool.wasm to /bin/ on a disk
```

From the shell, `mytool some "quoted words"` runs it. To get the file onto a
disk:
* with an internet card, host it anywhere the server can reach and download it
  from the shell: `cd /bin`, then `curl -O https://example.com/mytool.wasm`
  (or `curl -o /bin/mytool.wasm URL`). Downloads go to the disk programs use
  (see above), and `/bin` there is searched like the floppy's;
* or copy it into the disk's folder on the server,
  `<world>/opencomputers/<disk address>/`.

### Flashing the EEPROM

`flash FILE [LABEL]` writes your own BIOS to the EEPROM, after checking that
it's a wasm module with an `oc_main` (or `_start`) export and that it fits,
and asking for confirmation (`-y` skips that). It keeps the EEPROM's data,
the remembered boot disk. `flash -r FILE` backs up the current code,
`flash -i` shows the label, size and checksum, and `flash /bios.wasm`
restores the stock BIOS from the shell floppy. Reboot to run what you flashed.

An EEPROM holds 4096 bytes, so the code must be a tiny `oc_main` program
(`no_std`, `wasm32-unknown-unknown`), like
[`crates/wackasm-oc-bios`](../crates/wackasm-oc-bios). An ordinary WASI
program is 60 KB or more and won't fit. To replace the shell with your own
program instead, make it a disk's `/init.wasm`: the stock BIOS boots that,
with no size limit.

Limits for now:
* no SIMD, reference types or tail calls in wackasm yet; every scalar
  integer and float instruction works;
* `poll_oneoff` (so `std::thread::sleep`), sockets, symlinks and
  `fd_pread`/`fd_pwrite` return `ENOSYS`;
* OpenComputers file handles are read-only, truncating or appending. So
  opening an existing file for writing without truncating it appends, and a
  file can't be open for reading and writing at once;
* every file operation is a component call, which costs game time like it
  does for Lua (reads come in 2 KiB chunks);
* a program that loops forever without output can't be interrupted from the
  shell; power-cycle the computer.

To talk to components (GPU, robot, modem, redstone, ...), use the
[`wackasm-opencomputers`](../crates/wackasm-opencomputers) crate. It has a typed wrapper for
every OpenComputers component, typed signals, and this computer's uptime,
memory and power; `demo` on the floppy shows it in use. The raw `oc` import
ABI underneath is in
[`crates/wackasm-oc/README.md`](../crates/wackasm-oc/README.md). Any wasm
binary exporting `oc_main` can also replace the shell as a disk's `/init.wasm`.

### RAM

Installed RAM (OpenComputers' `ramSizes`, added up) times `memoryMultiplier`
(default 1.8, the factor OpenComputers gives Lua on 64-bit servers) is the
guest's linear-memory budget. A wasm page is 64 KiB, and `memory.grow` past
the budget returns `-1`. The table shows installed amounts:

| stick | KiB | wasm pages |
|---|---|---|
| Tier 1 | 192 | 3 |
| Tier 1.5 | 256 | 4 |
| Tier 2 | 384 | 6 |
| Tier 2.5 | 512 | 8 |
| Tier 3 | 768 | 12 |
| Tier 3.5 | 1024 | 16 |

All processes share the budget. The shell takes 128 KiB, and so does each
running example program, so at 1.8x one tier 1 stick (5 usable pages) runs
the shell plus one program.

## Files the mod writes

Everything is written by the native library, under `<world>/wackasm/` (or
under `-Dwackasm.workdir`):

| path | contents |
|---|---|
| `machines/<key>.state` | suspended machine snapshots (NBT only stores `<key>`) |
| `logs/<key>.log` | output of the guest's `oc.log` |

## Configuration

`config/wackasm.properties` (created with comments on first start, and read
again whenever a world or server starts):

| setting | default | meaning |
|---|---|---|
| `callBudgetMultiplier` | 1.0 | how much further the per-tick direct-call budget goes for wasm computers. OpenComputers charges each direct call `1/limit` of the budget (`filesystem.read` has limit 15). Wasm computers pay that divided by this, so 2.0 gives them twice the direct calls per tick of a Lua computer with the same parts. Lua computers are unaffected. Range 0.1 to 16 |
| `memoryMultiplier` | 1.8 | memory for wasm computers, as a multiple of installed RAM. Lua computers are unaffected. Range 1.0 to 4.0 |
| `asyncCallsPerTick` | 1 | indirect calls a wasm computer's async queue (`oc.invoke_async`) may run per tick. 1 matches Lua; raising it (16 is reasonable) lets async programs overlap several indirect calls per tick. Range 1 to 64 |
| `fuelPerSlice` | 5,000,000 | wasm instructions per execution slice before the machine yields to the next tick |

The budget itself is still OpenComputers' own `callBudgets` (by CPU and
memory tier), which applies to Lua and wasm alike.

JVM options: `-Dwackasm.native=<path>` loads a specific build of the library
instead of the bundled one, and `-Dwackasm.workdir=<dir>` moves the native
work dir.
