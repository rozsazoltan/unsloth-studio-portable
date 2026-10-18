param(
    [Parameter(Mandatory = $true)]
    [string]$RepoRoot
)

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path -LiteralPath $RepoRoot).Path
$programFilesX86 = ${env:ProgramFiles(x86)}
if (-not $programFilesX86) {
    throw 'ProgramFiles(x86) is unavailable. Install Visual Studio 2022 Build Tools with the Desktop development with C++ workload.'
}
$vswhere = Join-Path $programFilesX86 'Microsoft Visual Studio\Installer\vswhere.exe'
if (-not (Test-Path -LiteralPath $vswhere)) {
    throw "vswhere.exe not found at $vswhere. Install Visual Studio 2022 Build Tools with the Desktop development with C++ workload."
}

$msbuild = & $vswhere -latest -products '*' -requires Microsoft.Component.MSBuild -find 'MSBuild\Current\Bin\MSBuild.exe' | Select-Object -First 1
if (-not $msbuild -or -not (Test-Path -LiteralPath $msbuild)) {
    throw 'Framework MSBuild not found. Install Visual Studio 2022 Build Tools, Windows SDK 10.0.26100.0, and .NET Framework XAML build tasks.'
}

$project = Join-Path $repo 'native-ui\StudioUI.vcxproj'
& $msbuild $project /restore /m /p:RestoreSources=https://api.nuget.org/v3/index.json /p:Configuration=Release /p:Platform=x64
if ($LASTEXITCODE -ne 0) {
    throw "WinUI Release build failed with exit code $LASTEXITCODE. Verify the C++ v143 toolset, Windows SDK 10.0.26100.0, and Framework MSBuild/XAML build tasks."
}

$payloadRoot = Join-Path $repo 'native-ui\x64\Release'
foreach ($required in @('StudioUI.exe', 'Microsoft.WindowsAppRuntime.Bootstrap.dll', 'Microsoft.ui.xaml.dll')) {
    if (-not (Test-Path -LiteralPath (Join-Path $payloadRoot $required) -PathType Leaf)) {
        throw "Native build completed without required payload file '$required' in '$payloadRoot'. Check the self-contained Windows App SDK project configuration."
    }
}

Write-Output "Native WinUI payload ready: $payloadRoot"
