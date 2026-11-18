---
title: Guest ABI
description: The oc import module, the value encoding, and process events, for programs that don't use the Rust crate.
sidebar:
  order: 3
---

Programs in any language can talk to the computer through the `oc` import
module. `wackasm-opencomputers` is a typed layer over these imports. Programs
can also import `wasi_snapshot_preview1`.

The entry point is the export `oc_main` or, failing that, `_start`. Pointers
are `i32` offsets into memory 0.

## Calling convention

Imports that return data put it in a per-process **response buffer**, and
return a status. Read the buffer with `response_len()` and
`response_read(dst, cap)`. It holds an encoded value on success, or a UTF-8
message on failure.

| status | meaning |
|---|---|
| `0` OK | |
| `1` TIMEOUT | `pull_signal` timed out |
| `-1` ERROR | the call failed; reason in the response |
| `-2` BAD_ARGUMENT | a bad pointer, invalid UTF-8 or a malformed encoding |
| `-3` NO_SUCH_COMPONENT | |
| `-4` NO_SUCH_METHOD | |
| `-5` NOT_FOUND | no such file, no files disk, or an unknown async ticket |
| `-6` QUEUE_FULL | `invoke_async`: 64 calls already outstanding |

Wrong import signatures or unknown imports crash the computer.

## Value encoding

Arguments and results use a small tagged encoding. Integers are little-endian
unless noted; lengths and counts are `u32`.

| tag | value | followed by |
|---|---|---|
| 0 | nil | |
| 1 | false | |
| 2 | true | |
| 3 | integer | `i64` |
| 4 | float | `f64` |
| 5 | string (UTF-8) | length, bytes |
| 6 | bytes | length, bytes |
| 7 | array | count, values |
| 8 | map | count, key/value pairs |
| 9 | userdata | `u32` id of a host object |

Userdata are host objects such as open file handles and internet requests.
Pass them back as arguments, or call their methods with `invoke_value`.

## Imports

### Components and signals

| import | signature | notes |
|---|---|---|
| `invoke` | `(addr, addr_len, method, method_len, args, args_len) -> status` | `args` is an encoded array; results in the response |
| `invoke_value` | `(value: u32, method, method_len, args, args_len) -> status` | calls a method of a userdata value, such as an internet request's `read` |
| `release_value` | `(value: u32) -> status` | disposes of a userdata value and forgets its id. Values are otherwise kept until the computer stops |
| `components` | `(filter, filter_len) -> status` | map of address → type |
| `methods` | `(addr, addr_len) -> status` | map of name → `{direct, doc}` |
| `pull_signal` | `(timeout_s: f64) -> status` | `< 0` waits forever. The signal is `[name, args...]` |
| `pull_signal_ms` | `(timeout_ms: i64) -> status` | the same, in milliseconds |
| `push_signal` | `(sig, sig_len) -> status` | encoded `[name, args...]` |
| `beep` | `(freq: i32, ms: i32) -> status` | |

Direct callbacks run immediately (drawing on the call budget); others wait
for the next server tick.

### Async calls

These queue a call and return at once; see [Async programs](/wackasm/programs/async/).
Queued calls run in the order they were made, before any blocking call made
after them.

| import | signature | notes |
|---|---|---|
| `invoke_async` | `(addr, addr_len, method, method_len, args, args_len) -> i64` | queues a component call. Returns a ticket (`>= 0`), or a negative status (`-2` bad argument, `-6` queue full) |
| `invoke_value_async` | `(value: u32, method, method_len, args, args_len) -> i64` | the same, for a method of a userdata value |
| `completion_next` | `() -> status` | `0`: the response holds `[ticket, status, payload]`, where payload is the bytes `invoke` would have left in the response. `1`: none have completed |
| `cancel` | `(ticket: i64) -> status` | forgets a call. If it hasn't run, it won't; if it's running, its result is dropped. `-5` for an unknown ticket |
| `wait_ms` | `(timeout_ms: i64) -> status` | like `pull_signal_ms`, but also returns `2` as soon as a call has completed. `pull_signal` and `pull_signal_ms` ignore completions |

A process can have 64 calls outstanding: queued, running, or completed and
not yet read.

### This computer

| import | signature | notes |
|---|---|---|
| `response_len` | `() -> i32` | |
| `response_read` | `(dst, cap) -> i32` | bytes copied |
| `uptime` | `() -> f64` | seconds, as of the start of this slice |
| `uptime_ms` | `() -> i64` | |
| `address` | `(dst, cap) -> i32` | writes the computer's address; returns its full length |
| `memory_limit` | `() -> i64` | bytes this process may use |
| `files_disk` | `() -> status` | address of the disk WASI file calls use |
| `log` | `(msg, len)` | appends a line to the computer's log on the server |
| `shutdown` | `(reboot: i32)` | |
| `crash` | `(msg, len)` | stops the computer with an error |

### Processes

| import | signature | notes |
|---|---|---|
| `exec` | `(image, image_len) -> status` | replaces the running program with that wasm binary. Returns only on failure |
| `spawn` | `(image, image_len, argv, argv_len) -> event \| status` | runs a binary as a child, until its first event |
| `spawn_file` | `(fs, fs_len, path, path_len, argv, argv_len) -> event \| status` | the same, but the host loads the file (from its compiled-module cache when it can) |
| `spawn_with` | `(spec, spec_len) -> event \| status` | the general spawn; see below |
| `resume` | `(input, input_len) -> event \| status` | continues the paused child, appending `input` to its stdin |
| `kill` | `() -> status` | ends the child |

`spawn_with` takes an encoded map:

| key | value |
|---|---|
| `path` | a wasm file, loaded from `disk` (a filesystem address) or the caller's files disk |
| `image` | or: the wasm binary itself, as bytes |
| `argv` | array of strings (default `[path]`) |
| `env` | `["NAME=value", ...]` or a map (default: the caller's environment) |
| `cwd` | the child's working directory, given to it as `PWD` |

A relative `cwd` starts at the caller's `PWD`, and a relative `path` at the
child's directory.

## Process events

Only one process runs at a time: the child, until it has something for its
parent. It then pauses, and the parent's `spawn*` or `resume` returns an
event code, with the payload in the response buffer:

| event | payload | parent's next step |
|---|---|---|
| 0 EXITED | `[code, last_output]` | the child is gone |
| 1 OUTPUT | `[bytes]` | show it, then `resume` |
| 2 INPUT | `[]` | `resume(line)`; an empty input means end of file |
| 3 CRASHED | `[message, last_output]` | the child is gone |

Negative values are statuses: the child didn't start. A process can relay its
child's events to its own parent, which is how output reaches the screen
through several levels.

## WASI preview 1

- `args_get`, `environ_get`: the argv and environment the process was started
  with.
- `fd_write` to 1 or 2: an OUTPUT event per write. `fd_read` from 0: buffered
  input, otherwise an INPUT event.
- `proc_exit`: EXITED with that code. Returning from `_start` means 0.
- `clock_time_get`: the computer's uptime. `random_get`: host randomness.
- Files: fd 3 is a preopened `/` on the files disk. Opening, reading,
  writing, seeking, directory listing, stat, mkdir, unlink, rmdir and rename
  become `filesystem.*` calls.
- Everything else returns `ENOSYS`.
