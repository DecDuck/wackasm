# wackasm-opencomputers

Typed Rust bindings to every OpenComputers component, for WASI programs
running on the WebAssembly (wackasm) architecture.

```sh
cargo add wackasm-opencomputers
cargo build --release --target wasm32-wasip1
```

Full documentation, including how to get programs onto a computer:
<https://decduck.github.io/wackasm/>.

```rust
use std::time::Duration;
use wackasm_opencomputers::prelude::*;

fn main() -> Result<()> {
    let gpu = Gpu::first().ok_or(Error::NoSuchComponent)?;
    let (w, h) = gpu.get_resolution()?;
    gpu.fill(1, 1, w, h, " ")?;
    gpu.set(1, 1, "hello from Rust", None)?;

    if let Some(Signal::KeyDown { char, player, .. }) =
        signal::pull_filtered(Some(Duration::from_secs(10)), |s| matches!(s, Signal::KeyDown { .. }))
    {
        println!("{player} typed {:?}", char::from_u32(char));
    }

    let robot = Robot::first().ok_or(Error::NoSuchComponent)?;
    robot.r#move(Side::Front)?;
    Ok(())
}
```

Build with `cargo build --release --target wasm32-wasip1`, keeping the stack
small (`RUSTFLAGS="-C link-arg=-zstack-size=65536"`), copy the `.wasm` to
`/bin/` on a disk, and run it from the wackasm shell. `crates/wackasm-oc-programs`
has a worked example (`demo`).

## What's in it

* **`components`**: one struct per component type, one method per callback:
  53 component types and 402 methods. `Gpu`, `Screen`, `Filesystem`, `Eeprom`,
  `Internet`, `Modem`, `Tunnel`, `Redstone`, `Robot`, `Drone`,
  `InventoryController`, `Transposer`, `Geolyzer`, `Hologram`, `Printer3d`,
  `Data`, `Database`, `Debug`, the Adapter's vanilla blocks (`Furnace`,
  `NoteBlock`, `Beacon`, ...) and the rest. Each method's docs are
  OpenComputers' own, plus the Lua signature and whether it is a direct call.
  Find components with `Gpu::first()`, `Gpu::all()` or `Gpu::at(address)`.
* **`component`**: `list`, `methods`, untyped `invoke`, and `Proxy` for
  components the wrappers don't know (other mods').
* **`signal`**: `pull`, `pull_filtered`, `push`, and a typed `Signal` for the
  standard events (keys, touch, clipboard, component added and removed,
  redstone, modem messages, motion, ...). Anything else arrives as `Signal::Other`.
* **`computer`**: address, uptime, memory, beep, shutdown and reboot, crash,
  logging to the server, and `files_disk()` (the disk `std::fs` uses).
* **`internet`**: HTTP(S) through an internet card,
  `internet::get(url)` or `internet::Request::new(url)` with `method`,
  `header`, `body`, then `send()`. That returns a `Response` with `status()`,
  `message()` and `headers()`, and the body as `std::io::Read`.
  `internet::TcpStream::connect(host, port)` is `Read + Write`, with
  `try_read` for polling. Dropping either closes and releases it. Reads come
  in 2 KiB chunks, one per tick.
* **`draw`**: a `Canvas` framebuffer with text, half-block pixels (two per
  cell), braille dots (2x4 per cell), lines, rectangles, circles and plots.
  `present()` sends only the cells that changed, merged into as few GPU calls
  as possible, optionally double-buffered through GPU video memory.
* **`process`**: running other programs, like `std::process`.
  `Command::new("cat").arg("notes.txt")`, with `env`, `envs`, `env_remove`,
  `env_clear`, `current_dir` and `disk`, then `status()` (this program's stdin and stdout),
  `output()` (captured), or `spawn()` for a `Child` you drive event by event
  (`next_event`, `write_stdin`, `close_stdin`, `kill`, `wait`,
  `wait_with_output`). Programs are found on `PATH` (default `/bin`) as
  `NAME.wasm` or `NAME`, on this program's disk, then the boot disk, then the
  others. Children inherit the environment, the disk and the current
  directory. One child at a time, and stderr is merged into stdout.
* **Working directory**: `std::env::current_dir`/`set_current_dir` and
  relative `std::fs` paths work. A program starts in the directory it was run
  from because the crate changes into `PWD` before `main`. That happens
  whenever the program links the crate, which can take a `use wackasm_opencomputers
  as _;`. In a build without LTO, call `process::enter_working_dir()` at the
  top of `main` to be sure.
* **`task`** (the optional `async` feature): a single-threaded executor
  (`task::run`, `spawn`, `join`, `join_all`, `sleep`, `timeout`, `signal`),
  an `_async` twin of every component method, `component::invoke_async`,
  and async internet (`get_async`, `send_async`, `read_async`,
  `connect_async`, ...). Calls are queued with the architecture and the
  program keeps running until their results come back, so one program can
  wait on several calls, timers and signals at once. Enable it with
  `features = ["async"]`; everything else works without it.
* `Side`, `Color` and `keys`: the codes robots, redstone and keyboards use.

Files are easiest through `std::fs`, which the architecture maps onto a disk.

## Type mapping

Generated from each callback's documented signature (`function(x:number,
y:number, value:string[, vertical:boolean]):boolean`):

| Lua | parameter | optional parameter | return |
|---|---|---|---|
| `number` | `impl Into<Number>` (ints, `f64`, `Side`, `Color`) | `Option<Number>` | `Number` |
| `string` | `impl AsRef<[u8]>` (`&str`, bytes, `Address`) | `Option<&str>` | `OcString` (bytes; `.as_str()`, `Display`) |
| `boolean` | `bool` | `Option<bool>` | `bool` |
| `userdata` | `Handle` | `Option<Handle>` | `Handle` |
| `table`, unions | `impl Into<Value>` | `Option<Value>` | `Value` |
| `data...` | `&[Value]` | | |
| `X or nil` | | | `Option<X>` |
| `a, b` | | | `(A, B)` |

Optional parameters are concrete types, so `None` needs no annotation.
Trailing omitted ones are dropped, so the component applies its own defaults.
A reply of `nil, "reason"` becomes `Err(Error::Failed(reason))` unless the
return type is optional. Some boolean methods report `false, "reason"`; those
return `Ok(false)`, and the reason is in the raw reply from
`Component::invoke`.

`Number` is an integer or a float, as OpenComputers sent it: integers stay
exact, and `Number::as_i64`, `as_f64` and friends convert either way.

## Regenerating

The wrappers come from OpenComputers: Modernized itself:
`tools/extract_callbacks.py` reads the `@Callback` annotations and component
names out of the mod jar into `tools/callbacks.json`, and
`tools/gen_components.py` turns that into `src/components.rs`. The current
wrappers are from OpenComputers: Modernized 1.10.7 (Fabric).

```powershell
python tools/extract_callbacks.py <opencomputers-fabric-1.10.7.jar> tools/callbacks.json
python tools/gen_components.py tools/callbacks.json src/components.rs
```

The jar is in Gradle's cache after building `mc-mod` (look for
`opencomputers-fabric-*.jar` under `~/.gradle/caches/modules-2`). The
extractor also reads OpenComputers 1.7.5 jars.

## Caveats

* Signatures come from OpenComputers' doc strings. Where a doc says `number`
  for something that is really an integer code, the wrapper takes
  `impl Into<Number>`, which accepts both.
* Methods that only some tiers or variants have (data card tier 3, wireless
  modem strength, ...) are on the one struct, and fail with
  `Error::NoSuchMethod` on parts that lack them.
* Calls cost what they cost in Lua: direct calls draw on the per-tick budget,
  and indirect ones wait for the next tick.
