# Building the native companion

Requires Visual Studio 2022 Build Tools with the **Desktop development with C++** workload, v143 toolset, Windows SDK 10.0.26100.0, and the .NET Framework MSBuild/XAML build tasks. A .NET SDK is not required.

Build x64 Release from the repository root:

```powershell
& 'D:\program\vs-buildtools\2022\MSBuild\Current\Bin\MSBuild.exe' native-ui\StudioUI.vcxproj /restore /p:RestoreSources=https://api.nuget.org/v3/index.json /p:Configuration=Release /p:Platform=x64
```

The executable is `native-ui\x64\Release\StudioUI.exe`. It is unpackaged and expects `--pipe <full-pipe-name>` from the Rust launcher. Without the launcher it displays a visible connection error; it performs no fake lifecycle actions.

Windows App SDK 1.8.260921001 and CppWinRT 2.0.250303.1 restore from NuGet. Bootstrapper and deployment initialization are intentionally not called.

## Embedded runtime layout

`portable-files.txt` is the pinned payload allowlist. Each non-comment line names one exact file relative to `native-ui\x64\Release`; it includes the executable, compiled XAML resources, self-contained Windows App SDK runtime, required native libraries, and locale resources. It excludes PDB, LIB, EXP, WinMD metadata, and design-time binaries. The Rust build embeds those files under the same relative names. Do not replace this list with a directory-copy instruction or wildcard directory pattern.

At runtime, the launcher extracts the embedded tree to `%LOCALAPPDATA%\Unsloth Studio Desktop\native\<payload-sha256>\`. `StudioUI.exe` stays at that directory root; locale and resource files keep their listed subdirectories. The selected product root remains separate: `<parent>\Unsloth Studio Desktop\app` contains the managed Studio installation. The temporary extraction under Local AppData is required before the user chooses a product root.

After changing Windows App SDK or CppWinRT versions, rebuild Release and review every runtime dependency change against `portable-files.txt`. Release packaging must fail if any listed path is missing.
