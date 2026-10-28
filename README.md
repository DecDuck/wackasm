# wackasm

A deliciously slow and excitingly unconformant WASM interpreter with only one goal: **we can save and restore the currently running guest whenever we like, without the guest knowing.**

In more words, `wackasm` was built so that you could run a WASM module that uses typical blocking code, but lets you suspend the entire interpreter, including call stack, function frames, etc etc to any medium, like disk, over the network (or you could even put it in a database, if you're insane), and then read it back and resume executing, completely transparently to the guest.

## performance

I am not good at writing WASM interpreters, so for this reason, the code is built more around being understandable, rather than very fast. However, wackasm generally gets around ~100-120 million operations/s on the CPUs I test on, so it's not *prohibitively slow*. Because that number mostly tracks clock speed, `cargo bench --bench percent_of_wasmi` instead reports how fast wackasm runs a prime sieve relative to [wasmi](https://github.com/wasmi-labs/wasmi) running the exact same `.wasm` file with fuel metering on (currently ~10% of wasmi).

## support

`wackasm` is written with fleshy intelligence, so support for the mundane stuff like SIMD instructions, and a lot of mundane instructions like that, just haven't been done yet. No reason, just haven't (it's kinda boring).

Of the couple tens of thousands of WASM tests I found, these are the results:

**total: 18501 passed, 914 failed, 45409 unsupported**

## what's this for?

It's for an WASM OpenComputers architecture. The current ones out there piss me off because I can't write shitty code, i.e. you have to work around the fact that the WASM executors they use can't resume in-process state, so use hacky workarounds that make you write your code differently. 