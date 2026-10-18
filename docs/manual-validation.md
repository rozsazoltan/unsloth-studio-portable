# Manual demo validation

This checklist is a verification plan, not evidence of completed scenarios. No automated tests are added. Record actual outcomes and limitations when exercising a built demo.

## Safe baseline

Use a new empty product root, never an existing installation. Do not run real Studio setup until separately authorized. Installation can download large dependencies and modify host prerequisites. GPU behavior depends on the host.

1. Build using `pwsh -NoProfile -File scripts/build-release.ps1`. Confirm the resulting executable contains its native payload and does not rely on native build output after distribution.
2. Launch from CMD and Explorer. Visit Dashboard, Settings, About and Experimental. Check narrow-window layouts, keyboard navigation and visible operation errors.
3. Close to tray, reopen, and invoke a second instance. Confirm the original instance activates. Explicit Exit must stop only the launcher's owned child processes.
4. Select a parent with spaces and Unicode characters. Confirm the product root ends in `Unsloth Studio Desktop` and the default Hugging Face path ends in `huggingface`.
5. Set an external Hugging Face cache. Confirm saving settings does not create machine-wide environment variables or change an unrelated installation.
6. Check updates manually; verify the startup check is not repeated by normal navigation. A network error must remain visible without preventing use of the UI.

## Explicitly authorized installation

1. Install and update each require a separate clear setup-side-effect confirmation. Cancel must perform no setup. Confirm explains possible VC++ runtime/CMake installation, user PATH changes and external WebView cache effects.
2. After explicit authorization, install into the new managed root. Inspect the actual source/runtime and mise paths. Verify the launch environment directs HF data to the configured cache.
3. Start Studio on localhost, then stop it. Occupy the configured port with an unrelated server; the launcher must report a conflict, not present that server as its own Studio instance.
4. Update only while stopped. Interrupted setup must show an error; do not imply transactional rollback.

## Removal and Windows integration

1. Prepare runtime removal. Inspect the exact delete/preserve list, cancel, and confirm nothing is deleted.
2. Confirm runtime removal. Models, user data and every external cache must survive.
3. Prepare all-owned-data removal. Only verified launcher-owned paths may be deleted; an external Hugging Face cache must still survive.
4. Verify changed settings invalidate removal approval and expired/reused tokens cannot delete anything.
5. Reject drive roots, system/profile roots, unsafe overlapping cache paths, junctions and other reparse-point traversal.
6. Enable startup and shortcuts only on explicit request. Disable them and confirm unrelated startup entries/shortcuts remain untouched.

## Evidence boundaries

Cargo format/check/clippy establish source consistency and compilation, not native runtime behavior. A successful native/release build establishes packaging, not GPU support or successful Studio setup. Real Studio installation, GPU inference/training, startup behavior and published bin integration remain unverified until actually exercised.
