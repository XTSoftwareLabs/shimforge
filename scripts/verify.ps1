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
    Invoke-Cargo -CargoArguments @('clippy', '--workspace', '--all-targets', '--locked', '--', '-D', 'warnings')
    Invoke-Cargo -CargoArguments @('test', '--workspace', '--locked', '--', '--test-threads=1')
    Invoke-Cargo -CargoArguments @('test', '--test', 'parallel', '--locked', '--', '--test-threads=8')

    # On Windows ARM64, llvm-profdata rejects the profiles stable Rust writes
    # ("malformed instrumentation profile data: symbol name is empty"), even for a
    # new crate with no dependencies. Every test above still runs there; only the
    # coverage gate is skipped until the toolchain can merge its own profiles.
    $hostTriple = (& rustc -vV | Select-String '^host: ').Line.Substring(6)
    if ($hostTriple -eq 'aarch64-pc-windows-msvc') {
        $message = 'Coverage skipped on aarch64-pc-windows-msvc: the toolchain cannot merge instrumented profiles.'
        if ($env:GITHUB_ACTIONS -eq 'true') {
            Write-Output "::warning::$message"
        } else {
            Write-Warning $message
        }
        return
    }

    New-Item -ItemType Directory -Path coverage/windows -Force | Out-Null
    Invoke-Cargo -CargoArguments @(
        'llvm-cov', '--workspace', '--all-targets', '--locked',
        '--ignore-filename-regex', '(^|[/\\])tests([/\\]|\.rs$)',
        '--fail-under-lines', '100', '--fail-under-functions', '100',
        '--lcov', '--output-path', 'coverage/windows/lcov.info',
        '--', '--test-threads=1'
    )
}
finally {
    Pop-Location
}
