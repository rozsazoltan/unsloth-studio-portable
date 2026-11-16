use crate::settings::Settings;
use anyhow::{bail, Context, Result};
use std::{
    fs,
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter};

static CANCEL: AtomicBool = AtomicBool::new(false);

pub struct Shared {
    pub state: std::sync::Mutex<State>,
    pub operation: std::sync::Mutex<Option<u64>>,
    pub child: std::sync::Mutex<Option<Child>>,
}
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Absent,
    Installing,
    Installed,
    Starting,
    Running,
    Stopping,
    Removing,
    Failed,
}
#[derive(Clone, serde::Serialize)]
pub struct ErrorInfo {
    pub code: String,
    pub message: String,
}
#[derive(Clone, serde::Serialize)]
pub struct Step {
    pub index: u32,
    pub total: u32,
    pub name: String,
}
impl ErrorInfo {
    pub fn from_error(e: &anyhow::Error) -> Self {
        let message = e.to_string();
        let code = match message.as_str() {
            "operation_in_progress"
            | "approval_required"
            | "confirm_required"
            | "invalid_argument" => message.clone(),
            _ => "internal".to_string(),
        };
        Self { code, message }
    }
}
#[derive(Clone, serde::Serialize)]
pub struct State {
    pub phase: Phase,
    pub settings: Settings,
    pub mise_config_path: String,
    pub studio_url: Option<String>,
    pub last_error: Option<ErrorInfo>,
    pub hf_env_var: Option<String>,
    pub step: Option<Step>,
    pub started_at: Option<u64>,
    pub step_started_at: Option<u64>,
    pub elapsed_secs: Option<u64>,
}
pub fn is_installed(settings: &Settings) -> bool {
    PathBuf::from(&settings.root)
        .join("app/unsloth_studio/Scripts/python.exe")
        .is_file()
}

impl State {
    pub fn new(settings: Settings) -> Self {
        let (hf_env_var, hf_home) = effective_hf_home(&settings.hf_home);
        let mut settings = settings;
        settings.hf_home = hf_home;
        let phase = if is_installed(&settings) {
            Phase::Installed
        } else {
            Phase::Absent
        };
        Self {
            mise_config_path: PathBuf::from(&settings.root)
                .join("mise.toml")
                .to_string_lossy()
                .into(),
            settings,
            phase,
            studio_url: None,
            last_error: None,
            hf_env_var,
            step: None,
            started_at: None,
            step_started_at: None,
            elapsed_secs: None,
        }
    }
}

pub fn effective_hf_home(configured: &str) -> (Option<String>, String) {
    ["HF_HOME", "HF_HUB_CACHE"]
        .into_iter()
        .find_map(|name| {
            std::env::var_os(name)
                .map(|value| (name.to_string(), value.to_string_lossy().into_owned()))
        })
        .map_or((None, configured.to_string()), |(name, path)| {
            (Some(name), path)
        })
}

fn set_step(app: &AppHandle, shared: &Shared, index: u32, total: u32, name: &str) {
    let mut state = shared.state.lock().unwrap();
    clear_step(&mut state);
    state.step_started_at = Some(now_secs());
    state.step = Some(Step {
        index,
        total,
        name: name.into(),
    });
    emit(app, &state);
}

fn clear_step(state: &mut State) {
    state.step = None;
    state.step_started_at = None;
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn begin_operation(state: &mut State) {
    CANCEL.store(false, Ordering::Relaxed);
    state.started_at = Some(now_secs());
    state.elapsed_secs = None;
}
fn finish_operation(state: &mut State) {
    state.elapsed_secs = state
        .started_at
        .map(|started| now_secs().saturating_sub(started));
    state.started_at = None;
    clear_step(state);
}
pub fn emit(app: &AppHandle, state: &State) {
    let _ = app.emit("state", state);
}
pub fn cancel(shared: &Shared) -> Result<()> {
    if shared.operation.lock().unwrap().is_some() {
        CANCEL.store(true, Ordering::Relaxed);
    }
    Ok(())
}
fn reserve(shared: &Shared) -> Result<()> {
    let mut op = shared.operation.lock().unwrap();
    if op.is_some() {
        bail!("operation_in_progress");
    }
    *op = Some(1);
    Ok(())
}
fn release(shared: &Shared) {
    *shared.operation.lock().unwrap() = None;
}
fn with_operation<F>(shared: &Shared, f: F) -> Result<State>
where
    F: FnOnce() -> Result<State>,
{
    reserve(shared)?;
    let r = f();
    release(shared);
    r
}
pub fn mark_failed(app: &AppHandle, shared: &Shared, e: &anyhow::Error) {
    let mut st = shared.state.lock().unwrap();
    st.phase = Phase::Failed;
    finish_operation(&mut st);
    st.last_error = Some(ErrorInfo::from_error(e));
    emit(app, &st);
}
pub fn environment(c: &mut Command, s: &Settings) -> Result<()> {
    c.env_clear();
    for k in [
        "PATH",
        "SystemRoot",
        "WINDIR",
        "COMSPEC",
        "USERPROFILE",
        "HOMEDRIVE",
        "HOMEPATH",
        "APPDATA",
        "LOCALAPPDATA",
        "TEMP",
        "TMP",
        "PATHEXT",
        "SystemDrive",
        "ProgramData",
        "ProgramFiles",
        "ProgramFiles(x86)",
        "NUMBER_OF_PROCESSORS",
        "USERNAME",
        "HF_HOME",
        "HF_HUB_CACHE",
    ] {
        if let Some(v) = std::env::var_os(k) {
            c.env(k, v);
        }
    }
    let root = PathBuf::from(&s.root);
    let rt = root.join("runtime");
    let ud = root.join("user-data");
    c.env("HOME", root.join("app"))
        .env("UNSLOTH_STUDIO_HOME", root.join("app"))
        .env("APPDATA", ud.join("roaming"))
        .env("LOCALAPPDATA", ud.join("local"))
        .env("TEMP", rt.join("tmp"))
        .env("TMP", rt.join("tmp"))
        .env("UV_CACHE_DIR", rt.join("uv-cache"))
        .env("MISE_DATA_DIR", rt.join("mise-data"))
        .env("MISE_CONFIG_DIR", rt.join("mise-config"))
        .env("MISE_CACHE_DIR", rt.join("mise-cache"))
        .env("MISE_STATE_DIR", rt.join("mise-state"))
        .env("MISE_TMP_DIR", rt.join("tmp"))
        // The upstream installer ends with an interactive "Start Unsloth Studio now?" prompt and
        // blocks forever without a console. This flag is its documented non-interactive switch.
        .env("UNSLOTH_SKIP_AUTOSTART", "1");
    if std::env::var_os("HF_HOME").is_none() && std::env::var_os("HF_HUB_CACHE").is_none() {
        c.env("HF_HOME", &s.hf_home);
    }
    Ok(())
}
fn mise() -> Command {
    Command::new(std::env::var_os("MISE_BIN").unwrap_or_else(|| std::ffi::OsString::from("mise")))
}
fn run(app: &AppHandle, cmd: &mut Command, settings: &Settings) -> Result<()> {
    environment(cmd, settings)?;
    emit_command(app, cmd);
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let finished = Arc::new(AtomicUsize::new(0));
    let out_thread = stream_lines(app.clone(), stdout, false, finished.clone());
    let err_thread = stream_lines(app.clone(), stderr, true, finished.clone());
    let pid = child.id();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if CANCEL.load(Ordering::Relaxed) {
            let _ = Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/T", "/F"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            let _ = child.kill();
            let _ = child.wait();
            CANCEL.store(false, Ordering::Relaxed);
            emit_log(app, "cancelled by user");
            bail!("cancelled by user");
        }
        thread::sleep(Duration::from_millis(250));
    };
    let deadline = Instant::now() + Duration::from_secs(3);
    while finished.load(Ordering::Acquire) < 2 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    if finished.load(Ordering::Acquire) < 2 {
        emit_log(
            app,
            "(output pipes still held open by a child process; continuing)",
        );
    } else {
        out_thread
            .join()
            .map_err(|_| anyhow::anyhow!("stdout reader failed"))??;
        err_thread
            .join()
            .map_err(|_| anyhow::anyhow!("stderr reader failed"))??;
    }
    if !status.success() {
        bail!("child command failed: {}", status);
    }
    Ok(())
}
fn emit_command(app: &AppHandle, cmd: &Command) {
    let mut line = format!("$ {}", cmd.get_program().to_string_lossy());
    for arg in cmd.get_args() {
        line.push(' ');
        line.push_str(&arg.to_string_lossy());
    }
    emit_log(app, &line);
}
fn stream_lines<R: Read + Send + 'static>(
    app: AppHandle,
    reader: R,
    stderr: bool,
    finished: Arc<AtomicUsize>,
) -> thread::JoinHandle<Result<()>> {
    thread::spawn(move || {
        let _finished = Finished(finished);
        let mut reader = BufReader::new(reader);
        let mut bytes = Vec::new();
        loop {
            bytes.clear();
            let count = reader.read_until(b'\n', &mut bytes)?;
            if count == 0 {
                break;
            }
            for part in bytes.split(|byte| *byte == b'\r') {
                let line = String::from_utf8_lossy(part).trim_end().to_string();
                if !line.is_empty() {
                    let line = strip_ansi(&line);
                    emit_log(&app, &if stderr { format!("! {line}") } else { line });
                }
            }
        }
        Ok(())
    })
}
struct Finished(Arc<AtomicUsize>);
impl Drop for Finished {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::Release);
    }
}
fn emit_log(app: &AppHandle, line: &str) {
    let _ = app.emit("log", line.to_string());
}
fn strip_ansi(text: &str) -> String {
    let mut clean = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' && chars.peek() == Some(&'[') {
            chars.next();
            for code in chars.by_ref() {
                if ('@'..='~').contains(&code) {
                    break;
                }
            }
        } else {
            clean.push(ch);
        }
    }
    clean
}
pub async fn install(app: AppHandle, shared: std::sync::Arc<Shared>) -> Result<State> {
    reserve(&shared)?;
    let worker = shared.clone();
    let handle = app.clone();
    let joined =
        tauri::async_runtime::spawn_blocking(move || install_blocking(&handle, &worker)).await;
    release(&shared);
    match joined {
        Ok(Ok(state)) => Ok(state),
        Ok(Err(e)) => {
            mark_failed(&app, &shared, &e);
            Err(e)
        }
        Err(e) => {
            let e = anyhow::anyhow!("install task failed: {e}");
            mark_failed(&app, &shared, &e);
            Err(e)
        }
    }
}
fn install_blocking(app: &AppHandle, shared: &Shared) -> Result<State> {
    let s = {
        let mut st = shared.state.lock().unwrap();
        begin_operation(&mut st);
        st.phase = Phase::Installing;
        st.step = Some(Step {
            index: 1,
            total: 6,
            name: "Preparing folders".into(),
        });
        st.step_started_at = Some(now_secs());
        st.last_error = None;
        emit(app, &st);
        st.settings.clone()
    };
    let root = PathBuf::from(&s.root);
    fs::create_dir_all(&root)?;
    for p in [
        "runtime/tmp",
        "runtime/uv-cache",
        "runtime/mise-data",
        "runtime/mise-config",
        "runtime/mise-cache",
        "runtime/mise-state",
        "user-data",
    ] {
        fs::create_dir_all(root.join(p))?;
    }
    let mc = root.join("mise.toml");
    set_step(app, shared, 2, 6, "Writing mise.toml");
    if !mc.exists() {
        fs::write(&mc, include_str!("../assets/mise.toml"))?;
    }
    let src = root.join("app/source");
    set_step(app, shared, 3, 6, "Fetching Studio source");
    fs::create_dir_all(src.parent().unwrap())?;
    if src.exists() {
        run(
            app,
            Command::new("git")
                .arg("-C")
                .arg(&src)
                .args(["fetch", "--depth", "1", "origin", "main"]),
            &s,
        )?;
        run(
            app,
            Command::new("git")
                .arg("-C")
                .arg(&src)
                .args(["reset", "--hard", "FETCH_HEAD"]),
            &s,
        )?;
    } else {
        run(
            app,
            Command::new("git")
                .args([
                    "clone",
                    "--depth",
                    "1",
                    "--branch",
                    "main",
                    "https://github.com/unslothai/unsloth.git",
                ])
                .arg(&src),
            &s,
        )?;
    }
    set_step(app, shared, 4, 6, "Installing tools");
    run(app, mise().arg("--cd").arg(&root).arg("install"), &s)?;
    set_step(app, shared, 5, 6, "Running Studio setup");
    run(
        app,
        mise()
            .arg("--cd")
            .arg(&root)
            .args([
                "exec",
                "--",
                "powershell",
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ])
            .arg(src.join("install.ps1"))
            .args(["--local", "--skip-autostart", "--isolated-uv-cache"]),
        &s,
    )?;
    let mut st = shared.state.lock().unwrap();
    st.step = Some(Step {
        index: 6,
        total: 6,
        name: "Finalising".into(),
    });
    emit(app, &st);
    st.phase = Phase::Installed;
    finish_operation(&mut st);
    emit(app, &st);
    Ok(st.clone())
}
pub fn stop_child(shared: &Shared) -> Result<()> {
    if let Some(mut c) = shared.child.lock().unwrap().take() {
        let _ = c.kill();
        let _ = c.wait();
    }
    Ok(())
}
pub fn launch(app: &AppHandle, shared: &Arc<Shared>) -> Result<State> {
    with_operation(shared, || {
        let mut state = shared.state.lock().unwrap();
        if !matches!(state.phase, Phase::Installed) {
            bail!("invalid_argument");
        }
        state.phase = Phase::Starting;
        begin_operation(&mut state);
        state.step = Some(Step {
            index: 1,
            total: 2,
            name: "Starting Studio".into(),
        });
        state.step_started_at = Some(now_secs());
        state.last_error = None;
        emit(app, &state);
        let root = PathBuf::from(&state.settings.root);
        let python = root.join("app/unsloth_studio/Scripts/python.exe");
        let mut command = mise();
        command
            .arg("--cd")
            .arg(&root)
            .args(["exec", "--"])
            .arg(python)
            .args(["-I", "-m", "unsloth_cli", "studio", "-H", "127.0.0.1", "-p"])
            .arg(state.settings.port.to_string());
        environment(&mut command, &state.settings)?;
        emit_log(
            app,
            "Tip: Starting Unsloth Studio takes a few seconds. Click Open Studio to open it in a new browser tab.",
        );
        emit_command(app, &command);
        let mut child = command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        // Reader threads end when Stop kills the stored child and closes its pipes.
        let finished = Arc::new(AtomicUsize::new(0));
        let _out_thread = stream_lines(app.clone(), stdout, false, finished.clone());
        let _err_thread = stream_lines(app.clone(), stderr, true, finished);
        *shared.child.lock().unwrap() = Some(child);
        state.step = Some(Step {
            index: 2,
            total: 2,
            name: "Connecting".into(),
        });
        state.step_started_at = Some(now_secs());
        emit(app, &state);
        state.phase = Phase::Running;
        finish_operation(&mut state);
        state.studio_url = Some(format!("http://127.0.0.1:{}", state.settings.port));
        emit(app, &state);
        // Watcher: the Studio process can die silently after start (for example a missing module),
        // so surface the exit instead of leaving the UI stuck on "running".
        let watched = shared.clone();
        let watch_app = app.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_millis(500));
            let exited = {
                let mut guard = watched.child.lock().unwrap();
                match guard.as_mut() {
                    Some(child) => matches!(child.try_wait(), Ok(Some(_))),
                    None => false,
                }
            };
            if exited {
                let mut st = watched.state.lock().unwrap();
                if matches!(st.phase, Phase::Starting | Phase::Running) {
                    st.phase = Phase::Failed;
                    st.step = None;
                    st.studio_url = None;
                    st.last_error = Some(ErrorInfo {
                        code: "start_failed".into(),
                        message: "Studio exited right after start. Use Update to repair the installation."
                            .into(),
                    });
                    emit(&watch_app, &st);
                    drop(st);
                    emit_log(
                        &watch_app,
                        "Error: Studio exited right after start. Use Update to repair the installation.",
                    );
                }
                return;
            }
            if !matches!(
                watched.state.lock().unwrap().phase,
                Phase::Starting | Phase::Running
            ) {
                return;
            }
        });
        Ok(state.clone())
    })
}
pub fn stop(app: &AppHandle, shared: &Shared) -> Result<State> {
    with_operation(shared, || {
        let mut st = shared.state.lock().unwrap();
        if !matches!(st.phase, Phase::Starting | Phase::Running) {
            bail!("invalid_argument");
        }
        st.phase = Phase::Stopping;
        begin_operation(&mut st);
        st.step = Some(Step {
            index: 1,
            total: 1,
            name: "Stopping Studio".into(),
        });
        st.step_started_at = Some(now_secs());
        emit(app, &st);
        stop_child(shared)?;
        emit_log(app, "Unsloth Studio has been stopped.");
        st.phase = Phase::Installed;
        finish_operation(&mut st);
        st.studio_url = None;
        emit(app, &st);
        Ok(st.clone())
    })
}
pub fn remove(app: &AppHandle, shared: &Shared) -> Result<State> {
    with_operation(shared, || {
        let mut st = shared.state.lock().unwrap();
        if !matches!(st.phase, Phase::Installed | Phase::Failed) {
            bail!("invalid_argument");
        }
        st.phase = Phase::Removing;
        begin_operation(&mut st);
        st.step = Some(Step {
            index: 1,
            total: 2,
            name: "Stopping Studio".into(),
        });
        st.step_started_at = Some(now_secs());
        emit(app, &st);
        stop_child(shared)?;
        st.step = Some(Step {
            index: 2,
            total: 2,
            name: "Deleting product files".into(),
        });
        st.step_started_at = Some(now_secs());
        emit(app, &st);
        let root = PathBuf::from(&st.settings.root);
        safe_remove(&root)?;
        st.phase = Phase::Absent;
        finish_operation(&mut st);
        st.studio_url = None;
        st.last_error = None;
        emit(app, &st);
        Ok(st.clone())
    })
}
// Deletion stays inside the canonicalised product root: every entry is
// re-canonicalised, so a reparse point that resolves elsewhere is skipped.
// mise.toml and user-data are never deleted.
pub fn safe_remove(root: &Path) -> Result<()> {
    let canonical = root.canonicalize().context("product root is missing")?;
    fn walk(p: &Path, r: &Path) -> Result<()> {
        for e in fs::read_dir(p)? {
            let p = e?.path();
            if p.file_name()
                .is_some_and(|n| n == "mise.toml" || n == "user-data")
            {
                continue;
            }
            let Ok(c) = p.canonicalize() else {
                continue;
            };
            if !c.starts_with(r) {
                continue;
            }
            if c.is_dir() {
                walk(&c, r)?;
                fs::remove_dir(c)?
            } else {
                fs::remove_file(c)?
            }
        }
        Ok(())
    }
    walk(&canonical, &canonical)
}
