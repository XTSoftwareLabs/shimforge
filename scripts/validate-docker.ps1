# Builds the Linux test image and runs the checks.
[CmdletBinding()]
param([switch] $SkipBuild)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$RepositoryPath = Split-Path -Parent $PSScriptRoot
$ValidationImage = 'shimforge-test:local'

if (-not $SkipBuild) {
    & docker build --platform linux/amd64 --tag $ValidationImage $PSScriptRoot
    if ($LASTEXITCODE -ne 0) {
        throw "Building the validation image failed with exit code $LASTEXITCODE."
    }
}

$DockerArguments = @(
    'run', '--rm', '--platform', 'linux/amd64',
    '--mount', "type=bind,source=$RepositoryPath,target=/work",
    '--mount', 'type=volume,source=shimforge-target-linux,target=/target',
    '--mount', 'type=volume,source=shimforge-cargo-registry-linux,target=/usr/local/cargo/registry',
    '--mount', 'type=volume,source=shimforge-cargo-git-linux,target=/usr/local/cargo/git',
    $ValidationImage
)
& docker @DockerArguments
if ($LASTEXITCODE -ne 0) {
    throw "Linux validation failed with exit code $LASTEXITCODE."
}
