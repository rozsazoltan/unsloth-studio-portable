# Unsloth Studio Portable

A Windows-only Rust launcher with an embedded native WinUI 3 interface for managing Unsloth Studio through [mise](https://mise.jdx.dev/). Keep the app, managed tools and model cache together without manually maintaining installation commands.

**Demo in development.** A successful build does not establish that Studio installation, GPU support or Windows integrations have been manually verified. See [manual validation](docs/manual-validation.md).

## Use

Run `unsloth-studio-portable-windows-x64.exe` from Explorer or CMD. Select an installation parent, such as `D:\program`; the product folder is `Unsloth Studio Desktop`:

```text
Unsloth Studio Desktop/
  app/           Studio source and runtime
  huggingface/   Default model cache
```

Change the Hugging Face location in Settings to reuse an external cache. Environment variables apply to launched processes, not machine-wide settings. External model caches must survive removal.

Use the interface to install, launch, stop, update or remove Studio. Startup and shortcut creation are opt-in. Tray mode keeps the launcher available after closing its window. Update checks run once per launcher start and on request; updates require an explicit action.

The launcher is distributed as one executable; it extracts bundled WinUI files to run. Some bootstrap configuration/UI cache files must exist outside the selected product folder. mise must be available. Compatible GPU drivers and other upstream prerequisites may be required.

**Installation is not a sandbox.** Unsloth setup may install VC++ runtime or CMake, change user `PATH`, and touch an external WebView cache. Install/update must ask for separate informed approval. Removing the managed app does not undo these host changes. Do not use an existing unrelated Studio directory as the managed root.

Release distribution targets [marcosnils/bin](https://github.com/marcosnils/bin). The project has not yet verified published-release asset selection; no working `bin install` release is claimed.

## Development and contribution

Install Rust's Windows MSVC toolchain, VS 2022 C++ Build Tools, Windows SDK 10.0.26100.0, and PowerShell 7. Native XAML compilation uses full .NET Framework MSBuild; a .NET SDK is not required. NuGet access is needed on the first native build.

```powershell
apm install
pwsh -NoProfile -File scripts/build-release.ps1
```

`apm install` installs the project's development agent dependencies, not the compiler or Studio. Keep changes focused, preserve native UI behavior, and include manual verification notes. This demo prioritizes manual validation; do not add tests without discussion.

## License and acknowledgments

Original launcher code: **AGPL-3.0-or-later**. You may redistribute and/or modify it under GNU AGPL version 3 or, at your option, any later version. Provided without warranty; see [LICENSE](LICENSE).

Native packaging and interaction patterns are informed by [quota-usage-monitor](https://github.com/rozsazoltan/quota-usage-monitor). Thanks to [Unsloth](https://github.com/unslothai/unsloth), mise, uv, Microsoft WinUI and marcosnils/bin. Upstream software and bundled dependencies retain their own licenses; this launcher does not relicense Unsloth Studio.
