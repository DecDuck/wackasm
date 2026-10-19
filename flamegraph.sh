#!/usr/bin/env bash

set -euo pipefail

script_directory=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)

if ! cargo flamegraph --version >/dev/null 2>&1; then
    printf 'cargo-flamegraph is missing; install it with: cargo install flamegraph\n' >&2
    exit 1
fi

cargo build \
    --release \
    --manifest-path "$script_directory/compute/Cargo.toml" \
    --target wasm32-unknown-unknown \
    --lib

cargo flamegraph \
    --no-inline \
    --manifest-path "$script_directory/Cargo.toml" \
    --bin wackasm \
    --cmd 'record -F 99 -e cpu-clock --call-graph dwarf' \
    --output "$script_directory/flamegraph.svg"