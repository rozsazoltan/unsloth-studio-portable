# Launcher UI contract v1 (Tauri)

Supersedes `docs/ipc-v1.md`. The native C++/WinUI companion, its named-pipe transport and the embedded
payload are removed. Rust still owns settings, lifecycle, Windows integration and destructive actions;
the UI is a SolidJS document rendered by the Tauri webview.

## Transport

- UI calls `invoke(command, args)`; Rust replies with JSON or an error `{ code, message }`.
- Rust pushes `state` events after every state change (and once at startup).
- Rust pushes `log` events for operation output, bounded and rate-limited.
- No other channel exists. The webview has no direct filesystem, process or network access beyond these
  commands; the capability set in `tauri.conf.json` stays minimal.

## Commands

| Command | Args | Returns |
| --- | --- | --- |
| `get_state` | — | `State` |
| `save_settings` | `settings: Settings` | `State` |
| `pick_directory` | `target: "root_parent" \| "hf_home"` | `string \| null` |
| `run_operation` | `kind: OperationKind`, `upstreamSetupApproved?: boolean` | `State` |
| `prepare_removal` | `scope: "runtime" \| "all_owned_data"` | `State` (with `removal`) |
| `confirm_removal` | `token: string` | `State` |
| `apply_mise_config` | `confirm_overwrite?: boolean` | `State` |
| `open_external` | `url: string` | — |
| `exit_app` | — | — |

`OperationKind` = `install` | `update` | `start` | `stop` | `open_studio` | `check_updates`.

`install` and `update` accept `upstreamSetupApproved`. Missing or false approval must be rejected before
any setup or filesystem change. Approval is per operation, never a saved setting, and does not establish
sandboxing. The UI asks for a separate informed confirmation each time.

## Types

```ts
type Phase = "Absent" | "Installing" | "Ready" | "Starting" | "Running" | "Stopping" | "Updating" | "Removing" | "Failed";
type UpdateStatus = "unknown" | "checking" | "current" | "available" | "error";

interface Settings {
  root: string;
  hf_home: string;
  tray_enabled: boolean;
  startup_enabled: boolean;
  desktop_shortcut: boolean;
  start_menu_shortcut: boolean;
  port: number;
}

interface AppError { code: string; message: string }

interface Removal {
  token: string;
  paths: string[];
  preserved_paths: string[];
  warning: string;
}

interface MiseConfig {
  path: string;                                        // <root>\mise.toml
  status: "missing" | "current" | "pending_update" | "user_modified";
  revision: number;                                    // revision the launcher ships
  local_revision: number | null;                       // revision found on disk, null when unknown
  diff: string | null;                                 // unified diff, only for pending_update
}

interface State {
  revision: number;
  settings: Settings;
  phase: Phase;
  operation: { id: string; kind: string; detail: string } | null;
  installed_commit: string | null;
  upstream_commit: string | null;
  update_status: UpdateStatus;
  studio_url: string | null;
  last_error: AppError | null;
  removal: Removal | null;
}

interface LogEvent { operationId: string; text: string }
```

## Behavior (unchanged from v1)

- One modifying operation at a time.
- `root` is the product root, not its parent. The UI picks a parent folder and appends
  `Unsloth Studio Desktop`; the HF path stays editable. Rust validates every path change and rejects
  overlaps with owned trees using physical (resolved) paths.
- A settings change must not silently move an installed product.
- Update/remove cannot run while Studio is active. Runtime removal preserves models and user data.
  All-owned-data removal stays restricted to verified launcher-owned paths; an external HF cache always
  survives. `prepare_removal` returns the exact deletion and preserved-path lists, and the UI shows them
  in a danger dialog. `confirm_removal` consumes a short-lived single-use token tied to the current
  settings and root; Rust revalidates ownership and reparse-point safety before deleting.
- Install/update confirmation discloses possible VC++ runtime installation, CMake installation via
  winget, user PATH changes and external WebView cache changes. Cancel sends no setup command.
- Managed configuration: the launcher owns `<root>\mise.toml` (revisioned marker in the header) and treats it
  as the dependency contract. `missing` means it must be written before install; `current` means it matches
  the shipped revision; `pending_update` means the shipped revision differs and `diff` holds a unified diff for
  review; `user_modified` means the file on disk carries no recognizable revision (or a foreign one) and may
  contain user edits. Only `apply_mise_config` writes the file (atomic replace) and then runs project-local
  `mise install` from the product root with the scoped `MISE_*` directories, never `-g` and never a global
  tool activation. `user_modified` requires `confirm_overwrite: true` after the diff is shown. `install` and
  `update` reject with `mise_config_pending` while the config is not `current`. The file survives uninstall and
  Studio removal, and is never part of a deletion list.
- Studio serves localhost only. Closing the window with tray enabled hides to tray; `exit_app` stops
  owned processes and exits.- About shows product version, license, source and acknowledgments. Experimental explains demo
  constraints and opt-in Windows integrations; never imply sandboxed installer behavior.
