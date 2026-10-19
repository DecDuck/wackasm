# git clone https://github.com/WebAssembly/spec.git /tmp/wasm-spec

cargo run --release -p wackasm-spec-tests -- /tmp/wasm-spec/test/core --report test-results.tsv