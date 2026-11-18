---
title: How it works
description: The JNI boundary, suspension, processes, the module cache and WASI replay.
sidebar:
  order: 2
---

```
guest.wasm ──imports "oc".*──▶ OcMachine (Rust) ◀──JNI: run / payload / respond──▶ WackasmArchitecture (Java) ──▶ OpenComputers
                                    │
                                    └── <world>/wackasm/machines/<key>.state, logs/<key>.log
```

## Pull-based JNI

Rust never calls into Java and holds no JNI references between calls. Java
drives everything with a few static natives that take primitives and
`byte[]` in the [value encoding](/wackasm/reference/abi/#value-encoding).
That keeps the boundary thread-agnostic, which matters because OpenComputers
calls `runThreaded` on worker threads and `runSynchronized` on the server
thread.

| native | purpose |
|---|---|
| `long create(workDir, key, address)` | a new machine handle |
| `void destroy(handle)` | frees it |
| `void setMemoryLimit(handle, bytes)` | from `recomputeMemory` |
| `void boot(handle, eepromOrNull)` | runs the EEPROM's wasm |
| `int run(handle, uptime, fuel)` | one slice; returns a `RUN_*` code |
| `byte[] payload(handle)` | the pending request |
| `void respond(handle, status, payloadOrNull)` | completes it |
| `void save(handle)`, `boolean load(handle)`, `void discard(handle)` | snapshots |

| `run` code | payload | Java does |
|---|---|---|
| 0 YIELD | | `Sleep(0)` |
| 1 HALTED | | `Shutdown(false)` |
| 2 SHUTDOWN | `[reboot]` | `Shutdown(reboot)` |
| 3 ERROR | `[message]` | `Error(message)` |
| 4 INVOKE | `[address, method, args]` | direct: call inline; else `SynchronizedCall` |
| 5 PULL_SIGNAL | `[remaining seconds or nil]` | pop a signal and respond, `TIMEOUT` at ≤ 0, else `Sleep(ticks)` |
| 6 PUSH_SIGNAL | `[name, args...]` | `machine.signal` |
| 7 COMPONENTS | `[filter]` | map of address → type |
| 8 METHODS | `[address]` | map of name → `{direct, doc}` |
| 9 BEEP | `[freq, ms]` | `machine.beep` |
| 10 DEFAULT_DISK | `[]` | `[address]` of the disk for WASI files |
| 11 INVOKE_VALUE | `[userdata, method, args]` | `machine.invoke(Value, ...)` |
| 12 RELEASE_VALUE | `[userdata]` | dispose of the value |
| 13 INVOKE_ASYNC | `[ticket, address or userdata, method, args]` | like INVOKE; the answer goes to the guest's completion queue |

## Suspension

When a guest calls an import that needs the game, wackasm yields and leaves
the import pending in the executor. `run` returns a request code instead of
an answer. Java serves the request whenever and wherever it can, then calls
`respond`, which completes the import so the next `run` continues.

The suspended executor is serializable, so a computer can be saved at any
point: between slices, while a component call waits for the server thread,
while waiting for a signal (the deadline is absolute machine uptime, so
timeouts survive reloads), or while the call budget is spent. Snapshots are
bitcode, written atomically to the work directory. NBT stores only the
machine's key and the userdata table.

## Processes

`exec` replaces the running program. `spawn` pushes a child onto a stack of
processes. Only the highest process that isn't paused runs. A child pauses
when it has an event for its parent (output, a read from stdin, its exit),
and the parent's `spawn` or `resume` returns that event. A middle process can
relay its child's events upward. The stack is part of the snapshot. All
processes share the machine's RAM: each is capped at the total minus what
the others hold. A child's trap ends only that child.

## Async calls

`invoke_async` appends a call to a machine-wide queue and returns a ticket;
the guest keeps running. Whenever the guest waits or its slice ends, `run`
reports the head of the queue (`INVOKE_ASYNC`) before anything else, and
Java serves it like any other call. Its answer goes into the completion
queue of the process that made it, instead of completing an import. A
blocking request made while calls are queued is simply not reported yet:
the import stays pending, runs again once the queue is empty, and asks for
the same thing, so order is preserved.

From `runSynchronized`, Java serves up to `asyncCallsPerTick` indirect calls
per tick: after each answer, the native `nextAsync` makes the next queued
call pending without running the guest. The queue, tickets and completion
queues are part of the snapshot. Calls belong to a process by id, so the
calls of a process that has exited are dropped.

## Module cache

Most of the time to start a program goes into reading it: OpenComputers caps
`filesystem.read` at 2 KiB and limits direct calls per tick, so a 60 KB
program takes about 30 calls and several ticks, while compiling takes a few
milliseconds. The host keeps a server-wide cache (64 MiB, least recently
used evicted) of compiled modules, keyed by disk, path, size and
modification time. A program that ran before, on any computer, starts after
two component calls.

## WASI and replay

`wackasm-wasi`'s methods answer synchronously, but each disk access is a
request to Java. File imports therefore use **replay**: a method asks for the
answer to its *n*-th disk call; if it isn't cached yet, the request is
recorded, the import stays pending, and Java gets the request. The answer is
cached and the import runs again from the start, consuming cached answers in
order. Methods change their own state only after their last disk call, so
re-running is harmless, and the cache is part of the snapshot.
