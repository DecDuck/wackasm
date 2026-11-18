# Wackasm for OpenComputers

A Minecraft 1.12.2 Forge mod that adds a **WebAssembly** architecture to
[OpenComputers](https://github.com/MightyPirates/OpenComputers), running
guests on the wackasm interpreter. Machines are fully suspendable: a computer
can be saved and reloaded mid-instruction, even in the middle of a component
call.

```
mc-mod/                     this Forge mod (Java)
crates/wackasm-oc/          JNI library: suspension, snapshots, guest ABI
crates/wackasm-oc-bios/     the 4 KiB WebAssembly BIOS (EEPROM code)
crates/wackasm-oc-guest/    the "wackasm shell" floppy's /init.wasm
```

## Building

Requirements:

* a Rust **nightly** toolchain (wackasm uses nightly features) with the
  `wasm32-unknown-unknown` and `wasm32-wasip1` targets
  (`rustup target add wasm32-unknown-unknown wasm32-wasip1`)
* any JDK 17+ to run Gradle. Gradle downloads the Java 8 toolchain that
  Minecraft needs.

```powershell
cd mc-mod
./gradlew build        # build/libs/wackasm-oc-0.1.0.jar
./gradlew runClient    # dev client with OpenComputers loaded
./gradlew runServer
```

`build` runs cargo for the JNI library, the BIOS and the shell, then packages
the library under `natives/<os>-<arch>/`, the BIOS as `assets/wackasm/bios.wasm`
and the shell as `assets/wackasm/loot/shell/init.wasm`. It fails if the BIOS
outgrows a 4096-byte EEPROM. Pass `-Pskip_cargo=true` to package what is
already built. `./gradlew test` runs the Java codec tests.

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
it links the [`opencomputers`](../crates/opencomputers) crate, which changes
into it before `main`. `use opencomputers as _;` is enough. A program that
doesn't link the crate starts in `/`.

The floppy ships examples
([`crates/wackasm-oc-programs`](../crates/wackasm-oc-programs)): `echo`,
`hello`, `cat FILE...`, `write FILE [TEXT...]` (without TEXT it saves what you
type until Ctrl+D), `dir [PATH]`, `pwd`, `demo`, and three that run other programs:
`time PROGRAM ARGS...`, `env [NAME=VALUE...] [PROGRAM ARGS...]` and
`lines PROGRAM ARGS...` (numbers a program's output). Programs start with
`PATH=/bin` and can run others through `opencomputers::process::Command`.

```powershell
cargo new --bin mytool; cd mytool
# Keep the stack small: rustc's default 1 MiB is more RAM than most computers have.
$env:RUSTFLAGS = "-C link-arg=-zstack-size=65536"
cargo build --release --target wasm32-wasip1
# then copy target/wasm32-wasip1/release/mytool.wasm to /bin/ on a disk
```

From the shell, `mytool some "quoted words"` runs it. To get the file onto a
disk, either download it from a Lua computer running OpenOS with an internet
card (`wget`), or copy it into the disk's folder on the server,
`<world>/opencomputers/<disk address>/`.

Limits for now:
* no SIMD, and a few `i64` operations (`i64.rotl`, `i64.popcnt`, ...) are
  missing in wackasm; floats work fully;
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
[`opencomputers`](../crates/opencomputers) crate. It has a typed wrapper for
every OpenComputers component, typed signals, and this computer's uptime,
memory and power; `demo` on the floppy shows it in use. The raw `oc` import
ABI underneath is in
[`crates/wackasm-oc/README.md`](../crates/wackasm-oc/README.md). Any wasm
binary exporting `oc_main` can also replace the shell as a disk's `/init.wasm`.

### RAM

RAM sticks give the guest the same amounts as OpenComputers' `ramSizes`
config. Installed sticks add up, and the total is the guest's linear-memory
budget (a wasm page is 64 KiB). `memory.grow` past it returns `-1`.

| stick | KiB | wasm pages |
|---|---|---|
| Tier 1 | 192 | 3 |
| Tier 1.5 | 256 | 4 |
| Tier 2 | 384 | 6 |
| Tier 2.5 | 512 | 8 |
| Tier 3 | 768 | 12 |
| Tier 3.5 | 1024 | 16 |

Unlike Lua, no 1.8x "64-bit" scaling is applied: wasm memory is the same size
on every host. All processes share it: the BIOS and the shell run on one tier
1 stick, but running programs from the shell needs about **384 KiB** (one tier
2 stick). That covers the shell (128 KiB), the program's file while it loads,
and the program itself (128 KiB for the examples).

## Files the mod writes

Everything is written by the native library, under `<world>/wackasm/` (or
under `-Dwackasm.workdir`):

| path | contents |
|---|---|
| `machines/<key>.state` | suspended machine snapshots (NBT only stores `<key>`) |
| `logs/<key>.log` | output of the guest's `oc.log` |

## Configuration

`config/wackasm.cfg` (created on first start):

| setting | default | meaning |
|---|---|---|
| `callBudgetMultiplier` | 1.0 | how much further the per-tick direct-call budget goes for wasm computers. OpenComputers charges each direct call `1/limit` of the budget (`filesystem.read` has limit 15). Wasm computers pay that divided by this, so 2.0 gives them twice the direct calls per tick of a Lua computer with the same parts. Lua computers are unaffected. Range 0.1 to 16 |
| `fuelPerSlice` | 5,000,000 | wasm instructions per execution slice before the machine yields to the next tick |

The budget itself is still OpenComputers' own `callBudgets` (by CPU and
memory tier), which applies to Lua and wasm alike.

JVM options: `-Dwackasm.native=<path>` loads a specific build of the library
instead of the bundled one, and `-Dwackasm.workdir=<dir>` moves the native
work dir.
