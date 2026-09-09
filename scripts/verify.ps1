# Requires rustfmt, clippy, llvm-tools-preview, and cargo-llvm-cov 0.8.7.
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Invoke-Cargo {
    param([Parameter(Mandatory = $true)][string[]] $CargoArguments)

    & cargo @CargoArguments
    if ($LASTEXITCODE -ne 0) {
        throw "cargo $($CargoArguments -join ' ') failed with exit code $LASTEXITCODE."
    }
}

Push-Location (Split-Path -Parent $PSScriptRoot)
try {
    Invoke-Cargo -CargoArguments @('fmt', '--all', '--check')
    Invoke-Cargo -CargoArguments @('clippy', '--all-targets', '--locked', '--', '-D', 'warnings')
    Invoke-Cargo -CargoArguments @('test', '--locked', '--', '--test-threads=1')

    New-Item -ItemType Directory -Path coverage/windows -Force | Out-Null
    Invoke-Cargo -CargoArguments @(
        'llvm-cov', '--all-targets', '--locked',
        '--ignore-filename-regex', '(^|[/\\])tests([/\\]|\.rs$)',
        '--fail-under-lines', '100', '--fail-under-functions', '100',
        '--lcov', '--output-path', 'coverage/windows/lcov.info',
        '--', '--test-threads=1'
    )
}
finally {
    Pop-Location
}
