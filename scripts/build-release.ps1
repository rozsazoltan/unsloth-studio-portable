$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$nativeBuild = Join-Path $PSScriptRoot 'build-native.ps1'
& $nativeBuild -RepoRoot $repo
$payload = Join-Path $repo 'native-ui\x64\Release'
$env:UNSLOTH_NATIVE_PAYLOAD_DIR = $payload
Push-Location $repo
try {
    cargo build --release
    if ($LASTEXITCODE -ne 0) { throw "Cargo release build failed with exit code $LASTEXITCODE" }
    $dist = Join-Path $repo 'dist'
    New-Item -ItemType Directory -Force -Path $dist | Out-Null
    Copy-Item (Join-Path $repo 'target\release\unsloth-studio-portable.exe') (Join-Path $dist 'unsloth-studio-portable-windows-x64.exe') -Force
} finally {
    Pop-Location
    Remove-Item Env:UNSLOTH_NATIVE_PAYLOAD_DIR -ErrorAction SilentlyContinue
}
