# wackasm-oc

An [OpenComputers](https://github.com/MightyPirates/OpenComputers) architecture
that runs WebAssembly guests on wackasm. The crate builds a `cdylib`
(`wackasm_oc.dll` / `libwackasm_oc.so`) that the Fabric mod in [`mc-mod/`](../../mc-mod)
loads. The EEPROM BIOS is [`wackasm-oc-bios`](../wackasm-oc-bios) and the
example shell is [`wackasm-oc-guest`](../wackasm-oc-guest).

```
 guest.wasm ──imports "oc".*──▶ OcMachine (Rust) ◀──JNI: run / payload / respond──▶ WackasmArchitecture (Java) ──▶ OC Machine API
                                     │
                                     └── <work_dir>/machines/<key>.state, logs/<key>.log
```

## Design

**Pull-based JNI.** Rust never calls into Java and holds no JNI references
between calls. Java drives everything with a few static natives that take
primitives and `byte[]` in a shared tagged encoding (`src/value.rs` ⇄
`OcValueCodec.java`). This keeps the boundary thread-agnostic, which matters
because OC calls `runThreaded` on worker threads and `runSynchronized` on the
server thread.

**Suspension.** When the guest calls an `oc` import that needs the game,
wackasm yields `HostYield::ImportCall` and leaves the import *pending* in the
executor. `run()` returns a request code instead of answering. Java services
the request whenever and wherever it can, then calls `respond()`, which
completes the import (`import_return`) so the next `run()` continues. The
suspended executor is serializable, so a machine can be saved and restored at
any point, including:

* between fuel slices (`RUN_YIELD` → `Sleep(0)`);
* while a non-direct component call waits for `runSynchronized`
  (`SynchronizedCall`, then `runSynchronized` → `respond`);
* while waiting on `pull_signal`. The absolute deadline is stored, so a
  timeout counts machine uptime across reloads;
* while the direct-call budget is spent (`LimitReachedException` →
  `Sleep(0)`; the request stays pending and is reissued).

**Rust-owned working directory.** Snapshots are bitcode blobs with a
`WKOC`+version header, written atomically to `<work_dir>/machines/<key>.state`.
NBT stores only `wackasm:key` and the userdata table (see below). Guest logs
go to `<work_dir>/logs/<key>.log`.

**Booting, as Lua does.** `boot` runs the EEPROM's code, and it must be wasm:
a machine without an EEPROM stops with "no bios found; install a configured
EEPROM", the same as Lua. The bundled BIOS (4 KiB, a port of `bios.lua`):
1. binds the GPU to the screen;
2. tries the boot address stored in the EEPROM's data, then every other filesystem;
3. reads `/init.wasm` and beeps;
4. hands over with `oc.exec`, which swaps the running module for the one in
   guest memory, under the same memory limit;
5. stores the filesystem that booted as the new boot address.

If nothing boots it stops with "no bootable medium found: <reason>".

**Userdata.** Host objects such as `filesystem.open`'s `HandleValue` cross as
tag 9 plus a `u32` id. Java keeps the objects in a per-machine table that is
saved in NBT (class name plus `Value.save`, as OC persists Lua userdata) and
disposed of on shutdown.

**Processes.** `exec` replaces the caller. To run a program and get control
back, a process uses `spawn` instead, which pushes a child onto a stack of
processes. Only one runs at a time: the child, until it has something for
its parent, an *event*. The child then pauses, and the parent's `spawn` or
`resume` returns the event code with the payload in its response buffer:

| event | payload | parent's next step |
|---|---|---|
| 0 `EXITED` | `[code, last_output]` | the child is gone |
| 1 `OUTPUT` | `[bytes]` | show it, then `resume` |
| 2 `INPUT` | `[]` | `resume(line)`; an empty input means end of file |
| 3 `CRASHED` | `[message, last_output]` | the child is gone |

The parent is the terminal: the shell prints `OUTPUT` and collects a typed
line for `INPUT`. A middle process can relay its child's events to its own
parent (that is what `wackasm_opencomputers::process::Command::status` does), so the
running process is always the highest one that isn't paused. The stack is part of the snapshot, so a world can be saved
mid-program. A child's trap or bad import ends only that child, with
`CRASHED`. All processes share the machine's RAM: each is capped at the total
minus what the others hold.

**Module cache.** Most of spawn time is reading the program: OC caps
`filesystem.read` at 2 KiB and limits direct calls per tick, so a 60 KB
program takes about 30 calls and several ticks. Compiling it takes only
3 to 7 ms. `spawn_file` makes the host load the file, and keeps a server-wide
cache (`src/cache.rs`, 64 MiB, least recently used evicted) of compiled
modules keyed by disk, path, size and modification time. A program that ran
before, on any computer, starts after two component calls (`size`,
`lastModified`), with no reads and no compile. Each spawn instantiates a
fresh copy. Loading uses the same replay as WASI files, so it can be
suspended and saved. The image never passes through the caller's memory.

**WASI preview 1.** Every process can import `wasi_snapshot_preview1`, served
by [`wackasm-wasi`](../wackasm-wasi)'s marshalling over per-process stdio
(`src/process.rs`):

* `args_get`: the argv given to `spawn`. `environ_get`: the environment given
  to `spawn_with`, or the parent's (empty for the BIOS/OS).
* `fd_write` to 1 or 2: an `OUTPUT` event per write, in order.
* `fd_read` from 0: buffered input, or else it pauses with `INPUT`. The import
  stays pending and runs again after `resume`. A prompt written just before
  is delivered as `OUTPUT` first.
* `proc_exit`: `EXITED` with that code. Returning from `_start` means 0.
* `clock_time_get`: machine uptime (both clocks). `random_get`: host randomness.
* Files: fd 3 is a preopened `/` on the process's *disk*, a filesystem
  component Java picks on first use (`RUN_DEFAULT_DISK`): the boot disk if
  writable, else the first writable filesystem, else any. Never the tmpfs.
  `path_open`, `fd_read`/`fd_write`/`fd_seek`/`fd_tell`/`fd_close`,
  `fd_readdir`, `fd_filestat_get`, `path_filestat_get`,
  `path_create_directory`, `path_unlink_file`, `path_remove_directory` and
  `path_rename` become `filesystem.*` calls (`src/wasi.rs`). OC errors map to
  errnos (`ENOENT`, `EROFS`, `ENOSPC`, `ENODEV`, otherwise `EIO`).
* Working directory: WASI has none. wasi-libc keeps one in the guest
  (`chdir`/`getcwd`) and resolves relative paths against it before they reach
  the host, so a program always starts in `/`. The host passes the working
  directory as `PWD` (set by `spawn_with`'s `cwd`), and the `wackasm-opencomputers`
  crate changes into it before `main`.
* Anything else returns `ENOSYS`.

Each disk call is a Java request, but `wackasm-wasi`'s trait methods answer
synchronously, so file imports use **replay**. A method asks for the answer
to its *n*-th disk call. If the answer isn't cached yet, the request is
recorded, the import stays pending, and Java gets the request. The answer is
cached, and the import runs again from the start, consuming cached answers in
order. Methods change local state (the fd table) only after their last disk
call, and every run writes the same guest memory, so re-running is harmless.
The cache is part of the snapshot, so a world can be saved halfway through an
`open`.

## JNI surface: `dev.wackasm.oc.WackasmNative`

| method | purpose |
|---|---|
| `long create(workDir, key, address)` | new handle; `key` must match `[A-Za-z0-9_-]{1,64}` |
| `void destroy(handle)` | free; 0 is a no-op |
| `void setMemoryLimit(handle, bytes)` | from `recomputeMemory` |
| `void boot(handle, eepromOrNull)` | run the EEPROM's wasm; throws "no bios found..." otherwise; deletes the old snapshot |
| `int run(handle, uptime, fuel)` | one slice; returns `RUN_*` |
| `byte[] payload(handle)` | encoded array for the pending request or halt reason |
| `void respond(handle, status, payloadOrNull)` | finish the request, resume the guest |
| `void save(handle)` / `boolean load(handle)` / `void discard(handle)` | snapshots in the work dir |

Every method throws `IllegalStateException` on failure. Rust panics are
caught and rethrown as exceptions too.

| `run` code | payload | Java action |
|---|---|---|
| 0 `YIELD` | none | `Sleep(0)` |
| 1 `HALTED` | none | `Shutdown(false)` |
| 2 `SHUTDOWN` | `[reboot]` | `Shutdown(reboot)` |
| 3 `ERROR` | `[message]` | `Error(message)` |
| 4 `INVOKE` | `[address, method, args[]]` | direct → invoke inline; else `SynchronizedCall` |
| 5 `PULL_SIGNAL` | `[remainingSeconds \| nil]` | pop a signal and respond, respond `TIMEOUT` at ≤ 0, else `Sleep(ticks)` |
| 6 `PUSH_SIGNAL` | `[name, args...]` | `machine.signal` |
| 7 `COMPONENTS` | `[filter]` | respond with map address → type |
| 8 `METHODS` | `[address]` | respond with map name → `{direct, doc}` |
| 9 `BEEP` | `[freq, ms]` | `machine.beep` |
| 10 `DEFAULT_DISK` | `[]` | respond with `[address]` of the disk for WASI files, or `[]` |
| 11 `INVOKE_VALUE` | `[userdata, method, args[]]` | `machine.invoke(Value, ...)`: direct → inline; else `SynchronizedCall`. An unknown id is `NO_SUCH_COMPONENT` |
| 12 `RELEASE_VALUE` | `[userdata]` | dispose of the value, forget its id, respond `OK` |
| 13 `INVOKE_ASYNC` | `[ticket, address \| userdata, method, args[]]` | the head of the async queue: like `INVOKE`/`INVOKE_VALUE`, but the answer goes to the guest's completion queue. `runSynchronized` serves up to `asyncCallsPerTick` indirect calls, using `nextAsync` to take the next one |

`respond` statuses: `0 OK`, `1 TIMEOUT`, `-1 ERROR`, `-2 BAD_ARGUMENT`,
`-3 NO_SUCH_COMPONENT`, `-4 NO_SUCH_METHOD`. On `OK` the payload is an encoded
value; otherwise it is a UTF-8 message.

## Guest ABI: import module `oc`

Pointers are `i32` offsets into memory 0. The entry point is the export
`oc_main` or, failing that, `_start`.

| import | signature | notes |
|---|---|---|
| `invoke` | `(addr, addr_len, method, method_len, args, args_len) -> status` | `args` is an encoded array; results go in the response buffer |
| `pull_signal` | `(timeout_s: f64) -> status` | `< 0` waits forever; `0` = signal `[name, args...]`, `1` = timeout |
| `push_signal` | `(sig, sig_len) -> status` | encoded `[name, args...]` |
| `components` | `(filter, filter_len) -> status` | map address → type |
| `methods` | `(addr, addr_len) -> status` | map name → `{direct, doc}` |
| `beep` | `(freq: i32, ms: i32) -> status` | |
| `response_len` | `() -> i32` | Rust-only (no Java round trip) |
| `response_read` | `(dst, cap) -> i32 copied` | Rust-only |
| `uptime` | `() -> f64` | Rust-only, as of the start of this slice |
| `address` | `(dst, cap) -> i32 full_len` | Rust-only |
| `memory_limit` | `() -> i64` | Rust-only |
| `log` | `(msg, len)` | Rust-only, appends to the work-dir log |
| `exec` | `(image, image_len) -> status` | Rust-only; replaces the program with that wasm binary. Returns only on failure (`-1`, reason in the response) |
| `spawn` | `(image, image_len, argv, argv_len) -> event \| status` | Rust-only; runs that binary as a child with `argv` (encoded array of strings) until its first event. `-1` if it won't load |
| `spawn_file` | `(fs, fs_len, path, path_len, argv, argv_len) -> event \| status` | Rust-only; like `spawn`, but the host loads the file (from its module cache when it can). `-5` (`STATUS_NOT_FOUND`) if there is no such file |
| `spawn_with` | `(spec, spec_len) -> event \| status` | Rust-only; the general spawn. `spec` is an encoded map: `path` (with optional `disk`, else the caller's disk) or `image`, plus optional `argv`, `env` (`["NAME=value"]` or a map; default: the caller's) and `cwd` (the child's working directory, given to it as `PWD`). A relative `cwd` starts at the caller's `PWD`, and a relative `path` at the child's directory. The child shares the caller's WASI disk |
| `resume` | `(input, input_len) -> event \| status` | Rust-only; continues the paused child, appending `input` to its stdin |
| `kill` | `() -> status` | Rust-only; ends the child |
| `invoke_value` | `(value: u32, method, method_len, args, args_len) -> status` | a method of a userdata value (`value` is its id), such as an internet card request's `read`; results go in the response buffer. Direct callbacks run inline, others wait for the server tick, as for `invoke` |
| `release_value` | `(value: u32) -> status` | dispose of a userdata value (closing it) and forget its id. Values are otherwise kept until the machine stops |
| `invoke_async` | `(addr, addr_len, method, method_len, args, args_len) -> i64` | queues a call and returns a ticket, or `-2`/`-6` (queue full, 64 outstanding). Queued calls run in order, before any later blocking call |
| `invoke_value_async` | `(value: u32, method, method_len, args, args_len) -> i64` | the same for a userdata value |
| `completion_next` | `() -> status` | `0` with `[ticket, status, payload bytes]` in the response, or `1` if none |
| `cancel` | `(ticket: i64) -> status` | drops a queued call, or discards a running one's result |
| `wait_ms` | `(timeout_ms: i64) -> status` | `pull_signal_ms` that also returns `2` when a completion is ready |
| `files_disk` | `() -> status` | Rust-only; the address of this process's WASI disk (chosen as for a first file access) in the response, or `-5` if there is none |
| `shutdown` | `(reboot: i32)` | halts |
| `crash` | `(msg, len)` | halts with an error |

Bad guest pointers, invalid UTF-8 or malformed encodings return
`STATUS_BAD_ARGUMENT` (message in the response buffer) without a trip to Java.
Wrong import signatures or unknown imports crash the machine.

## Building and testing

```powershell
cargo build -p wackasm-oc --release   # target/release/wackasm_oc.dll
cargo test  -p wackasm-oc
```

The mod's Gradle build runs these cargo builds and bundles the outputs, so
for in-game use see [`mc-mod/README.md`](../../mc-mod/README.md).

To try the boot chain without Minecraft, run it against a fake EEPROM, GPU,
screen, keyboard and (optionally) a disk backed by a host directory:

```powershell
# BIOS on the EEPROM, a disk holding init.wasm, one tier 1 stick of RAM:
cargo run -p wackasm-oc --example headless -- --eeprom bios.wasm --disk ./disk --ram 192 --keys "help\rinfo\r"
```

## Known limitations

* wackasm doesn't implement SIMD (`v128` arithmetic), reference-type and
  table instructions (`table.get`, `ref.null`, ...), or tail calls yet. A
  guest that executes one crashes with "unsupported operator". Every scalar
  integer and float instruction is implemented and passes the spec tests
  (`i32.wast`, `i64.wast`, `int_exprs.wast`, `f32.wast`, `f64.wast`, ...).
* The memory limit (installed RAM, from `recomputeMemory`) is enforced by
  wackasm's `MemoryManager`. A `memory.grow` past it returns `-1`, so the
  guest's allocator sees out-of-memory instead of the machine being killed. A
  module whose *initial* memory exceeds the limit fails to boot. Lowering the
  limit while the machine runs keeps memory already allocated and only
  refuses later growth.
* A wasm `start` section runs inside `boot` with unlimited fuel, and it must
  not call `oc` imports. Use `oc_main`.
* Snapshots of machines that are broken without shutting down stay in
  `machines/` because OC has no removal hook. Clean them up out of band.
