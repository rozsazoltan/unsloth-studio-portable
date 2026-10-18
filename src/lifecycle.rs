use crate::settings::{safe_owned_paths, Settings};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::Digest;
#[cfg(windows)]
use std::{collections::HashMap, sync::atomic::AtomicU64};
use std::{
    collections::HashSet,
    fs,
    io::Read,
    net::{TcpStream, ToSocketAddrs},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const UPSTREAM_URL: &str = "https://github.com/unslothai/unsloth.git";
const PINNED_SOURCE_COMMIT: &str = "5e55110278872d71a0f97fb951fedd1305d926fb";
const UV_MISE_BACKEND: &str = "aqua:astral-sh/uv@0.13.1";
const MAX_CAPTURE_BYTES: usize = 64 * 1024;

#[derive(Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub enum Phase {
    Absent,
    Installing,
    Ready,
    Starting,
    Running,
    Stopping,
    Updating,
    Removing,
    Failed,
}

#[derive(Clone, Serialize)]
pub struct ErrorInfo {
    pub code: String,
    pub message: String,
}

#[derive(Clone, Serialize)]
pub struct Operation {
    pub id: String,
    pub kind: String,
    pub detail: String,
}

#[derive(Clone, Serialize)]
pub struct Removal {
    pub token: String,
    pub paths: Vec<String>,
    pub preserved_paths: Vec<String>,
    pub warning: String,
}

#[derive(Clone, Serialize)]
pub struct State {
    pub revision: u64,
    pub settings: Settings,
    pub phase: Phase,
    pub operation: Option<Operation>,
    pub installed_commit: Option<String>,
    pub upstream_commit: Option<String>,
    pub update_status: String,
    pub studio_url: Option<String>,
    pub last_error: Option<ErrorInfo>,
    pub removal: Option<Removal>,
}

pub type SharedState = Arc<Mutex<State>>;

impl State {
    pub fn new(settings: Settings) -> Self {
        let phase = if installed(&settings) {
            Phase::Ready
        } else {
            Phase::Absent
        };
        Self {
            revision: 1,
            settings,
            phase,
            operation: None,
            installed_commit: None,
            upstream_commit: None,
            update_status: "unknown".into(),
            studio_url: None,
            last_error: None,
            removal: None,
        }
    }
    pub fn snapshot(&mut self) -> serde_json::Value {
        self.refresh();
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }

    pub fn refresh(&mut self) {
        if self.operation.as_ref().is_some_and(|operation| {
            matches!(operation.kind.as_str(), "start" | "open_studio")
                && matches!(self.phase, Phase::Starting)
        }) {
            return;
        }
        if !is_running()
            && matches!(
                self.phase,
                Phase::Running | Phase::Starting | Phase::Stopping
            )
        {
            self.studio_url = None;
            self.phase = if installed(&self.settings) {
                Phase::Ready
            } else {
                Phase::Absent
            };
            self.revision += 1;
        }
    }
}

pub fn run_operation(state: SharedState, command: String) -> Result<()> {
    run_operation_approved(state, command, false)
}

pub fn run_operation_approved(
    state: SharedState,
    command: String,
    upstream_setup_approved: bool,
) -> Result<()> {
    if matches!(command.as_str(), "install" | "update") && !upstream_setup_approved {
        bail!("explicit per-operation approval is required because upstream setup may change Windows user or system state");
    }
    if command == "check_updates" {
        return start_update_check(state);
    }
    let mut s = state.lock().unwrap();
    if s.operation.is_some() {
        bail!("another operation is already running");
    }
    s.refresh();
    if matches!(command.as_str(), "update" | "install" | "start" | "stop")
        && matches!(s.phase, Phase::Running | Phase::Starting | Phase::Stopping)
        && command != "stop"
    {
        bail!("stop Studio before installing or updating");
    }
    let id = uuid::Uuid::new_v4().to_string();
    s.operation = Some(Operation {
        id,
        kind: command.clone(),
        detail: "Starting".into(),
    });
    s.phase = match command.as_str() {
        "install" => Phase::Installing,
        "update" => Phase::Updating,
        "start" | "open_studio" => Phase::Starting,
        "stop" => Phase::Stopping,
        _ => s.phase.clone(),
    };
    s.revision += 1;
    let settings = s.settings.clone();
    let operation_id = s.operation.as_ref().unwrap().id.clone();
    drop(s);
    thread::spawn(move || {
        let result = match command.as_str() {
            "install" => install(&settings, &state, &operation_id).map(|_| ()),
            "update" => update(&settings, &state, &operation_id).map(|_| ()),
            "start" | "open_studio" => start(&settings),
            "stop" => stop_owned(),
            _ => Err(anyhow::anyhow!("unknown operation")),
        };
        let result = result.and_then(|()| ensure_operations_not_cancelled());
        let update_result = if result.is_ok() && matches!(command.as_str(), "install" | "update") {
            let check = check_updates(&settings);
            let local_commit = if check.is_err() {
                git_output(
                    &settings,
                    &["rev-parse", "HEAD"],
                    "read installed commit",
                    Duration::from_secs(30),
                )
                .ok()
                .filter(|commit| validate_commit(commit).is_ok())
            } else {
                None
            };
            Some((check, local_commit))
        } else {
            None
        };
        let mut state = state.lock().unwrap();
        state.operation = None;
        match result {
            Ok(()) => {
                state.last_error = None;
                if let Some((update_result, local_commit)) = update_result {
                    match update_result {
                        Ok((installed, upstream, status)) => {
                            state.installed_commit = installed;
                            state.upstream_commit = upstream;
                            state.update_status = status;
                        }
                        Err(_) => {
                            state.installed_commit = local_commit;
                            state.upstream_commit = None;
                            state.update_status = "error".into();
                        }
                    }
                }
                let running = is_running();
                state.studio_url = running.then(|| format!("http://127.0.0.1:{}", settings.port));
                state.phase = if running {
                    Phase::Running
                } else if installed(&settings) {
                    Phase::Ready
                } else {
                    Phase::Absent
                };
            }
            Err(e) => {
                state.phase = if command == "stop" {
                    if is_running() {
                        Phase::Running
                    } else if installed(&settings) {
                        Phase::Ready
                    } else {
                        Phase::Absent
                    }
                } else {
                    Phase::Failed
                };
                state.last_error = Some(ErrorInfo {
                    code: "operation_failed".into(),
                    message: format!("{e:#}"),
                });
            }
        }
        state.revision += 1;
    });
    Ok(())
}

fn start_update_check(state: SharedState) -> Result<()> {
    let mut current = state.lock().unwrap();
    if current.update_status == "checking" {
        return Ok(());
    }
    current.update_status = "checking".into();
    current.revision += 1;
    let settings = current.settings.clone();
    let settings_bytes = serde_json::to_vec(&settings)?;
    drop(current);
    thread::spawn(move || {
        let result = check_updates(&settings);
        let mut current = state.lock().unwrap();
        if serde_json::to_vec(&current.settings).ok().as_deref() != Some(settings_bytes.as_slice())
        {
            current.update_status = "unknown".into();
            current.installed_commit = None;
            current.upstream_commit = None;
            current.revision += 1;
            return;
        }
        match result {
            Ok((installed, upstream, status)) => {
                current.installed_commit = installed;
                current.upstream_commit = upstream;
                current.update_status = status;
                current.last_error = None;
            }
            Err(error) => {
                current.update_status = "error".into();
                current.last_error = Some(ErrorInfo {
                    code: "update_check_failed".into(),
                    message: format!("{error:#}"),
                });
            }
        }
        current.revision += 1;
    });
    Ok(())
}

fn install(settings: &Settings, state: &SharedState, operation_id: &str) -> Result<()> {
    ensure_operations_not_cancelled()?;
    settings.validate()?;
    if is_running() {
        bail!("stop Studio before installing");
    }
    let mise = ensure_prerequisites()?;
    let source = settings.app_dir().join("source");
    crate::settings::reject_reparse_components(&settings.root)?;
    crate::settings::reject_reparse_components(&settings.app_dir())?;
    crate::settings::reject_reparse_components(&source)?;
    if source.exists() {
        bail!("source directory already exists; remove or inspect it before installing");
    }
    if settings.app_dir().join("unsloth_studio").exists() {
        bail!("Studio runtime already exists without a managed source checkout; inspect it before installing");
    }

    ensure_operations_not_cancelled()?;
    progress(
        state,
        operation_id,
        "Claiming empty product root and preparing managed directories",
    );
    settings.mark_owned()?;
    fs::create_dir_all(settings.app_dir())?;
    prepare_runtime_dirs(settings)?;

    ensure_operations_not_cancelled()?;
    progress(
        state,
        operation_id,
        "Fetching pinned official Unsloth source",
    );
    let mut init = Command::new("git");
    init.args(["-c", "core.hooksPath=NUL", "init", "--"])
        .arg(&source);
    apply_managed_environment(&mut init, settings)?;
    if !run_command(
        &mut init,
        Duration::from_secs(60),
        "initialize source checkout",
    )?
    .success
    {
        bail!("could not initialize official source checkout; verify product-root permissions");
    }
    ensure_operations_not_cancelled()?;
    git_checked(
        settings,
        &["remote", "add", "origin", UPSTREAM_URL],
        "configure official source remote",
        Duration::from_secs(60),
    )?;
    git_checked(
        settings,
        &[
            "fetch",
            "--no-tags",
            "--depth=1",
            "origin",
            PINNED_SOURCE_COMMIT,
        ],
        "fetch pinned official source",
        Duration::from_secs(300),
    )?;
    git_checked(
        settings,
        &["checkout", "--detach", PINNED_SOURCE_COMMIT],
        "check out pinned official source",
        Duration::from_secs(60),
    )?;
    crate::settings::reject_reparse_tree(&source)?;

    ensure_operations_not_cancelled()?;
    progress(
        state,
        operation_id,
        "Running approved upstream Studio setup through mise",
    );
    run_upstream_setup(settings, &mise, &source, operation_id, state)?;
    ensure_operations_not_cancelled()?;
    verify_install(settings)?;
    Ok(())
}

fn update(settings: &Settings, state: &SharedState, operation_id: &str) -> Result<()> {
    ensure_operations_not_cancelled()?;
    settings.validate()?;
    if is_running() {
        bail!("stop Studio before updating");
    }
    if !settings.is_owned() {
        bail!("launcher ownership marker is missing; refusing to update unowned files");
    }
    if !installed(settings) {
        bail!("verified Studio interpreter or source checkout is missing; refusing to update");
    }
    let source = settings.app_dir().join("source");
    if !source.join(".git").exists() {
        bail!("owned source checkout is missing; install Studio before updating");
    }
    crate::settings::reject_reparse_tree(&source)?;
    verify_official_clean_checkout(settings)?;
    let mise = ensure_prerequisites()?;
    ensure_operations_not_cancelled()?;
    prepare_runtime_dirs(settings)?;

    progress(state, operation_id, "Fetching official main branch");
    git_checked(
        settings,
        &["fetch", "--no-tags", "origin", "refs/heads/main"],
        "fetch official updates",
        Duration::from_secs(600),
    )?;
    ensure_operations_not_cancelled()?;
    if git_output(
        settings,
        &["rev-parse", "--is-shallow-repository"],
        "inspect source history",
        Duration::from_secs(30),
    )? == "true"
    {
        progress(
            state,
            operation_id,
            "Deepening shallow source history for safe fast-forward",
        );
        git_checked(
            settings,
            &[
                "fetch",
                "--no-tags",
                "--unshallow",
                "origin",
                "refs/heads/main",
            ],
            "deepen official source history",
            Duration::from_secs(1800),
        )?;
    }
    let upstream = git_output(
        settings,
        &["rev-parse", "FETCH_HEAD"],
        "validate fetched commit",
        Duration::from_secs(30),
    )?;
    validate_commit(&upstream)?;
    let ancestry = git_status(
        settings,
        &["merge-base", "--is-ancestor", "HEAD", "FETCH_HEAD"],
        "check fast-forward ancestry",
        Duration::from_secs(30),
    )?;
    if !ancestry.success {
        bail!("update is not a fast-forward from installed source; inspect the checkout before retrying");
    }
    ensure_operations_not_cancelled()?;
    progress(
        state,
        operation_id,
        "Applying verified fast-forward source update",
    );
    git_checked(
        settings,
        &["merge", "--ff-only", "FETCH_HEAD"],
        "fast-forward official source",
        Duration::from_secs(120),
    )?;

    ensure_operations_not_cancelled()?;
    progress(
        state,
        operation_id,
        "Running approved upstream Studio setup through mise",
    );
    run_upstream_setup(settings, &mise, &source, operation_id, state)?;
    ensure_operations_not_cancelled()?;
    verify_install(settings)?;
    Ok(())
}

fn progress(state: &SharedState, operation_id: &str, detail: &str) {
    if let Ok(mut current) = state.lock() {
        if let Some(operation) = current
            .operation
            .as_mut()
            .filter(|op| op.id == operation_id)
        {
            operation.detail = detail.to_owned();
            current.revision += 1;
        }
    }
}

fn ensure_prerequisites() -> Result<PathBuf> {
    let mise = ensure_mise()?;
    let git = PathBuf::from("git");
    let git_version = run_command(
        Command::new(&git).arg("--version"),
        Duration::from_secs(15),
        "Git prerequisite check",
    )
    .context("Git for Windows is required; install Git and ensure git.exe is available in PATH")?;
    if !git_version.success {
        bail!("Git prerequisite check failed; repair Git for Windows before continuing");
    }
    powershell_executable()?;
    Ok(mise)
}

fn ensure_mise() -> Result<PathBuf> {
    let mise = std::env::var_os("MISE_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("mise"));
    let version = run_command(
        Command::new(&mise).arg("--version"),
        Duration::from_secs(15),
        "mise prerequisite check",
    )
    .context(
        "mise-en-place is required; install mise and ensure it is available in PATH or MISE_BIN",
    )?;
    if !version.success {
        bail!("mise prerequisite check failed; install or repair mise-en-place before continuing");
    }
    Ok(mise)
}

fn powershell_executable() -> Result<PathBuf> {
    let windows = std::env::var_os("SystemRoot")
        .or_else(|| std::env::var_os("WINDIR"))
        .map(PathBuf::from)
        .context("Windows system directory is unavailable")?;
    let powershell = windows.join("System32/WindowsPowerShell/v1.0/powershell.exe");
    if !powershell.is_file() {
        bail!(
            "Windows PowerShell was not found at {}; repair Windows PowerShell before setup",
            powershell.display()
        );
    }
    Ok(powershell)
}

fn prepare_runtime_dirs(settings: &Settings) -> Result<()> {
    for path in [
        settings.root.join("runtime"),
        settings.root.join("runtime/uv-cache"),
        settings.root.join("runtime/mise-data"),
        settings.root.join("runtime/mise-config"),
        settings.root.join("runtime/mise-cache"),
        settings.root.join("runtime/mise-state"),
        settings.root.join("runtime/tmp"),
        settings.root.join("user-data/roaming"),
        settings.root.join("user-data/local"),
    ] {
        crate::settings::reject_reparse_components(&path)?;
        fs::create_dir_all(path)?;
    }
    Ok(())
}

fn verify_official_clean_checkout(settings: &Settings) -> Result<()> {
    let remote = git_output(
        settings,
        &["remote", "get-url", "origin"],
        "verify source remote",
        Duration::from_secs(30),
    )?;
    if remote != UPSTREAM_URL {
        bail!("source origin does not match the fixed official Unsloth HTTPS repository");
    }
    let status = git_output(
        settings,
        &["status", "--porcelain", "--untracked-files=all"],
        "verify clean source",
        Duration::from_secs(30),
    )?;
    if !status.is_empty() {
        bail!(
            "source checkout has local or untracked changes; commit or remove them before updating"
        );
    }
    Ok(())
}

fn git_status(
    settings: &Settings,
    args: &[&str],
    phase: &str,
    timeout: Duration,
) -> Result<CommandOutput> {
    let mut command = git_command(settings, args)?;
    run_command(&mut command, timeout, phase)
}

fn git_checked(
    settings: &Settings,
    args: &[&str],
    phase: &str,
    timeout: Duration,
) -> Result<String> {
    let output = git_status(settings, args, phase, timeout)?;
    if !output.success {
        bail!("{phase} failed; verify network access and source permissions");
    }
    Ok(output.stdout)
}

fn git_output(
    settings: &Settings,
    args: &[&str],
    phase: &str,
    timeout: Duration,
) -> Result<String> {
    git_checked(settings, args, phase, timeout).map(|out| out.trim().to_owned())
}

fn git_command(settings: &Settings, args: &[&str]) -> Result<Command> {
    let source = settings.app_dir().join("source");
    let mut command = Command::new("git");
    command
        .arg("-c")
        .arg("core.hooksPath=NUL")
        .arg("-c")
        .arg("credential.interactive=never")
        .arg("-C")
        .arg(source)
        .args(args);
    apply_managed_environment(&mut command, settings)?;
    Ok(command)
}

fn run_upstream_setup(
    settings: &Settings,
    mise: &Path,
    source: &Path,
    operation_id: &str,
    state: &SharedState,
) -> Result<()> {
    let powershell = powershell_executable()?;
    let script = source.join("install.ps1");
    if !script.is_file() {
        bail!("pinned source checkout does not contain install.ps1");
    }
    crate::settings::reject_reparse_components(&script)?;
    let mut command = Command::new(mise);
    command
        .args(["--no-config", "--no-hooks", "exec", UV_MISE_BACKEND, "--"])
        .arg(powershell)
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(script)
        .args(["--local", "--skip-autostart", "--isolated-uv-cache"])
        .current_dir(source);
    apply_managed_environment(&mut command, settings)?;
    progress(
        state,
        operation_id,
        "Approved setup may change Windows user state; running with product-local caches",
    );
    let output = run_command(
        &mut command,
        Duration::from_secs(2700),
        "upstream Studio setup",
    )?;
    if !output.success {
        bail!("upstream setup failed; it may have changed user state and no automatic rollback was attempted");
    }
    Ok(())
}

fn apply_managed_environment(command: &mut Command, settings: &Settings) -> Result<()> {
    command.env_clear();
    for key in [
        "PATH",
        "SystemRoot",
        "WINDIR",
        "COMSPEC",
        "PROCESSOR_ARCHITECTURE",
        "PROCESSOR_ARCHITEW6432",
        "NUMBER_OF_PROCESSORS",
        "USERNAME",
        "USERDOMAIN",
        "USERDNSDOMAIN",
        "USERPROFILE",
        "HOMEDRIVE",
        "HOMEPATH",
        "PUBLIC",
        "ALLUSERSPROFILE",
    ] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    let runtime = settings.root.join("runtime");
    let user_data = settings.root.join("user-data");
    command
        .env("HOME", settings.app_dir())
        .env("APPDATA", user_data.join("roaming"))
        .env("LOCALAPPDATA", user_data.join("local"))
        .env("TEMP", runtime.join("tmp"))
        .env("TMP", runtime.join("tmp"))
        .env("UNSLOTH_STUDIO_HOME", settings.app_dir())
        .env("UNSLOTH_ISOLATE_UV_CACHE", "1")
        .env("UV_CACHE_DIR", runtime.join("uv-cache"))
        .env("HF_HOME", &settings.hf_home)
        .env("HF_HUB_CACHE", settings.hf_home.join("hub"))
        .env("HF_DATASETS_CACHE", settings.hf_home.join("datasets"))
        .env("MISE_DATA_DIR", runtime.join("mise-data"))
        .env("MISE_CONFIG_DIR", runtime.join("mise-config"))
        .env("MISE_CACHE_DIR", runtime.join("mise-cache"))
        .env("MISE_STATE_DIR", runtime.join("mise-state"))
        .env("MISE_TMP_DIR", runtime.join("tmp"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", runtime.join("gitconfig"))
        .env("GIT_TERMINAL_PROMPT", "0");
    Ok(())
}

fn verify_install(settings: &Settings) -> Result<()> {
    let source = settings.app_dir().join("source");
    crate::settings::reject_reparse_tree(&source)?;
    if !settings.is_owned() {
        bail!("launcher ownership marker was lost during setup");
    }
    let python = settings
        .app_dir()
        .join("unsloth_studio")
        .join("Scripts")
        .join("python.exe");
    crate::settings::reject_reparse_components(&python)?;
    if !python.is_file() {
        bail!("setup completed without expected interpreter app/unsloth_studio/Scripts/python.exe");
    }
    Ok(())
}

fn validate_commit(commit: &str) -> Result<()> {
    if commit.len() != 40 || !commit.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("official repository returned an invalid commit identifier");
    }
    Ok(())
}

pub fn cancel_owned_operations() -> Result<()> {
    COMMANDS_CANCELLED.store(true, Ordering::SeqCst);
    #[cfg(windows)]
    {
        let jobs = COMMAND_JOBS.get_or_init(|| Mutex::new(HashMap::new()));
        let jobs = jobs.lock().unwrap();
        let mut failed = false;
        for handle in jobs.values() {
            if terminate_job_handle(*handle).is_err() {
                failed = true;
            }
        }
        if failed {
            bail!("could not terminate every launcher-owned command job during shutdown");
        }
    }
    Ok(())
}

fn ensure_operations_not_cancelled() -> Result<()> {
    if COMMANDS_CANCELLED.load(Ordering::SeqCst) {
        bail!("launcher operations were canceled during shutdown");
    }
    Ok(())
}

#[cfg(windows)]
struct CommandJob {
    id: u64,
    handle: usize,
}

#[cfg(windows)]
impl CommandJob {
    fn new() -> Result<Self> {
        let handle = create_kill_on_close_job()?;
        let jobs = COMMAND_JOBS.get_or_init(|| Mutex::new(HashMap::new()));
        let mut jobs = jobs.lock().unwrap();
        if COMMANDS_CANCELLED.load(Ordering::SeqCst) {
            unsafe { close_job_handle(handle) };
            bail!("launcher operations were canceled during shutdown");
        }
        let id = NEXT_COMMAND_JOB.fetch_add(1, Ordering::Relaxed);
        jobs.insert(id, handle);
        Ok(Self { id, handle })
    }

    fn spawn(&self, command: &mut Command) -> Result<Child> {
        let jobs = COMMAND_JOBS.get_or_init(|| Mutex::new(HashMap::new()));
        let _jobs = jobs.lock().unwrap();
        ensure_operations_not_cancelled()?;
        spawn_suspended_in_job(command, self.handle)
    }

    fn terminate(&self) -> Result<()> {
        terminate_job_handle(self.handle)
    }
}

#[cfg(windows)]
impl Drop for CommandJob {
    fn drop(&mut self) {
        if let Some(jobs) = COMMAND_JOBS.get() {
            if let Ok(mut jobs) = jobs.lock() {
                jobs.remove(&self.id);
            }
        }
        unsafe { close_job_handle(self.handle) };
    }
}

#[cfg(not(windows))]
struct CommandJob;

#[cfg(not(windows))]
impl CommandJob {
    fn new() -> Result<Self> {
        ensure_operations_not_cancelled()?;
        Ok(Self)
    }

    fn spawn(&self, command: &mut Command) -> Result<Child> {
        ensure_operations_not_cancelled()?;
        command.spawn().context("start command process")
    }

    fn terminate(&self) -> Result<()> {
        Ok(())
    }
}

struct CommandOutput {
    success: bool,
    stdout: String,
}

fn run_command(command: &mut Command, timeout: Duration, phase: &str) -> Result<CommandOutput> {
    ensure_operations_not_cancelled()?;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let job = CommandJob::new()?;
    let mut child = job
        .spawn(command)
        .with_context(|| format!("could not start {phase}"))?;
    let stdout = child
        .stdout
        .take()
        .context("command stdout pipe is missing")?;
    let stderr = child
        .stderr
        .take()
        .context("command stderr pipe is missing")?;
    let stdout_reader = thread::spawn(move || drain_output(stdout));
    let stderr_reader = thread::spawn(move || drain_output(stderr));
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(100)),
            Ok(None) => {
                let _ = job.terminate();
                let _ = child.kill();
                let _ = child.wait();
                drop(job);
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                bail!("{phase} timed out after {} seconds", timeout.as_secs());
            }
            Err(error) => {
                let _ = job.terminate();
                let _ = child.kill();
                let _ = child.wait();
                drop(job);
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(error).with_context(|| format!("could not wait for {phase}"));
            }
        }
    };
    drop(job);
    let stdout = stdout_reader
        .join()
        .map_err(|_| anyhow::anyhow!("{phase} stdout reader failed"))??;
    let _stderr = stderr_reader
        .join()
        .map_err(|_| anyhow::anyhow!("{phase} stderr reader failed"))??;
    ensure_operations_not_cancelled()?;
    Ok(CommandOutput {
        success: status.success(),
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
    })
}

fn drain_output(mut reader: impl Read) -> std::io::Result<Vec<u8>> {
    let mut captured = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        let remaining = MAX_CAPTURE_BYTES.saturating_sub(captured.len());
        captured.extend_from_slice(&buffer[..count.min(remaining)]);
    }
    Ok(captured)
}

fn start(settings: &Settings) -> Result<()> {
    settings.validate()?;
    if is_running() {
        bail!("Studio is already running");
    }
    if !installed(settings) {
        bail!("Studio is not installed in a verified launcher-owned root");
    }
    if port_is_listening(settings.port) {
        bail!(
            "localhost port {} is already in use by another process",
            settings.port
        );
    }
    let app_dir = settings.app_dir();
    let python = settings
        .app_dir()
        .join("unsloth_studio")
        .join("Scripts")
        .join("python.exe");
    if !python.is_file() {
        bail!("Studio is not installed. Install is disabled until upstream installer isolation is verified.");
    }
    crate::settings::reject_reparse_components(&python)?;
    let mise = ensure_mise()?;
    let mut command = Command::new(mise);
    command
        .current_dir(&app_dir)
        .args(["--no-config", "--no-hooks", "exec", UV_MISE_BACKEND, "--"])
        .arg(python)
        .args(["-I", "-m", "unsloth_cli", "studio", "-H", "127.0.0.1", "-p"])
        .arg(settings.port.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    apply_managed_environment(&mut command, settings)?;
    let child = launch_in_job(&mut command).context("start Studio process")?;
    remember_child(child)?;
    if let Err(error) = wait_health(settings.port) {
        let _ = stop_owned();
        return Err(error);
    }
    Ok(())
}

#[cfg(windows)]
fn create_kill_on_close_job() -> Result<usize> {
    use std::ffi::c_void;

    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x0000_2000;

    #[repr(C)]
    struct BasicLimitInformation {
        per_process_user_time_limit: i64,
        per_job_user_time_limit: i64,
        limit_flags: u32,
        minimum_working_set_size: usize,
        maximum_working_set_size: usize,
        active_process_limit: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
    }
    #[repr(C)]
    struct IoCounters {
        read_operation_count: u64,
        write_operation_count: u64,
        other_operation_count: u64,
        read_transfer_count: u64,
        write_transfer_count: u64,
        other_transfer_count: u64,
    }
    #[repr(C)]
    struct ExtendedLimitInformation {
        basic: BasicLimitInformation,
        io: IoCounters,
        process_memory_limit: usize,
        job_memory_limit: usize,
        peak_process_memory_used: usize,
        peak_job_memory_used: usize,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateJobObjectW(attributes: *mut c_void, name: *const u16) -> *mut c_void;
        fn SetInformationJobObject(
            job: *mut c_void,
            class: u32,
            info: *const c_void,
            size: u32,
        ) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }

    let job = unsafe { CreateJobObjectW(std::ptr::null_mut(), std::ptr::null()) };
    if job.is_null() {
        bail!("could not create command job object");
    }
    let mut limits = ExtendedLimitInformation {
        basic: BasicLimitInformation {
            per_process_user_time_limit: 0,
            per_job_user_time_limit: 0,
            limit_flags: JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            minimum_working_set_size: 0,
            maximum_working_set_size: 0,
            active_process_limit: 0,
            affinity: 0,
            priority_class: 0,
            scheduling_class: 0,
        },
        io: IoCounters {
            read_operation_count: 0,
            write_operation_count: 0,
            other_operation_count: 0,
            read_transfer_count: 0,
            write_transfer_count: 0,
            other_transfer_count: 0,
        },
        process_memory_limit: 0,
        job_memory_limit: 0,
        peak_process_memory_used: 0,
        peak_job_memory_used: 0,
    };
    let configured = unsafe {
        SetInformationJobObject(
            job,
            9,
            (&mut limits as *mut ExtendedLimitInformation).cast(),
            std::mem::size_of::<ExtendedLimitInformation>() as u32,
        )
    } != 0;
    if !configured {
        unsafe { CloseHandle(job) };
        bail!("could not configure command kill-on-close job object");
    }
    Ok(job as usize)
}

#[cfg(windows)]
fn spawn_suspended_in_job(command: &mut Command, job: usize) -> Result<Child> {
    use std::ffi::c_void;
    use std::os::windows::process::CommandExt;

    const CREATE_SUSPENDED: u32 = 0x0000_0004;
    const PROCESS_TERMINATE: u32 = 0x0001;
    const PROCESS_SET_QUOTA: u32 = 0x0100;
    const PROCESS_SUSPEND_RESUME: u32 = 0x0800;

    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn AssignProcessToJobObject(job: *mut c_void, process: *mut c_void) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }
    #[link(name = "ntdll")]
    extern "system" {
        fn NtResumeProcess(process: *mut c_void) -> i32;
    }

    command.creation_flags(CREATE_SUSPENDED);
    let mut child = command.spawn()?;
    let process = unsafe {
        OpenProcess(
            PROCESS_TERMINATE | PROCESS_SET_QUOTA | PROCESS_SUSPEND_RESUME,
            0,
            child.id(),
        )
    };
    let assigned =
        !process.is_null() && unsafe { AssignProcessToJobObject(job as *mut c_void, process) } != 0;
    let resumed = assigned && unsafe { NtResumeProcess(process) } >= 0;
    if !process.is_null() {
        unsafe { CloseHandle(process) };
    }
    if !resumed {
        let _ = child.kill();
        let _ = child.wait();
        bail!("could not assign command process to its kill-on-close job object");
    }
    Ok(child)
}

#[cfg(windows)]
fn terminate_job_handle(handle: usize) -> Result<()> {
    #[link(name = "kernel32")]
    extern "system" {
        fn TerminateJobObject(job: *mut std::ffi::c_void, exit_code: u32) -> i32;
    }
    if unsafe { TerminateJobObject(handle as *mut std::ffi::c_void, 1) } == 0 {
        bail!("could not terminate a launcher-owned command job");
    }
    Ok(())
}

#[cfg(windows)]
unsafe fn close_job_handle(handle: usize) {
    #[link(name = "kernel32")]
    extern "system" {
        fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
    }
    CloseHandle(handle as *mut std::ffi::c_void);
}

#[cfg(windows)]
fn launch_in_job(command: &mut Command) -> Result<Child> {
    use std::{ffi::c_void, os::windows::process::CommandExt};

    const CREATE_SUSPENDED: u32 = 0x0000_0004;
    const PROCESS_TERMINATE: u32 = 0x0001;
    const PROCESS_SET_QUOTA: u32 = 0x0100;
    const PROCESS_SUSPEND_RESUME: u32 = 0x0800;
    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x0000_2000;

    #[repr(C)]
    struct BasicLimitInformation {
        per_process_user_time_limit: i64,
        per_job_user_time_limit: i64,
        limit_flags: u32,
        minimum_working_set_size: usize,
        maximum_working_set_size: usize,
        active_process_limit: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
    }
    #[repr(C)]
    struct IoCounters {
        read_operation_count: u64,
        write_operation_count: u64,
        other_operation_count: u64,
        read_transfer_count: u64,
        write_transfer_count: u64,
        other_transfer_count: u64,
    }
    #[repr(C)]
    struct ExtendedLimitInformation {
        basic: BasicLimitInformation,
        io: IoCounters,
        process_memory_limit: usize,
        job_memory_limit: usize,
        peak_process_memory_used: usize,
        peak_job_memory_used: usize,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateJobObjectW(attributes: *mut c_void, name: *const u16) -> *mut c_void;
        fn SetInformationJobObject(
            job: *mut c_void,
            class: u32,
            info: *const c_void,
            size: u32,
        ) -> i32;
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn AssignProcessToJobObject(job: *mut c_void, process: *mut c_void) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }
    #[link(name = "ntdll")]
    extern "system" {
        fn NtResumeProcess(process: *mut c_void) -> i32;
    }

    command.creation_flags(CREATE_SUSPENDED);
    let mut child = command.spawn()?;
    let job = unsafe { CreateJobObjectW(std::ptr::null_mut(), std::ptr::null()) };
    if job.is_null() {
        let _ = child.kill();
        let _ = child.wait();
        bail!("could not create Studio process job object");
    }
    let mut limits = ExtendedLimitInformation {
        basic: BasicLimitInformation {
            per_process_user_time_limit: 0,
            per_job_user_time_limit: 0,
            limit_flags: JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            minimum_working_set_size: 0,
            maximum_working_set_size: 0,
            active_process_limit: 0,
            affinity: 0,
            priority_class: 0,
            scheduling_class: 0,
        },
        io: IoCounters {
            read_operation_count: 0,
            write_operation_count: 0,
            other_operation_count: 0,
            read_transfer_count: 0,
            write_transfer_count: 0,
            other_transfer_count: 0,
        },
        process_memory_limit: 0,
        job_memory_limit: 0,
        peak_process_memory_used: 0,
        peak_job_memory_used: 0,
    };
    let configured = unsafe {
        SetInformationJobObject(
            job,
            9,
            (&mut limits as *mut ExtendedLimitInformation).cast(),
            std::mem::size_of::<ExtendedLimitInformation>() as u32,
        )
    } != 0;
    let process = unsafe {
        OpenProcess(
            PROCESS_TERMINATE | PROCESS_SET_QUOTA | PROCESS_SUSPEND_RESUME,
            0,
            child.id(),
        )
    };
    let assigned =
        configured && !process.is_null() && unsafe { AssignProcessToJobObject(job, process) } != 0;
    let resumed = assigned && unsafe { NtResumeProcess(process) } >= 0;
    if !process.is_null() {
        unsafe { CloseHandle(process) };
    }
    if !resumed {
        unsafe { CloseHandle(job) };
        let _ = child.kill();
        let _ = child.wait();
        bail!("could not assign Studio process to kill-on-close job object");
    }
    let jobs = STUDIO_JOB.get_or_init(|| Mutex::new(None));
    let mut current = jobs.lock().unwrap();
    if current.is_some() {
        unsafe { CloseHandle(job) };
        let _ = child.kill();
        let _ = child.wait();
        bail!("launcher already owns a Studio job object");
    }
    *current = Some(job as usize);
    Ok(child)
}

#[cfg(not(windows))]
fn launch_in_job(command: &mut Command) -> Result<Child> {
    command.spawn().context("start Studio process")
}

#[cfg(windows)]
fn terminate_studio_job() {
    if let Some(jobs) = STUDIO_JOB.get() {
        if let Some(job) = *jobs.lock().unwrap() {
            #[link(name = "kernel32")]
            extern "system" {
                fn TerminateJobObject(job: *mut std::ffi::c_void, exit_code: u32) -> i32;
            }
            unsafe { TerminateJobObject(job as *mut std::ffi::c_void, 1) };
        }
    }
}

#[cfg(not(windows))]
fn terminate_studio_job() {}

#[cfg(windows)]
fn close_studio_job() {
    if let Some(jobs) = STUDIO_JOB.get() {
        if let Some(job) = jobs.lock().unwrap().take() {
            #[link(name = "kernel32")]
            extern "system" {
                fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
            }
            unsafe { CloseHandle(job as *mut std::ffi::c_void) };
        }
    }
}

#[cfg(not(windows))]
fn close_studio_job() {}

fn wait_health(port: u16) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(40);
    let addr = format!("127.0.0.1:{port}");
    while Instant::now() < deadline {
        if is_running()
            && addr
                .to_socket_addrs()?
                .any(|a| TcpStream::connect_timeout(&a, Duration::from_millis(300)).is_ok())
        {
            return Ok(());
        }
        if !is_running() {
            bail!("Studio process exited before localhost became ready");
        }
        thread::sleep(Duration::from_millis(300));
    }
    bail!("Studio did not become ready at http://127.0.0.1:{port} within 40 seconds")
}

fn port_is_listening(port: u16) -> bool {
    TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        Duration::from_millis(250),
    )
    .is_ok()
}

fn installed(settings: &Settings) -> bool {
    if settings.validate().is_err() || !settings.is_owned() {
        return false;
    }
    let git = settings.app_dir().join("source").join(".git");
    let python = settings
        .app_dir()
        .join("unsloth_studio")
        .join("Scripts")
        .join("python.exe");
    crate::settings::reject_reparse_components(&git).is_ok()
        && crate::settings::reject_reparse_components(&python).is_ok()
        && git.exists()
        && python.is_file()
}

fn check_updates(settings: &Settings) -> Result<(Option<String>, Option<String>, String)> {
    settings.validate()?;
    if !settings.is_owned() {
        return Ok((None, None, "unknown".into()));
    }
    let source = settings.app_dir().join("source");
    if !installed(settings) || !source.join(".git").exists() {
        return Ok((None, None, "unknown".into()));
    }
    crate::settings::reject_reparse_tree(&source)?;
    let configured_remote = git_output(
        settings,
        &["remote", "get-url", "origin"],
        "verify source remote",
        Duration::from_secs(30),
    )?;
    if configured_remote != UPSTREAM_URL {
        bail!("source origin does not match the fixed official Unsloth HTTPS repository");
    }
    let installed_commit = git_output(
        settings,
        &["rev-parse", "HEAD"],
        "read installed commit",
        Duration::from_secs(30),
    )?;
    let remote = git_output(
        settings,
        &["ls-remote", UPSTREAM_URL, "refs/heads/main"],
        "check official update",
        Duration::from_secs(15),
    )?;
    let upstream = remote
        .split_whitespace()
        .next()
        .context("upstream returned no main commit")?
        .to_owned();
    validate_commit(&installed_commit)?;
    validate_commit(&upstream)?;
    let status = if installed_commit.eq_ignore_ascii_case(&upstream) {
        "current"
    } else {
        "available"
    };
    Ok((Some(installed_commit), Some(upstream), status.into()))
}

pub fn prepare_removal(s: &Settings, scope: &str) -> Result<Removal> {
    if is_running() {
        bail!("stop Studio before removing managed files");
    }
    let paths = safe_owned_paths(s, scope)?;
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let digest = removal_digest(s, scope, &paths)?;
    Ok(Removal {
        token: format!("{}:{}:{}:{}", uuid::Uuid::new_v4(), stamp, scope, digest),
        paths: paths.iter().filter(|p| p.exists()).map(|p| p.display().to_string()).collect(),
        preserved_paths: preserved_paths(s, scope)?,
        warning: "This permanently deletes only listed launcher-owned source/runtime paths. The product root, ownership marker, unlisted app data, and external Hugging Face data remain preserved.".into(),
    })
}

pub fn remove(s: &Settings, removal: &Removal, token: &str) -> Result<()> {
    if removal.token != token {
        bail!("confirmation token is invalid or already consumed");
    }
    if is_running() {
        bail!("stop Studio before removing managed files");
    }
    let parts: Vec<_> = token.split(':').collect();
    if parts.len() != 4 || uuid::Uuid::parse_str(parts[0]).is_err() {
        bail!("invalid confirmation token");
    }
    let ts: u64 = parts[1].parse()?;
    if SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_secs()
        .saturating_sub(ts)
        > 300
    {
        bail!("confirmation token expired; prepare removal again");
    }
    let scope = parts[2];
    if !matches!(scope, "runtime" | "all_owned_data") {
        bail!("invalid removal scope in confirmation token");
    }
    let paths = safe_owned_paths(s, scope)?;
    let digest = removal_digest(s, scope, &paths)?;
    if digest != parts[3] {
        bail!("confirmation token belongs to different physical product root or deletion scope");
    }
    if paths
        .iter()
        .filter(|p| p.exists())
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        != removal.paths
    {
        bail!("removal paths changed; prepare removal again");
    }
    let confirmed_paths = paths.clone();
    let consumed = CONSUMED_REMOVALS.get_or_init(|| Mutex::new(HashSet::new()));
    if !consumed.lock().unwrap().insert(token.to_owned()) {
        bail!("confirmation token is invalid or already consumed");
    }
    for path in paths {
        let current_paths = safe_owned_paths(s, scope)?;
        if current_paths != confirmed_paths || removal_digest(s, scope, &current_paths)? != parts[3]
        {
            bail!("physical removal paths changed; prepare removal again");
        }
        if path.exists() {
            reject_tree_reparse_points(&path)?;
            fs::remove_dir_all(path)?;
        }
    }
    Ok(())
}

fn removal_digest(settings: &Settings, scope: &str, paths: &[PathBuf]) -> Result<String> {
    let root = crate::settings::physical_path(&settings.root)?;
    let identities = paths
        .iter()
        .map(|path| crate::settings::physical_path(path))
        .collect::<Result<Vec<_>>>()?;
    let payload = serde_json::to_vec(&(settings, scope, root, identities))?;
    Ok(format!("{:x}", sha2::Sha256::digest(payload)))
}

fn preserved_paths(settings: &Settings, scope: &str) -> Result<Vec<String>> {
    let mut paths = Vec::new();
    paths.push(settings.root.display().to_string());
    let default_hf = settings.root.join("huggingface");
    let hf_identity = crate::settings::physical_path(&settings.hf_home)?;
    let default_hf_identity = crate::settings::physical_path(&default_hf)?;
    let hf_is_default = crate::settings::physical_path_eq(&hf_identity, &default_hf_identity)?;
    if (scope != "all_owned_data" || !hf_is_default) && settings.hf_home.exists() {
        paths.push(settings.hf_home.display().to_string());
    }
    if settings.app_dir().exists() {
        paths.push(settings.app_dir().display().to_string());
    }
    let user_data = settings.root.join("user-data");
    if user_data.exists() {
        paths.push(user_data.display().to_string());
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

fn reject_tree_reparse_points(path: &std::path::Path) -> Result<()> {
    crate::settings::reject_reparse_tree(path)
}

#[derive(Serialize, Deserialize)]
struct ChildRecord {
    pid: u32,
    started: u64,
}
fn child_record() -> Result<PathBuf> {
    Ok(crate::settings::config_dir()?.join("owned-studio-process.json"))
}
fn remember_child(mut child: Child) -> Result<()> {
    let started = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let mut owned = STUDIO_CHILD
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap();
    if owned.is_some() {
        let _ = child.kill();
        let _ = child.wait();
        close_studio_job();
        bail!("launcher already owns a Studio process");
    }
    let record = (|| -> Result<()> {
        let path = child_record()?;
        fs::create_dir_all(path.parent().context("process record has no parent")?)?;
        fs::write(
            path,
            serde_json::to_vec(&ChildRecord {
                pid: child.id(),
                started,
            })?,
        )?;
        Ok(())
    })();
    if let Err(error) = record {
        let _ = child.kill();
        let _ = child.wait();
        close_studio_job();
        return Err(error);
    }
    *owned = Some(child);
    Ok(())
}
pub fn is_running() -> bool {
    let Some(child) = STUDIO_CHILD.get() else {
        return false;
    };
    let mut child = child.lock().unwrap();
    let Some(process) = child.as_mut() else {
        return false;
    };
    match process.try_wait() {
        Ok(None) => true,
        Ok(Some(_)) => {
            *child = None;
            close_studio_job();
            let _ = child_record().and_then(|p| fs::remove_file(p).map_err(Into::into));
            false
        }
        Err(_) => true,
    }
}
pub fn stop_owned() -> Result<()> {
    let Some(child) = STUDIO_CHILD.get() else {
        close_studio_job();
        if let Ok(record) = child_record() {
            let _ = fs::remove_file(record);
        }
        return Ok(());
    };
    let mut child = child.lock().unwrap();
    let Some(process) = child.as_mut() else {
        close_studio_job();
        if let Ok(record) = child_record() {
            let _ = fs::remove_file(record);
        }
        return Ok(());
    };
    if process.try_wait()?.is_none() {
        terminate_studio_job();
        let _ = process.kill();
        let _ = process.wait();
    }
    *child = None;
    close_studio_job();
    let _ = fs::remove_file(child_record()?);
    Ok(())
}

static STUDIO_CHILD: OnceLock<Mutex<Option<Child>>> = OnceLock::new();
static CONSUMED_REMOVALS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
static COMMANDS_CANCELLED: AtomicBool = AtomicBool::new(false);
#[cfg(windows)]
static COMMAND_JOBS: OnceLock<Mutex<HashMap<u64, usize>>> = OnceLock::new();
#[cfg(windows)]
static NEXT_COMMAND_JOB: AtomicU64 = AtomicU64::new(1);
#[cfg(windows)]
static STUDIO_JOB: OnceLock<Mutex<Option<usize>>> = OnceLock::new();
