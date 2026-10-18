# Native UI protocol v1

Rust owns settings, lifecycle, Windows integration, and destructive actions. Native UI is an original C++/WinRT WinUI 3 companion. Transport is a local Windows duplex named pipe. Rust starts the companion with `--pipe <full-pipe-name>`. A single companion connection carries requests and events. A client retries connection for at most ten seconds. Disconnect must not stop Studio. Runtime extraction and settings bootstrap may use a minimal local-app-data location before the product root is selected; disclose this exception.

## Framing and security
Each UTF-8 JSON message has a four-byte little-endian unsigned length prefix. Maximum payload: 1 MiB. Version must equal 1. Reject malformed frames, oversized payloads, unknown commands and remote pipe clients. Random pipe name, current-user-only DACL, and expected companion PID validation are required. Never put tokens or passwords in snapshots/logs. One serialized writer per connection. Dispatch UI changes onto DispatcherQueue, never block the UI thread on I/O.

## Requests
Envelope: `{"version":1,"id":1,"command":{"type":"get_snapshot"}}`.

Command types:
- `get_snapshot`
- `save_settings`: `settings` object below
- `install`, `update`: each accepts `upstream_setup_approved` boolean, default false. The UI sends true only after a separate per-operation informed confirmation. Missing/false approval must reject before setup or filesystem changes. Approval is not a saved setting and does not establish sandboxing.
- `start`, `stop`, `open_studio`, `check_updates`
- `prepare_removal`: `scope` = `runtime` or `all_owned_data`
- `confirm_removal`: `token` = exact token returned in snapshot
- `exit`

Settings keys, all required:
```json
{"root":"D:\\program\\Unsloth Studio Desktop","hf_home":"D:\\program\\Unsloth Studio Desktop\\huggingface","tray_enabled":true,"startup_enabled":false,"desktop_shortcut":false,"start_menu_shortcut":false,"port":8888}
```

`root` is the product root, not its parent. UI offers parent-folder selection and appends `Unsloth Studio Desktop`. HF path is editable. Rust validates all paths and changes. Defaults do not create shortcuts or startup entries. A settings change must not silently move an installed product. Studio serves localhost only.

## Events
Reply: `{"version":1,"type":"reply","id":1,"ok":true,"error":null}`. Error shape: `{"code":"stable_code","message":"Readable explanation"}`.

Snapshot: `{"version":1,"type":"snapshot","state":{...}}`.
State keys:
- `revision`: unsigned integer
- `settings`: complete Settings
- `phase`: `Absent`, `Installing`, `Ready`, `Starting`, `Running`, `Stopping`, `Updating`, `Removing`, or `Failed`
- `operation`: null or `{"id":"opaque","kind":"install","detail":"Readable status"}`
- `installed_commit`, `upstream_commit`: string or null
- `update_status`: `unknown`, `checking`, `current`, `available`, or `error`
- `studio_url`: string or null
- `last_error`: error object or null
- `removal`: null or `{"token":"opaque","paths":["..."],"preserved_paths":["..."],"warning":"..."}`

Log event: `{"version":1,"type":"log","operation_id":"opaque","text":"Readable line"}`. Bound retained logs and rate. Rust sends a snapshot on connect and after state changes. UI may poll get_snapshot once per second as a fallback; only one outstanding snapshot request.

## Behavior
One modifying operation at a time. Update/remove cannot run while Studio is active. Runtime removal preserves models and user data. All-owned-data removal remains restricted to verified launcher-owned paths; external HF cache always survives. Prepare returns exact deletion and preserved-path lists. Native UI displays these in a danger confirmation dialog. Confirm consumes a short-lived, single-use token tied to the current settings and root. Rust revalidates ownership and reparse-point safety before deletion.

Install/update confirmation discloses possible VC++ runtime installation, CMake installation through winget, user PATH changes and external WebView cache changes. Cancel sends no setup command. Each new install/update requires a new confirmation; an earlier approval does not carry forward.

About includes product version, license, source and acknowledgments. Experimental explains demo constraints and opt-in Windows integrations; never imply sandboxed installer behavior. Close UI to tray when tray is enabled; Exit command stops owned processes and exits core.
