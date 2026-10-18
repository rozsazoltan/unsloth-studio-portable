$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
if (-not (Test-Path $vswhere)) { throw "vswhere.exe not found at $vswhere. Install Visual Studio 2022 Build Tools with C++ workload." }
$msbuild = & $vswhere -latest -products '*' -requires Microsoft.Component.MSBuild -find 'MSBuild\Current\Bin\MSBuild.exe' | Select-Object -First 1
if (-not $msbuild -or -not (Test-Path $msbuild)) { throw 'Framework MSBuild not found. Install Visual Studio 2022 Build Tools and .NET Framework XAML build tasks.' }
$project = Join-Path $repo 'native-ui\StudioUI.vcxproj'
& $msbuild $project /restore /m /p:Configuration=Release /p:Platform=x64
if ($LASTEXITCODE -ne 0) { throw "WinUI build failed with exit code $LASTEXITCODE" }
$payload = Join-Path $repo 'native-ui\x64\Release'
if (-not (Test-Path (Join-Path $payload 'StudioUI.exe'))) { throw "StudioUI.exe missing from $payload" }
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
