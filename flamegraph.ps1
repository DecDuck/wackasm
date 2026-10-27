$ErrorActionPreference = 'Stop'

& cargo flamegraph --version *> $null
if ($LASTEXITCODE -ne 0) {
    throw 'cargo-flamegraph is missing; install it with: cargo install flamegraph'
}

# The default Windows backend (blondie) records via ETW, which requires elevation.
$principal = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'cargo flamegraph on Windows must be run from an elevated (Administrator) shell.'
}

cargo build `
    --release `
    --manifest-path (Join-Path $PSScriptRoot 'compute/Cargo.toml') `
    --target wasm32-unknown-unknown `
    --lib
if ($LASTEXITCODE -ne 0) {
    throw "Failed to build compute (exit code $LASTEXITCODE)."
}

# wackasm-cli loads compute.wasm relative to the repository root.
Push-Location $PSScriptRoot
try {
    cargo flamegraph `
        --manifest-path (Join-Path $PSScriptRoot 'crates/wackasm-cli/Cargo.toml') `
        --bin wackasm `
        --output (Join-Path $PSScriptRoot 'flamegraph.svg')
    if ($LASTEXITCODE -ne 0) {
        throw "cargo flamegraph failed (exit code $LASTEXITCODE)."
    }
}
finally {
    Pop-Location
}
