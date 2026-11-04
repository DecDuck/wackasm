$ErrorActionPreference = 'Stop'

$examplesDirectory = Join-Path $PSScriptRoot 'examples'
$compiler = Get-Command 'rs-wat2wasm.exe' -ErrorAction SilentlyContinue
if (-not $compiler) {
    throw 'rs-wat2wasm.exe was not found on PATH.'
}

$watFiles = @(Get-ChildItem -LiteralPath $examplesDirectory -Filter '*.wat' -File | Sort-Object Name)
if ($watFiles.Count -eq 0) {
    throw "No .wat files found in $examplesDirectory."
}

foreach ($watFile in $watFiles) {
    $wasmPath = [System.IO.Path]::ChangeExtension($watFile.FullName, '.wasm')
    $command = 'type "{0}" | "{1}" > "{2}"' -f $watFile.FullName, $compiler.Source, $wasmPath

    # Keep binary stdout redirection in cmd.exe to avoid PowerShell text encoding.
    & $env:ComSpec /d /c $command
    if ($LASTEXITCODE -ne 0) {
        throw "Failed to compile $($watFile.Name) (exit code $LASTEXITCODE)."
    }

    if ((Get-Item -LiteralPath $wasmPath).Length -eq 0) {
        throw "Compiler produced an empty file for $($watFile.Name)."
    }

    Write-Host "Compiled $($watFile.Name) -> $([System.IO.Path]::GetFileName($wasmPath))"
}
