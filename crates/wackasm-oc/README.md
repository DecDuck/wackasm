# wackasm-oc

An [OpenComputers](https://github.com/MightyPirates/OpenComputers) architecture
that runs WebAssembly guests on wackasm. The crate builds a `cdylib`
(`wackasm_oc.dll` / `libwackasm_oc.so`) loaded by the Java classes in `java/`.

```
 guest.wasm ──imports "oc".*──▶ OcMachine (Rust) ◀──JNI: run / payload / respond──▶ WackasmArchitecture (Java) ──▶ OC Machine API
                                     │
                                     └── <work_dir>/machines/<key>.state, logs/<key>.log, boot.wasm
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
NBT stores only `wackasm:key`. Guest logs go to `<work_dir>/logs/<key>.log`.
If the EEPROM doesn't start with `\0asm` (a Lua BIOS, say),
`<work_dir>/boot.wasm` is booted instead, because an EEPROM is too small for
most wasm programs.

## JNI surface: `dev.wackasm.oc.WackasmNative`

| method | purpose |
|---|---|
| `long create(workDir, key, address)` | new handle; `key` must match `[A-Za-z0-9_-]{1,64}` |
| `void destroy(handle)` | free; 0 is a no-op |
| `void setMemoryLimit(handle, bytes)` | from `recomputeMemory` |
| `void boot(handle, eepromOrNull)` | fresh program; deletes the old snapshot |
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

Put the library on `java.library.path`, or point `-Dwackasm.native=<path>` at
it. Register the architecture during mod init with
`li.cil.oc.api.Machine.add(WackasmArchitecture.class)`. The JVM options
`-Dwackasm.workdir` and `-Dwackasm.fuel` set the work dir and the
instructions per slice.

## Known limitations

* `WackasmArchitecture.java` targets the OC 1.12 API but hasn't been compiled
  against the OC jar. `WackasmNative` and `OcValueCodec` were tested against
  the real DLL.
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
