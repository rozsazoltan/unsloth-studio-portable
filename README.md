> [!WARNING]
> This project is archived. The approach lives on, generalized, in [verzly/autark-marketplace](https://github.com/verzly/autark-marketplace), which provides the same portable, marketplace-driven install model for any app.

# Unsloth Studio Desktop

Portable Windows launcher and manager for Unsloth Studio. It manages dependencies through [mise-en-place](https://github.com/jdx/mise).

Manual Unsloth installation on Windows can be difficult. This launcher keeps managed tools in a project-local `mise.toml` and runs Studio inside that managed environment.

## Use

1. Download and run the portable executable from Releases.
2. Choose a product folder.
3. Install Studio, then start or stop it from the launcher.
4. Use launcher controls to update or uninstall Studio.

Studio opens at `http://127.0.0.1:<port>`. The managed `mise.toml` lives at the product root, next to `app`. Windows WebView2 Runtime is required.

If `mise` is not available, the launcher downloads the official Windows release into `<root>\runtime\bin\mise.exe` and uses that copy by absolute path. The local copy is never added to `PATH`, so it cannot be invoked globally, and `mise self-update` refreshes only the launcher-managed binary. A system `mise` on `PATH`, or one pointed to by `MISE_BIN`, is preferred and left untouched.

## Local development

Install the Rust Windows build prerequisites and pnpm, then run:

```powershell
pnpm --dir ui install
pnpm --dir ui build
cargo run
```

`pnpm --dir ui install` needs the esbuild build-script allowlist in `ui/pnpm-workspace.yaml`.

Contributions are welcome as issues and pull requests. Keep the launcher Windows-only, and do not add tests without asking first.

## Releases

Releases use CalVer tags `vYYYY.M.N`, with an unpadded month and a counter starting at zero each month. First release: `v2026.10.0`.

## License and acknowledgments

Launcher code is licensed under [AGPL-3.0-or-later](LICENSE). Thanks to [Unsloth Studio](https://github.com/unslothai/unsloth) and [mise-en-place](https://github.com/jdx/mise).
