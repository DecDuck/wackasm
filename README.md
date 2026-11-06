# wackasm

A deliciously slow and excitingly unconformant WASM interpreter with only one goal: **we can save and restore the currently running guest whenever we like, without the guest knowing.**

In more words, `wackasm` was built so that you could run a WASM module that uses typical blocking code, but lets you suspend the entire interpreter, including call stack, function frames, etc etc to any medium, like disk, over the network (or you could even put it in a database, if you're insane), and then read it back and resume executing, completely transparently to the guest.

## performance

The interpreter benchmarks against `wasmi`. Register IR with memory fast paths,
compare/branch fusion and constant-address accesses measured about **90%** of wasmi's performance on the existing
prime benchmark, up from about **47%**
with the stack IR. There is still performance headroom, and results vary by machine.

Measured on Windows on 2026-11-06 with
`cargo bench -p wackasm --bench percent_of_wasmi` (limit 1,000, rounds 10,000,
500 samples):

| Implementation | Throughput relative to wasmi | Fuel per run |
| --- | --- | --- |
| Stack IR baseline | 47.374% | 392,371 |
| Register IR | 80.906% | 312,809 |
| Register IR + memory fast paths | 82.373% | 312,809 |
| Register IR + memory fast paths + compare/branch fusion | 86.459% | 312,809 |
| Above + constant-address loads/stores | 89.661% | 312,809 |

Each run recalibrates wasmi, so percentages vary. Criterion did not detect a
statistically significant performance change for the compare/branch fusion run.
For constant-address accesses, it reported a statistically significant **3.7%**
throughput improvement over the preceding run.

Call-path fast paths measured **85.659%** of wasmi, versus **82.415%** in the
saved pre-change direct-dispatch rerun. Criterion reported a statistically
significant **3.9%** relative-throughput improvement against that baseline, not
against the earlier 89.661% measurement.

The optional `compact-instructions` feature reduces runtime instructions from
24 to 16 bytes on 64-bit hosts. A final paired rerun measured **82.415%** of wasmi
for default dispatch and **83.616%** for compact dispatch; Criterion classified the
difference as within its noise threshold. Earlier compact trials regressed, so
this is an opt-in instruction-memory saving, not a claimed throughput improvement.

```sh
cargo test -p wackasm --features compact-instructions
cargo bench -p wackasm --features compact-instructions --bench percent_of_wasmi
```

Also host/import functions are definitely lower than native WASM functions, so use them sparingly or do as much work as possible host side.

## execution

WebAssembly is compiled to a three-address register IR. Parameters and locals occupy
the first frame slots; validated operand-stack depths determine the remaining slots.
`v128` values use two slots. Local reads and constants are deferred until needed,
results can be written directly into locals, and common immediate operations and
compare/branch pairs are combined.

Default dispatch retains direct handler pointers. With `compact-instructions`,
a shared table resolves a 16-bit handler ID and handlers read aligned, tagless
operands directly from 12 inline bytes. Operand layouts and portable IR are
generated from one schema, without new per-instruction allocations. Serialization
still uses the same portable operators, so snapshots can move between dispatch
modes and suspended instruction indexes remain unchanged. Owned unsupported-op
descriptions are preserved across cloning, serialization and destruction.

Conditional jumps fuse supported i32 register/immediate comparisons and i64
register/immediate comparisons, including full-width `i64.eqz`. i64 immediates
are used only when sign extension from i32 preserves the constant exactly;
larger constants retain their full value in a register. Fusion does not cross
labels or discard comparison results written to locals.

Internal calls and returns stay inside threaded dispatch. Fuel is charged at loop
headers and calls, not plain blocks; a call with no fuel is suspended before entering
the callee. Import calls yield to the host, and their results are written back into
the caller's argument slots.

Indirect calls compare cached canonical signature IDs rather than walking
parameter/result types; structurally identical duplicate type declarations share
an ID. Resolved callee metadata is reused for frame setup. Local initialization
and return copying have dedicated zero-, one- and two-slot paths (including
two-slot `v128` results); larger copies retain their general paths. Signature IDs
and function metadata are rebuilt on load/restore, never serialized.

With finite fuel, loops and direct, mutual or indirect recursion eventually yield
`OutOfFuel` (or return/trap earlier). Loop headers and calls remain metered,
including retries after snapshot restoration.

Scalar and vector loads/stores use unaligned little-endian accesses after one
widened effective-address bounds check. Memory 0 has specialized handlers using a
cached pointer and length; other memories use validated indexes without repeating
index checks. Bulk copy/fill validate complete ranges before writing, including
overlapping copies. Cached views are refreshed after memory growth and each
dispatch entry, never serialized. Restoring a snapshot revalidates compiled memory
indexes before enabling unchecked lookup. Guest addresses always remain checked.

Constant address bases are folded with memory offsets into dedicated load/store
instructions, avoiding address-register materialization and reads for every
supported scalar and vector width. Folding requires an unsigned u32 sum without
overflow; other addresses retain register-based accesses. Runtime bounds checks
remain in both paths, including after memory growth and snapshot restoration.

If a start function calls an import, construction leaves initialization suspended.
Finish it with `execute`/`import_return` or `execute_with_imports` before calling an
export; exports cannot discard an unfinished start function.

Suspended frames store function indexes, instruction indexes and register-buffer
offsets, never runtime pointers. Function metadata and instruction handlers are
rebuilt on deserialization. The register IR changes the serialized format, so
snapshots from the previous stack-IR implementation must be recreated.

## imports

For the suspend/restore functionality, `wackasm` helps you out with opaque handles to host-side resources that are given an opportunity to *reactivate* when the engine gets deserialized. So for stuff like file handles and open sockets, you can re-open them when necessary (and you can use `Drop` to finialise them).

## support

`wackasm` is written with fleshy intelligence, so support for the mundane stuff like SIMD instructions, and a lot of mundane instructions like that, just haven't been done yet. No reason, just haven't (it's kinda boring).

Of the couple tens of thousands of WASM tests I found, ~~these are the results:~~

~~**total: 18501 passed, 914 failed, 45409 unsupported**~~

*this is now out of date, check [./test-history.txt](./test-history.txt) for the latest numbers*

Register-IR validation on 2026-11-06 against the core spec suite: **20,422 passed,
481 failed, 43,956 unsupported**, versus **20,216 passed, 687 failed, 43,956
unsupported** before the rewrite. Per-directive comparison found no regressions;
206 previously failing checks now pass. Memory fast paths, comparison fusion,
constant-address accesses, compact dispatch and call fast paths preserve every
spec classification. All 93 unit/integration tests pass in each dispatch mode, including
suspension/restore, unaligned accesses, memory growth, multi-memory, bounds traps,
register-IR regressions, canonical signatures, small call/return copies, bounded
recursive execution, every opcode's compact encoding and ownership,
constant-address lowering/overflow checks and
fused-comparison boundary checks against wasmi.

## what's this for?

It's for an WASM OpenComputers architecture. The current ones out there piss me off because I can't write shitty code, i.e. you have to work around the fact that the WASM executors they use can't resume in-process state, so use hacky workarounds that make you write your code differently. 

## ai usage

I didn't feel like hooking up wackasm to the wasm tests myself, so everything under `crates/wackasm-spec-tests/` is AI-generated. The register-IR compiler/runtime rewrite and its regression tests were also implemented with AI assistance.