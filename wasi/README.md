# WASI Preview1 import test guest

This guest uses Rust's standard library on `wasm32-wasip1` to exercise the
WASI operations exposed through normal APIs: process arguments and environment,
stdout, preopened directories, file I/O, metadata, timestamps, links, random
bytes, clocks, and sleeping/yielding.

Build it for `wasm32-wasip1`; the CLI runs the exported `_start` entry through
wackasm:

```powershell
cargo build -p wasi --target wasm32-wasip1 --release
cargo run -p wackasm-cli
```

The CLI runs the guest in `wackasm` with a confined directory under `target`.