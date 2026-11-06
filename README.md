# wackasm

A deliciously slow and excitingly unconformant WASM interpreter with only one goal: **we can save and restore the currently running guest whenever we like, without the guest knowing.**

In more words, `wackasm` was built so that you could run a WASM module that uses typical blocking code, but lets you suspend the entire interpreter, including call stack, function frames, etc etc to any medium, like disk, over the network (or you could even put it in a database, if you're insane), and then read it back and resume executing, completely transparently to the guest.

## performance

The interpreter benchmarks against `wasmi`. Register IR with memory fast paths,
compare/branch fusion, constant-address accesses, specialized branch handlers,
backedge inversion, store immediates, add/branch and add/load fusion, a
register-passing handler ABI and base+index addressing measured about **116%** of
wasmi's performance on the existing prime benchmark, up from about **47%**
with the stack IR. Results vary by machine.

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
| Frameless handlers, specialized branches, backedge inversion, store immediates | 100.19%–101.87% | 312,809 |
| Above + add/branch and add/load fusion, SysV handler ABI | 105.22% | 312,809 |

Each run recalibrates wasmi, so percentages vary. Criterion did not detect a
statistically significant performance change for the compare/branch fusion run.
For constant-address accesses, it reported a statistically significant **3.7%**
throughput improvement over the preceding run.

The latest row was measured against a same-machine rerun of the previous commit
(**88.371%**); Criterion reported a statistically significant **13.4%–15.3%**
throughput improvement. Frameless handlers and specialized branches alone were
within noise (+1.5%); most of the gain came from dispatching 12.4% fewer
instructions in the hot loops.

The final row is **+19.1%** over that same 88.371% rerun. Because the wasmi
calibration moved between 1.96 ms and 2.42 ms across runs, the steps were also
compared by interleaving one wasmi and one wackasm run 400 times and taking the
median ratio: about **108%** after add/branch and add/load fusion (14% fewer
dispatches), about **109%** after the SysV ABI change and about **116%** after
base+index addressing (wackasm median 1.76–1.84 ms, down from 1.89–1.97 ms).

With `LIMIT` raised to 5,000 (35,231,177 fuel per run), Criterion measured
**102.65%** of wasmi, **+16.2%** over the pre-session baseline saved at limit 1,000;
that baseline used the smaller workload, so the change is indicative only.

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
labels or discard comparison results written to locals. At load time each fused
branch selects a handler specialized for its comparison (with `invert` folded in)
and direction, so there is no runtime comparison switch.

An i32 add/sub immediate followed by a zero test of its result (`local.tee; br_if`
or `i32.eqz; br_if`) becomes one add-and-branch instruction, unless something
branches to the test. On a backedge it meters fuel before writing its result, so
an exhausted branch can be retried. An i32 add/sub immediate that only produces a
memory-0 load address is folded into a relative load, which wraps the add at
32 bits before applying the static offset exactly as the separate instructions would.
Likewise, a register-register `i32.add` that only produces a memory-0 address becomes
a base+index load or store (with a register or immediate value). Base+index forms
are keyed by access width, so e.g. `i32.load8_u` and `i64.load8_u` share one
handler. A store is not folded when its constant value would have to be
materialized into a register that the add read.

On x86-64, handlers use the SysV calling convention (also on Windows), which passes
six integer arguments in registers and has fewer callee-saved registers than
Win64. Memory 0's base and length are passed in two of those registers, so
memory-0 accesses do not reload them from the dispatch context; `memory.grow`
refreshes them. Other targets use the Rust ABI with the same signature.

Fuel stays in the dispatch context. Holding it in the sixth argument register made
every metering handler shorter, but measured about 3% slower on a Core Ultra 9 285H
in same-session A/B runs (also with 64-byte function alignment), so it was reverted.

A `br_if` immediately followed by a loop backedge `br` is inverted, so each
iteration takes one branch; this is skipped when the `br` is itself a branch target.
Constants assigned to locals are written directly, and constant values stored
to memory 0 use immediate store instructions (64-bit stores only when the value is
an exact sign-extended i32).

Hot handlers have no stack frame: traps, fuel exhaustion and other cold paths are
tail-called with the handler signature instead of being called.

Internal calls and returns stay inside threaded dispatch. Fuel is charged at loop
headers and calls, not plain blocks; a call with no fuel is suspended before entering
the callee. Entering a loop by fallthrough runs its header's `ConsumeFuel`; backward
branches charge the same fuel themselves and land just after the header, so totals
are unchanged while each iteration dispatches one fewer instruction. Import calls
yield to the host, and their results are written back into the caller's argument slots.

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
spec classification, as do frameless handlers, specialized branches, backedge
inversion, store immediates, add/branch and add/load fusion and the SysV handler
ABI (per-directive results identical to the previous commit). All unit/integration tests pass in each dispatch mode, including
suspension/restore, unaligned accesses, memory growth, multi-memory, bounds traps,
register-IR regressions, canonical signatures, small call/return copies, bounded
recursive execution, every opcode's compact encoding and ownership,
constant-address lowering/overflow checks, backedge fuel parity, store-immediate
lowering, add/branch retry after suspension, add/load 32-bit wrapping, base+index
wrapping and clobber guards, exact fuel after traps and
fused-comparison boundary checks against wasmi.

## what's this for?

It's for an WASM OpenComputers architecture. The current ones out there piss me off because I can't write shitty code, i.e. you have to work around the fact that the WASM executors they use can't resume in-process state, so use hacky workarounds that make you write your code differently. 

## ai usage

I didn't feel like hooking up wackasm to the wasm tests myself, so everything under `crates/wackasm-spec-tests/` is AI-generated. The register-IR compiler/runtime rewrite and its regression tests were also implemented with AI assistance.