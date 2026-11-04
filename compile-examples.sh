#!/usr/bin/env bash

set -euo pipefail

script_directory=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
examples_directory="$script_directory/examples"
compiler=$(command -v rs-wat2wasm || true)

if [[ -z "$compiler" ]]; then
    printf '%s\n' 'rs-wat2wasm was not found on PATH.' >&2
    exit 1
fi

shopt -s nullglob
wat_files=("$examples_directory"/*.wat)

if (( ${#wat_files[@]} == 0 )); then
    printf 'No .wat files found in %s.\n' "$examples_directory" >&2
    exit 1
fi

for wat_file in "${wat_files[@]}"; do
    wasm_path="${wat_file%.wat}.wasm"
    wat_name=${wat_file##*/}
    wasm_name=${wasm_path##*/}

    if ! "$compiler" < "$wat_file" > "$wasm_path"; then
        printf 'Failed to compile %s.\n' "$wat_name" >&2
        exit 1
    fi

    if [[ ! -s "$wasm_path" ]]; then
        printf 'Compiler produced an empty file for %s.\n' "$wat_name" >&2
        exit 1
    fi

    printf 'Compiled %s -> %s\n' "$wat_name" "$wasm_name"
done