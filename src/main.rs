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
    Ok(st.clone())
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
