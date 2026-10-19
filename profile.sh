#!/usr/bin/env bash

set -euo pipefail

script_directory=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
cd "$script_directory"

cargo build \
    --release \
    --manifest-path "$script_directory/compute/Cargo.toml" \
    --target wasm32-unknown-unknown \
    --lib

cargo run --release --bin wackasm