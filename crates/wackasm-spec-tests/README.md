# wackasm-spec-tests

Runs WebAssembly `.wast` scripts against `wackasm` and reports each directive as passed, failed, or unsupported. Unsupported checks are not silently counted as passes.

Check out the official test suite separately, then point the runner at its core tests:

```sh
git clone https://github.com/WebAssembly/spec.git /tmp/wasm-spec
cargo run -p wackasm-spec-tests -- /tmp/wasm-spec/test/core
```

Pass a substring after the path to filter test file names:

```sh
cargo run -p wackasm-spec-tests -- /tmp/wasm-spec/test/core i32
```

Write failed and unsupported checks plus summaries to a tab-separated report:

```sh
cargo run -p wackasm-spec-tests -- /tmp/wasm-spec/test/core --report /tmp/wackasm-report.tsv
grep '^FAIL' /tmp/wackasm-report.tsv
```

Each issue line includes the status, test file, directive number, and detail. The report also includes per-file and overall totals.

Run the harness's self-tests with `cargo test -p wackasm-spec-tests`.

The runner converts `f32`, `f64`, and `v128` arguments and checks their `assert_return` values, including NaN patterns and SIMD lane patterns. Reference matching is limited to null and non-null function references; extern/host references, GC references, and function-reference identity are unsupported. These harness capabilities do not add executor support: tests still require the runtime to implement the operators and typed returns used by each module. Global reads, linking, exceptions, suspension, exhaustion, threads, and component directives are also unsupported.