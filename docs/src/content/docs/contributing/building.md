---
title: Building from source
description: Build the mod, the native library and the guest programs, and test without Minecraft.
sidebar:
  order: 1
---

## Requirements

- Rust **nightly** (wackasm uses nightly features), with the
  `wasm32-unknown-unknown` and `wasm32-wasip1` targets:
  `rustup target add wasm32-unknown-unknown wasm32-wasip1`;
- [`patch-crate`](https://crates.io/crates/patch-crate), which provides
  `cargo patch-crate` (`cargo install patch-crate`). The interpreter builds against a
  patched `wasmparser` (`patches/`), applied with `cargo patch-crate`;
- JDK 25 or newer, to run Gradle (RetroFuturaGradle 2.x needs it). Gradle downloads the Java 8 toolchain
  Minecraft needs;
- Node.js 22 or newer, for these docs.

## Layout

```
crates/wackasm/                 the interpreter
crates/wackasm-wasi/            WASI preview 1 marshalling
crates/wackasm-oc/              the JNI library: machines, processes, WASI, snapshots
crates/wackasm-opencomputers/   the guest library (published on crates.io)
crates/wackasm-oc-bios/         the 4 KiB EEPROM BIOS (wasm32-unknown-unknown)
crates/wackasm-oc-guest/        the shell, the floppy's /init.wasm
crates/wackasm-oc-programs/     the floppy's /bin programs (wasm32-wasip1)
mc-mod/                         the Forge mod (Java)
docs/                           this site (Starlight)
```

The BIOS, shell and programs build for wasm only, so they're separate cargo
workspaces.

## The mod

```sh
cargo patch-crate
cd mc-mod
./gradlew build        # build/libs/wackasm-oc-<version>.jar
./gradlew runClient    # a development client with OpenComputers
```

`build` runs cargo for the native library, the BIOS, the shell and the
programs, then packages them: the library under `natives/<os>-<arch>/`, the
BIOS as `assets/wackasm/bios.wasm`, and the floppy under
`assets/wackasm/loot/shell/`. It fails if the BIOS outgrows a 4096-byte
EEPROM. `-Pskip_cargo=true` packages what is already built.

The jar contains the native library for the platform it was built on. For a
multi-platform jar, put other platforms' builds in
`mc-mod/natives-extra/<os>-<arch>/` first (for example
`linux-x86_64/libwackasm_oc.so`). The release workflow does this.

## Tests

```sh
cargo test --workspace         # interpreter, spec tests, machine and ABI tests
cd mc-mod && ./gradlew test    # Java codec and call-budget tests
```

## Without Minecraft

The `headless` example runs a computer against simulated components: an
EEPROM, a GPU and screen, a keyboard that types a script, a disk backed by a
host directory, and optionally an internet card that makes real connections.

```sh
cargo run -p wackasm-oc --example headless -- \
  --eeprom crates/wackasm-oc-bios/target/wasm32-unknown-unknown/release/wackasm_oc_bios.wasm \
  --disk ./disk --ram 384 --keys 'help\rls /bin\r' --internet
```

`./disk` needs the shell as `init.wasm` and programs in `bin/`. In `--keys`,
`\r` is Enter, `\b` backspace and `\d` Ctrl+D. The machine is saved and
reloaded every few slices, which exercises suspension. At the end it prints
the screen and counts of the calls made.

## The docs

```sh
cd docs
npm ci
npm run dev       # http://localhost:4321/wackasm/
```

The site links to the Rust API docs at `/wackasm/rustdoc/`, which CI builds
and adds after the Starlight build. To preview them locally, build them and
copy them into `docs/public/` (ignored by git):

```sh
RUSTDOCFLAGS="--enable-index-page -Zunstable-options" cargo doc --no-deps   -p wackasm-opencomputers -p wackasm -p wackasm-wasi -p wackasm-oc   --features wackasm-opencomputers/async
cp -r target/doc docs/public/rustdoc
```

rustdoc's links are all relative, so its output works under any path.
