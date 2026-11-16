mod lifecycle;
mod settings;
use anyhow::Result;
use lifecycle::{Phase, Shared, State};
use serde::Serialize;
use settings::Settings;
use std::sync::{Arc, Mutex};
use tauri::{Emitter, Manager};

#[derive(Debug, Serialize)]
struct CmdError {
    code: String,
    message: String,
}
impl From<anyhow::Error> for CmdError {
    fn from(e: anyhow::Error) -> Self {
        let m = e.to_string();
        let code = match m.split(':').next().unwrap_or_default() {
            "operation_in_progress"
            | "approval_required"
            | "confirm_required"
            | "invalid_argument" => m.split(':').next().unwrap_or_default().to_string(),
            _ => "internal".to_string(),
        };
        Self { code, message: m }
    }
}
type CmdResult<T> = Result<T, CmdError>;

fn studio_data_path(shared: &Shared) -> std::path::PathBuf {
    std::path::Path::new(&shared.state.lock().unwrap().settings.root).join("app")
}

fn is_runtime_entry(name: &str) -> bool {
    matches!(
        name,
        "unsloth_studio"
            | "python"
            | "uv"
            | "source"
            | "bin"
            | "share"
            | "llama.cpp"
            | "whisper.cpp"
            | "audio.cpp"
    ) || name.starts_with(".venv_t5")
}

fn data_entries(root: &std::path::Path) -> anyhow::Result<Vec<std::ffi::OsString>> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    entries
        .map(|entry| {
            let entry = entry?;
            Ok(
                (!is_runtime_entry(&entry.file_name().to_string_lossy()))
                    .then(|| entry.file_name()),
            )
        })
        .filter_map(|entry| entry.transpose())
        .collect()
}

fn run_powershell(script: &str) -> anyhow::Result<()> {
    let status = std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", script])
        .status()?;
    anyhow::ensure!(status.success(), "PowerShell archive operation failed");
    Ok(())
}

fn copy_entry(source: &std::path::Path, destination: &std::path::Path) -> anyhow::Result<()> {
    anyhow::ensure!(
        !std::fs::symlink_metadata(source)?.file_type().is_symlink(),
        "symbolic links are not supported in Studio data archives"
    );
    if std::fs::symlink_metadata(destination)
        .is_ok_and(|metadata| metadata.file_type().is_symlink())
    {
        anyhow::bail!("Studio data destination contains a symbolic link");
    }
    if source.is_dir() {
        std::fs::create_dir_all(destination)?;
        for entry in std::fs::read_dir(source)? {
            let entry = entry?;
            copy_entry(&entry.path(), &destination.join(entry.file_name()))?;
        }
    } else if source.is_file() {
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(source, destination)?;
    }
    Ok(())
}

fn remove_database_sidecars(root: &std::path::Path) -> anyhow::Result<()> {
    fn visit(path: &std::path::Path) -> anyhow::Result<()> {
        let metadata = std::fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() {
            return Ok(());
        } else if metadata.is_dir() {
            for entry in std::fs::read_dir(path)? {
                visit(&entry?.path())?;
            }
        } else if path.extension().is_some_and(|ext| ext == "db") {
            for suffix in ["-wal", "-shm"] {
                match std::fs::remove_file(path.with_file_name(format!(
                    "{}{suffix}",
                    path.file_name().unwrap().to_string_lossy()
                ))) {
                    Ok(()) => (),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                    Err(error) => return Err(error.into()),
                }
            }
        }
        Ok(())
    }
    visit(root)
}

fn temp_folder() -> anyhow::Result<std::path::PathBuf> {
    let path = std::env::temp_dir().join(format!("unsloth-account-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&path)?;
    Ok(path)
}

#[tauri::command(rename_all = "snake_case")]
fn backup_account(
    _app: tauri::AppHandle,
    s: tauri::State<'_, Arc<Shared>>,
) -> CmdResult<Option<String>> {
    if s.operation.lock().unwrap().is_some() {
        return Err(CmdError { code: "operation_in_progress".into(), message: "The account cannot be backed up while an operation is running. Wait for it to finish and try again.".into() });
    }
    let Some(path) = rfd::FileDialog::new()
        .set_file_name("unsloth-studio-settings.zip")
        .add_filter("Zip archive", &["zip"])
        .save_file()
    else {
        return Ok(None);
    };
    let temp = temp_folder()?;
    let result = (|| -> anyhow::Result<()> {
        let data = studio_data_path(&s);
        anyhow::ensure!(
            data.join("studio.db").is_file(),
            "invalid_argument:The Studio data folder has no studio.db"
        );
        for entry in data_entries(&data)? {
            copy_entry(&data.join(&entry), &temp.join(&entry))?;
        }
        let source = temp.join("*").to_string_lossy().replace('\'', "''");
        let destination = path.to_string_lossy().replace('\'', "''");
        run_powershell(&format!(
            "Compress-Archive -Path '{source}' -DestinationPath '{destination}' -Force"
        ))
    })();
    let cleanup = std::fs::remove_dir_all(&temp);
    result?;
    cleanup.map_err(anyhow::Error::from)?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

#[tauri::command(rename_all = "snake_case")]
fn restore_account(
    _app: tauri::AppHandle,
    s: tauri::State<'_, Arc<Shared>>,
) -> CmdResult<Option<String>> {
    if s.operation.lock().unwrap().is_some() {
        return Err(CmdError { code: "operation_in_progress".into(), message: "The account cannot be restored while an operation is running. Wait for it to finish and try again.".into() });
    }
    if matches!(
        s.state.lock().unwrap().phase,
        Phase::Starting | Phase::Running | Phase::Stopping
    ) {
        return Err(CmdError {
            code: "invalid_argument".into(),
            message: "stop Studio before restoring the data".into(),
        });
    }
    let Some(path) = rfd::FileDialog::new()
        .add_filter("Zip archive", &["zip"])
        .add_filter("SQLite database", &["db"])
        .pick_file()
    else {
        return Ok(None);
    };
    let data = studio_data_path(&s);
    if path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
    {
        let temp = temp_folder()?;
        let result = (|| -> anyhow::Result<()> {
            let source = path.to_string_lossy().replace('\'', "''");
            let destination = temp.to_string_lossy().replace('\'', "''");
            run_powershell(&format!(
                "Expand-Archive -LiteralPath '{source}' -DestinationPath '{destination}' -Force"
            ))?;
            let source_root = if temp.join("studio.db").is_file() {
                temp.clone()
            } else {
                let mut roots = std::fs::read_dir(&temp)?
                    .filter_map(|entry| entry.ok())
                    .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
                    .map(|entry| entry.path())
                    .filter(|path| path.join("studio.db").is_file());
                let Some(root) = roots.next() else {
                    anyhow::bail!(
                        "invalid_argument:the archive does not look like a Studio data backup"
                    );
                };
                anyhow::ensure!(
                    roots.next().is_none(),
                    "invalid_argument:the archive does not look like a Studio data backup"
                );
                root
            };
            std::fs::create_dir_all(&data)?;
            let source_root = std::fs::canonicalize(&source_root)?;
            for entry in data_entries(&source_root)? {
                let source = source_root.join(&entry);
                anyhow::ensure!(
                    std::fs::canonicalize(&source)?.starts_with(&source_root),
                    "archive entry escaped temporary folder"
                );
                copy_entry(&source, &data.join(entry))?;
            }
            remove_database_sidecars(&data)?;
            Ok(())
        })();
        let cleanup = std::fs::remove_dir_all(&temp);
        result?;
        cleanup.map_err(anyhow::Error::from)?;
    } else {
        let target = data.join("auth/auth.db");
        std::fs::create_dir_all(target.parent().unwrap()).map_err(anyhow::Error::from)?;
        std::fs::copy(&path, &target).map_err(anyhow::Error::from)?;
        for suffix in ["-wal", "-shm"] {
            match std::fs::remove_file(target.with_file_name(format!("auth.db{suffix}"))) {
                Ok(()) => (),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                Err(error) => return Err(anyhow::Error::from(error).into()),
            }
        }
    }
    Ok(Some(path.to_string_lossy().into_owned()))
}

#[tauri::command(rename_all = "snake_case")]
fn delete_account(confirm: String, s: tauri::State<'_, Arc<Shared>>) -> CmdResult<State> {
    if confirm != "REMOVE" {
        return Err(CmdError {
            code: "confirm_required".into(),
            message: "type REMOVE to confirm".into(),
        });
    }
    if s.operation.lock().unwrap().is_some() {
        return Err(CmdError { code: "operation_in_progress".into(), message: "The account cannot be deleted while an operation is running. Wait for it to finish and try again.".into() });
    }
    if matches!(
        s.state.lock().unwrap().phase,
        Phase::Starting | Phase::Running | Phase::Stopping
    ) {
        return Err(CmdError {
            code: "invalid_argument".into(),
            message: "stop Studio before deleting the account".into(),
        });
    }
    let data = studio_data_path(&s);
    for entry in data_entries(&data)? {
        let path = data.join(entry);
        let result = if std::fs::symlink_metadata(&path)
            .is_ok_and(|metadata| metadata.file_type().is_dir())
        {
            std::fs::remove_dir_all(path)
        } else {
            std::fs::remove_file(path)
        };
        match result {
            Ok(()) => (),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(anyhow::Error::from(error).into()),
        }
    }
    Ok(s.state.lock().unwrap().clone())
}

#[tauri::command(rename_all = "snake_case")]
fn get_state(s: tauri::State<'_, Arc<Shared>>) -> CmdResult<State> {
    Ok(s.state.lock().unwrap().clone())
}

#[tauri::command(rename_all = "snake_case")]
fn save_settings(
    mut settings: Settings,
    move_folder: bool,
    s: tauri::State<'_, Arc<Shared>>,
) -> CmdResult<State> {
    if settings.root.trim().is_empty() || settings.hf_home.trim().is_empty() {
        return Err(CmdError {
            code: "invalid_argument".into(),
            message: "root and hugging face folder are required".into(),
        });
    }
    // Moving the product folder while an operation owns it would break the
    // running child, so only a changed root or name is rejected here.
    {
        let current = s.state.lock().unwrap();
        let folder_changed = settings.root != current.settings.root
            || settings.normalized_folder_name() != current.settings.normalized_folder_name();
        drop(current);
        if folder_changed && s.operation.lock().unwrap().is_some() {
            return Err(CmdError {
                code: "operation_in_progress".into(),
                message: "The install folder cannot be changed while an operation is running. Wait for it to finish and try again.".into(),
            });
        }
    }
    settings.apply_folder_name(move_folder)?;
    settings.save()?;
    Ok(apply_settings(&s, settings))
}

fn apply_settings(s: &Shared, settings: Settings) -> State {
    let mut st = s.state.lock().unwrap();
    st.settings = settings;
    let (hf_env_var, hf_home) = lifecycle::effective_hf_home(&st.settings.hf_home);
    st.settings.hf_home = hf_home;
    st.hf_env_var = hf_env_var;
    st.mise_config_path = std::path::Path::new(&st.settings.root)
        .join("mise.toml")
        .to_string_lossy()
        .into();
    // Any settings change can move the product root, so re-check the install
    // state. Only live operations and a running Studio keep their phase.
    if !matches!(
        st.phase,
        lifecycle::Phase::Installing
            | lifecycle::Phase::Starting
            | lifecycle::Phase::Running
            | lifecycle::Phase::Stopping
            | lifecycle::Phase::Removing
    ) {
        st.phase = if lifecycle::is_installed(&st.settings) {
            lifecycle::Phase::Installed
        } else {
            lifecycle::Phase::Absent
        };
    }
    match settings::settings_path() {
        Ok(path) => {
            st.settings_exists = path.is_file();
            st.settings_path = path.to_string_lossy().into_owned();
        }
        Err(_) => {
            st.settings_exists = false;
            st.settings_path = String::new();
        }
    }
    st.clone()
}

#[tauri::command(rename_all = "snake_case")]
fn find_legacy_settings() -> CmdResult<Option<String>> {
    let active = settings::settings_path()?;
    Ok(settings::legacy_path()
        .filter(|path| path != &active)
        .map(|path| path.to_string_lossy().into_owned()))
}

#[tauri::command(rename_all = "snake_case")]
fn migrate_settings(s: tauri::State<'_, Arc<Shared>>) -> CmdResult<State> {
    if s.operation.lock().unwrap().is_some() {
        return Err(CmdError {
            code: "operation_in_progress".into(),
            message: "The settings file cannot be migrated while an operation is running. Wait for it to finish and try again.".into(),
        });
    }
    let legacy = settings::legacy_path().ok_or_else(|| CmdError {
        code: "invalid_argument".into(),
        message: "No settings file was found in the default location.".into(),
    })?;
    let target = settings::settings_path()?;
    if target.exists() {
        return Err(CmdError {
            code: "invalid_argument".into(),
            message: format!(
                "A settings file already exists at {}. It was not overwritten.",
                target.display()
            ),
        });
    }
    std::fs::create_dir_all(target.parent().ok_or_else(|| CmdError {
        code: "internal".into(),
        message: "settings path has no parent".into(),
    })?)
    .map_err(anyhow::Error::from)?;
    if let Err(rename_error) = std::fs::rename(&legacy, &target) {
        if let Err(copy_error) = std::fs::copy(&legacy, &target) {
            return Err(CmdError {
                code: "internal".into(),
                message: format!("{rename_error}; fallback copy failed: {copy_error}"),
            });
        }
        std::fs::remove_file(&legacy).map_err(anyhow::Error::from)?;
    }
    let settings = Settings::load_or_default()?;
    Ok(apply_settings(&s, settings))
}

#[tauri::command(rename_all = "snake_case")]
fn export_settings(s: tauri::State<'_, Arc<Shared>>) -> CmdResult<Option<String>> {
    if s.operation.lock().unwrap().is_some() {
        return Err(CmdError {
            code: "operation_in_progress".into(),
            message: "Settings cannot be exported while an operation is running. Wait for it to finish and try again.".into(),
        });
    }
    let Some(path) = rfd::FileDialog::new()
        .set_file_name("launcher-settings.json")
        .add_filter("JSON", &["json"])
        .save_file()
    else {
        return Ok(None);
    };
    let settings = s.state.lock().unwrap().settings.clone();
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&settings).map_err(anyhow::Error::from)?,
    )
    .map_err(anyhow::Error::from)?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

#[tauri::command(rename_all = "snake_case")]
fn import_settings(s: tauri::State<'_, Arc<Shared>>) -> CmdResult<Option<State>> {
    if s.operation.lock().unwrap().is_some() {
        return Err(CmdError {
            code: "operation_in_progress".into(),
            message: "Settings cannot be imported while an operation is running. Wait for it to finish and try again.".into(),
        });
    }
    let Some(path) = rfd::FileDialog::new()
        .add_filter("JSON", &["json"])
        .pick_file()
    else {
        return Ok(None);
    };
    let settings: Settings =
        serde_json::from_slice(&std::fs::read(path).map_err(anyhow::Error::from)?)
            .map_err(anyhow::Error::from)?;
    if settings.root.trim().is_empty() || settings.hf_home.trim().is_empty() {
        return Err(CmdError {
            code: "invalid_argument".into(),
            message: "root and hugging face folder are required".into(),
        });
    }
    settings.save()?;
    Ok(Some(apply_settings(&s, settings)))
}

#[tauri::command(rename_all = "snake_case")]
fn pick_directory(target: String) -> CmdResult<Option<String>> {
    if !matches!(target.as_str(), "root_parent" | "hf_home") {
        return Err(CmdError {
            code: "invalid_argument".into(),
            message: "invalid directory target".into(),
        });
    }
    Ok(rfd::FileDialog::new()
        .pick_folder()
        .map(|p| p.to_string_lossy().into()))
}

#[tauri::command(rename_all = "snake_case")]
async fn install(
    approve_effects: bool,
    app: tauri::AppHandle,
    s: tauri::State<'_, Arc<Shared>>,
) -> CmdResult<State> {
    if !approve_effects {
        return Err(CmdError {
            code: "approval_required".into(),
            message: "approval required".into(),
        });
    }
    lifecycle::install(app, s.inner().clone())
        .await
        .map_err(Into::into)
}

#[tauri::command(rename = "cancel", rename_all = "snake_case")]
fn cancel_install(s: tauri::State<'_, Arc<Shared>>) -> CmdResult<State> {
    lifecycle::cancel(&s)?;
    Ok(s.state.lock().unwrap().clone())
}

#[tauri::command(rename_all = "snake_case")]
fn start(app: tauri::AppHandle, s: tauri::State<'_, Arc<Shared>>) -> CmdResult<State> {
    lifecycle::launch(&app, s.inner()).map_err(Into::into)
}

#[tauri::command(rename_all = "snake_case")]
fn stop(app: tauri::AppHandle, s: tauri::State<'_, Arc<Shared>>) -> CmdResult<State> {
    lifecycle::stop(&app, &s).map_err(Into::into)
}

#[tauri::command(rename_all = "snake_case")]
async fn remove(
    confirm: String,
    app: tauri::AppHandle,
    s: tauri::State<'_, Arc<Shared>>,
) -> CmdResult<State> {
    if confirm != "REMOVE" {
        return Err(CmdError {
            code: "confirm_required".into(),
            message: "type REMOVE to confirm".into(),
        });
    }
    // Deleting a full install takes seconds to minutes; keep it off the event
    // thread so the window stays responsive.
    let shared = s.inner().clone();
    tauri::async_runtime::spawn_blocking(move || lifecycle::remove(&app, &shared))
        .await
        .map_err(|error| CmdError {
            code: "internal".into(),
            message: error.to_string(),
        })?
        .map_err(Into::into)
}

#[tauri::command(rename_all = "snake_case")]
fn open_studio(app: tauri::AppHandle, s: tauri::State<'_, Arc<Shared>>) -> CmdResult<State> {
    let running_url = {
        let st = s.state.lock().unwrap();
        if matches!(st.phase, Phase::Running) {
            st.studio_url.clone()
        } else {
            None
        }
    };
    let state = match running_url {
        Some(_) => s.state.lock().unwrap().clone(),
        None => lifecycle::launch(&app, s.inner())?,
    };
    if let Some(url) = state.studio_url.clone() {
        tauri_plugin_opener::open_url(url, None::<&str>).map_err(|e| CmdError {
            code: "internal".into(),
            message: e.to_string(),
        })?;
    }
    let _ = app.emit("state", &state);
    Ok(state)
}

#[allow(clippy::needless_pass_by_value)]
#[tauri::command(rename_all = "snake_case")]
fn reveal_path(target: String, s: tauri::State<'_, Arc<Shared>>) -> CmdResult<()> {
    let st = s.state.lock().unwrap();
    let path = match target.as_str() {
        "root" => std::path::PathBuf::from(&st.settings.root),
        "mise_config" => std::path::PathBuf::from(&st.mise_config_path),
        "hf_home" => std::path::PathBuf::from(&st.settings.hf_home),
        "settings" => crate::settings::settings_path().map_err(|e| CmdError {
            code: "internal".into(),
            message: e.to_string(),
        })?,
        "legacy_settings" => crate::settings::legacy_path().ok_or_else(|| CmdError {
            code: "invalid_argument".into(),
            message: "no legacy settings file".into(),
        })?,
        _ => {
            return Err(CmdError {
                code: "invalid_argument".into(),
                message: "invalid reveal target".into(),
            })
        }
    };
    let exists = path.exists();
    let result = if exists && path.is_file() {
        tauri_plugin_opener::reveal_item_in_dir(&path)
    } else {
        let folder = if exists {
            path.clone()
        } else {
            path.parent()
                .map(std::path::Path::to_path_buf)
                .filter(|p| p.exists())
                .ok_or_else(|| anyhow::anyhow!("path does not exist"))?
        };
        tauri_plugin_opener::open_path(folder, None::<&str>)
    };
    result.map_err(|e| CmdError {
        code: "internal".into(),
        message: e.to_string(),
    })?;
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
fn open_url(url: String) -> CmdResult<()> {
    if !url.starts_with("https://") {
        return Err(CmdError {
            code: "invalid_argument".into(),
            message: "only https links are allowed".into(),
        });
    }
    tauri_plugin_opener::open_url(url, None::<&str>).map_err(|e| CmdError {
        code: "internal".into(),
        message: e.to_string(),
    })?;
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
fn probe_studio(s: tauri::State<'_, Arc<Shared>>) -> CmdResult<bool> {
    let port = s.state.lock().unwrap().settings.port;
    // shortcut: plain TCP reachability check, not an HTTP status check; add an HTTP GET only if a 404 must be distinguished from a dead process.
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    Ok(std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(800)).is_ok())
}

#[tauri::command(rename_all = "snake_case")]
fn exit_app(app: tauri::AppHandle, s: tauri::State<'_, Arc<Shared>>) -> CmdResult<()> {
    let _ = lifecycle::stop_child(&s);
    app.exit(0);
    Ok(())
}

fn main() {
    let settings = Settings::load_or_default().unwrap_or_default();
    let state = Arc::new(Shared {
        state: Mutex::new(State::new(settings)),
        operation: Mutex::new(None),
        child: Mutex::new(None),
    });
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            get_state,
            save_settings,
            find_legacy_settings,
            migrate_settings,
            export_settings,
            import_settings,
            backup_account,
            restore_account,
            delete_account,
            pick_directory,
            install,
            cancel_install,
            start,
            stop,
            remove,
            open_studio,
            reveal_path,
            open_url,
            probe_studio,
            exit_app
        ])
        .setup(|app| {
            let shared = app.state::<Arc<Shared>>().inner().clone();
            if let Some(w) = app.get_webview_window("main") {
                let handle = app.handle().clone();
                let owned = shared.clone();
                w.on_window_event(move |e| {
                    if let tauri::WindowEvent::CloseRequested { .. } = e {
                        let _ = lifecycle::stop_child(&owned);
                        handle.exit(0)
                    }
                });
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("Tauri application failed");
}
