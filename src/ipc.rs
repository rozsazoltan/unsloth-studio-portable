use crate::{
    lifecycle::{self, SharedState},
    settings::Settings,
    tray,
};
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{atomic::AtomicBool, Arc},
    thread,
    time::{Duration, Instant},
};

const MAX_FRAME: usize = 1024 * 1024;

#[derive(Deserialize)]
struct Request {
    version: u8,
    id: u64,
    command: CommandEnvelope,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum CommandEnvelope {
    GetSnapshot,
    SaveSettings {
        settings: Settings,
    },
    Install {
        #[serde(default)]
        upstream_setup_approved: bool,
    },
    Update {
        #[serde(default)]
        upstream_setup_approved: bool,
    },
    Start,
    Stop,
    OpenStudio,
    CheckUpdates,
    PrepareRemoval {
        scope: String,
    },
    ConfirmRemoval {
        token: String,
    },
    Exit,
}

pub fn launch_ui(state: SharedState, exit_requested: Arc<AtomicBool>) -> Result<Child> {
    let exe = extract_payload()?;
    let full_pipe = format!(r"\\.\pipe\unsloth-studio-{}", uuid::Uuid::new_v4());
    #[cfg(windows)]
    let pipe = create_pipe(&full_pipe)?;
    let child = Command::new(exe)
        .arg("--pipe")
        .arg(&full_pipe)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("start WinUI companion")?;
    #[cfg(windows)]
    {
        let expected_pid = child.id();
        thread::Builder::new()
            .name("unsloth-ipc".into())
            .spawn(move || {
                if let Err(error) = serve_pipe(pipe, expected_pid, state, exit_requested) {
                    eprintln!("Launcher IPC: {error:#}");
                }
            })?;
    }
    #[cfg(not(windows))]
    {
        let _ = (state, exit_requested);
        bail!("WinUI companion is Windows-only");
    }
    Ok(child)
}

fn extract_payload() -> Result<PathBuf> {
    let bytes = include_bytes!(concat!(env!("OUT_DIR"), "/native_payload.bin"));
    let base = PathBuf::from(std::env::var_os("LOCALAPPDATA").context("LOCALAPPDATA is missing")?)
        .join("Unsloth Studio Desktop")
        .join("native");
    reject_reparse_path(&base)?;
    fs::create_dir_all(&base)?;
    let digest = Sha256::digest(bytes);
    let id = digest
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let final_dir = base.join(id);
    reject_reparse_path(&final_dir)?;
    let exe = final_dir.join("StudioUI.exe");
    if exe.is_file() {
        verify_cached_payload(&final_dir, bytes)?;
        return Ok(exe);
    }

    let mut cursor = std::io::Cursor::new(bytes);
    let count = read_u32(&mut cursor)?;
    if count == 0 {
        bail!("native UI payload is missing; build the self-contained StudioUI payload first");
    }
    let staging = base.join(format!("stage-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&staging)?;
    for _ in 0..count {
        let len = read_u32(&mut cursor)? as usize;
        if len == 0 || len > 4096 {
            bail!("invalid native payload path length");
        }
        let mut name = vec![0; len];
        cursor.read_exact(&mut name)?;
        let name = String::from_utf8(name)?;
        let relative = Path::new(&name);
        if relative.is_absolute()
            || relative
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
        {
            bail!("unsafe native payload path: {name}");
        }
        let size = read_u64(&mut cursor)?;
        if size > 256 * 1024 * 1024 {
            bail!("native payload file exceeds limit");
        }
        let mut expected = [0u8; 32];
        cursor.read_exact(&mut expected)?;
        let mut data = vec![0; size as usize];
        cursor.read_exact(&mut data)?;
        if Sha256::digest(&data).as_slice() != expected {
            bail!("native payload hash mismatch: {name}");
        }
        let destination = staging.join(relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(destination, data)?;
    }
    verify_payload_dir(&staging)?;
    fs::rename(&staging, &final_dir).context("publish immutable native payload")?;
    Ok(exe)
}

fn verify_payload_dir(dir: &Path) -> Result<()> {
    reject_reparse_path(dir)?;
    let required = [
        "StudioUI.exe",
        "Microsoft.WindowsAppRuntime.Bootstrap.dll",
        "Microsoft.ui.xaml.dll",
    ];
    for file in required {
        if !dir.join(file).is_file() {
            bail!("native payload is incomplete; missing {file}; a standalone debug EXE is not self-contained");
        }
    }
    Ok(())
}

fn verify_cached_payload(dir: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Cursor;
    let mut cursor = Cursor::new(bytes);
    let count = read_u32(&mut cursor)?;
    if count == 0 {
        bail!("native UI payload is missing; build the self-contained StudioUI payload first");
    }
    for _ in 0..count {
        let len = read_u32(&mut cursor)? as usize;
        if len == 0 || len > 4096 {
            bail!("invalid native payload path length");
        }
        let mut name = vec![0; len];
        cursor.read_exact(&mut name)?;
        let name = String::from_utf8(name)?;
        let relative = Path::new(&name);
        if relative.is_absolute()
            || relative
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            bail!("unsafe native payload path: {name}");
        }
        let size = read_u64(&mut cursor)? as usize;
        if size > 256 * 1024 * 1024 {
            bail!("native payload file exceeds limit");
        }
        let mut expected = [0; 32];
        cursor.read_exact(&mut expected)?;
        let mut source = vec![0; size];
        cursor.read_exact(&mut source)?;
        let path = dir.join(relative);
        reject_reparse_path(&path)?;
        let cached =
            fs::read(&path).with_context(|| format!("verify cached native payload {name}"))?;
        if Sha256::digest(&source).as_slice() != expected
            || Sha256::digest(&cached).as_slice() != expected
        {
            bail!("cached native payload hash mismatch: {name}");
        }
    }
    if cursor.position() != bytes.len() as u64 {
        bail!("native payload has trailing data");
    }
    verify_payload_dir(dir)
}

#[cfg(windows)]
fn reject_reparse_path(path: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::{GetFileAttributesW, FILE_ATTRIBUTE_REPARSE_POINT};
    let mut current = PathBuf::new();
    for part in path.components() {
        current.push(part.as_os_str());
        if !current.exists() {
            continue;
        }
        let wide: Vec<u16> = current.as_os_str().encode_wide().chain(Some(0)).collect();
        let attrs = unsafe { GetFileAttributesW(windows::core::PCWSTR(wide.as_ptr())) };
        if attrs != u32::MAX && attrs & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
            bail!(
                "reparse point in native payload path: {}",
                current.display()
            );
        }
    }
    Ok(())
}

#[cfg(not(windows))]
fn reject_reparse_path(path: &Path) -> Result<()> {
    let mut current = PathBuf::new();
    for part in path.components() {
        current.push(part.as_os_str());
        if current.exists() && fs::symlink_metadata(&current)?.file_type().is_symlink() {
            bail!("symlink in native payload path: {}", current.display());
        }
    }
    Ok(())
}

fn read_u32(r: &mut impl Read) -> Result<u32> {
    let mut b = [0; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}
fn read_u64(r: &mut impl Read) -> Result<u64> {
    let mut b = [0; 8];
    r.read_exact(&mut b)?;
    Ok(u64::from_le_bytes(b))
}

#[cfg(windows)]
fn create_pipe(name: &str) -> Result<Pipe> {
    use std::{ffi::OsStr, os::windows::ffi::OsStrExt};
    use windows::{
        core::PCWSTR,
        Win32::{
            Foundation::{LocalFree, INVALID_HANDLE_VALUE},
            Security::Authorization::{
                ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
            },
            Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES},
            Storage::FileSystem::PIPE_ACCESS_DUPLEX,
            System::Pipes::{
                CreateNamedPipeW, PIPE_NOWAIT, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE,
            },
        },
    };
    let name: Vec<u16> = OsStr::new(name).encode_wide().chain(Some(0)).collect();
    let sddl: Vec<u16> = OsStr::new("D:P(A;;GA;;;OW)")
        .encode_wide()
        .chain(Some(0))
        .collect();
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(sddl.as_ptr()),
            SDDL_REVISION_1,
            &mut descriptor,
            None,
        )?;
    }
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: false.into(),
    };
    let handle = unsafe {
        CreateNamedPipeW(
            PCWSTR(name.as_ptr()),
            PIPE_ACCESS_DUPLEX,
            PIPE_TYPE_BYTE | PIPE_NOWAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            (MAX_FRAME + 4) as u32,
            (MAX_FRAME + 4) as u32,
            0,
            Some(&attributes),
        )
    };
    unsafe {
        LocalFree(windows::Win32::Foundation::HLOCAL(descriptor.0));
    }
    if handle == INVALID_HANDLE_VALUE {
        bail!("CreateNamedPipeW failed");
    }
    Ok(Pipe(handle))
}

#[cfg(windows)]
fn serve_pipe(
    pipe: Pipe,
    expected_pid: u32,
    state: SharedState,
    exit_requested: Arc<AtomicBool>,
) -> Result<()> {
    use windows::Win32::{
        Foundation::GetLastError,
        System::Pipes::{
            ConnectNamedPipe, GetNamedPipeClientProcessId, PeekNamedPipe, SetNamedPipeHandleState,
            PIPE_READMODE_BYTE, PIPE_WAIT,
        },
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let connected = unsafe { ConnectNamedPipe(pipe.0, None) };
        if connected.is_ok() {
            break;
        }
        let code = unsafe { GetLastError().0 };
        if code == 535 {
            // ERROR_PIPE_CONNECTED
            break;
        }
        if Instant::now() >= deadline {
            bail!("native UI did not connect to its private pipe within ten seconds");
        }
        thread::sleep(Duration::from_millis(40));
    }
    let mut client_pid = 0;
    unsafe {
        GetNamedPipeClientProcessId(pipe.0, &mut client_pid)?;
    }
    if client_pid != expected_pid {
        bail!("native pipe client PID did not match launched companion");
    }
    let mode = PIPE_READMODE_BYTE | PIPE_WAIT;
    unsafe {
        SetNamedPipeHandleState(pipe.0, Some(&mode), None, None)?;
    }
    let mut stream = PipeIo(&pipe);
    send_snapshot(&mut stream, &state)?;
    let mut removal = None;
    let mut last_snapshot = Instant::now();
    loop {
        if exit_requested.load(std::sync::atomic::Ordering::Acquire) {
            return Ok(());
        }
        let mut available = 0;
        let peek = unsafe { PeekNamedPipe(pipe.0, None, 0, None, Some(&mut available), None) };
        if peek.is_err() {
            break;
        }
        if available >= 4 {
            let mut prefix = [0; 4];
            stream.read_exact(&mut prefix)?;
            let size = u32::from_le_bytes(prefix) as usize;
            if size == 0 || size > MAX_FRAME {
                bail!("invalid IPC frame length");
            }
            let mut payload = vec![0; size];
            stream.read_exact(&mut payload)?;
            let request: Request =
                serde_json::from_slice(&payload).context("malformed IPC JSON request")?;
            if request.version != 1 {
                bail!("unsupported IPC protocol version");
            }
            let result = dispatch(request.command, &state, &mut removal, &exit_requested);
            let (ok, error) = match result {
                Ok(()) => (true, serde_json::Value::Null),
                Err(error) => (
                    false,
                    serde_json::json!({"code":"request_failed","message":format!("{error:#}")}),
                ),
            };
            write_frame(
                &mut stream,
                &serde_json::json!({"version":1,"type":"reply","id":request.id,"ok":ok,"error":error}),
            )?;
            send_snapshot(&mut stream, &state)?;
            last_snapshot = Instant::now();
        } else if last_snapshot.elapsed() >= Duration::from_secs(1) {
            send_snapshot(&mut stream, &state)?;
            last_snapshot = Instant::now();
        } else {
            thread::sleep(Duration::from_millis(40));
        }
    }
    Ok(())
}

fn dispatch(
    command: CommandEnvelope,
    state: &SharedState,
    removal: &mut Option<lifecycle::Removal>,
    exit_requested: &AtomicBool,
) -> Result<()> {
    match command {
        CommandEnvelope::GetSnapshot => Ok(()),
        CommandEnvelope::SaveSettings { settings } => {
            settings.validate()?;
            let reservation = uuid::Uuid::new_v4().to_string();
            let old = {
                let mut current = state.lock().unwrap();
                current.refresh();
                if current.operation.is_some()
                    || lifecycle::is_running()
                    || matches!(
                        current.phase,
                        lifecycle::Phase::Running
                            | lifecycle::Phase::Starting
                            | lifecycle::Phase::Stopping
                            | lifecycle::Phase::Removing
                    )
                {
                    bail!("settings cannot change during an operation or while Studio is running");
                }
                let old = current.settings.clone();
                if old.root != settings.root && old.has_active_content()? {
                    bail!("cannot change product root while launcher-owned source or runtime content exists");
                }
                current.operation = Some(lifecycle::Operation {
                    id: reservation.clone(),
                    kind: "save_settings".into(),
                    detail: "Saving settings".into(),
                });
                current.revision += 1;
                old
            };
            let result = (|| -> Result<()> {
                tray::apply_integrations(&old, &settings)?;
                if let Err(save_error) = settings.save() {
                    let rollback = tray::apply_integrations(&settings, &old);
                    return match rollback {
                        Ok(()) => Err(save_error).context("save settings; integrations were rolled back"),
                        Err(rollback_error) => Err(save_error).context(format!(
                            "save settings failed and integration rollback also failed: {rollback_error:#}"
                        )),
                    };
                }
                Ok(())
            })();
            let mut current = state.lock().unwrap();
            if current
                .operation
                .as_ref()
                .is_some_and(|op| op.id == reservation)
            {
                current.operation = None;
            }
            if let Err(error) = result {
                current.revision += 1;
                return Err(error);
            }
            current.settings = settings;
            current.removal = None;
            current.revision += 1;
            *removal = None;
            Ok(())
        }
        CommandEnvelope::Install {
            upstream_setup_approved,
        } => lifecycle::run_operation_approved(
            state.clone(),
            "install".into(),
            upstream_setup_approved,
        ),
        CommandEnvelope::Update {
            upstream_setup_approved,
        } => lifecycle::run_operation_approved(
            state.clone(),
            "update".into(),
            upstream_setup_approved,
        ),
        CommandEnvelope::Start => lifecycle::run_operation(state.clone(), "start".into()),
        CommandEnvelope::Stop => lifecycle::run_operation(state.clone(), "stop".into()),
        CommandEnvelope::OpenStudio => open_studio(state),
        CommandEnvelope::CheckUpdates => {
            lifecycle::run_operation(state.clone(), "check_updates".into())
        }
        CommandEnvelope::PrepareRemoval { scope } => {
            let mut current = state.lock().unwrap();
            if current.operation.is_some()
                || lifecycle::is_running()
                || matches!(
                    current.phase,
                    lifecycle::Phase::Running
                        | lifecycle::Phase::Starting
                        | lifecycle::Phase::Stopping
                        | lifecycle::Phase::Removing
                )
            {
                bail!("stop Studio and wait for all operations before preparing removal");
            }
            let prepared = lifecycle::prepare_removal(&current.settings, &scope)?;
            *removal = Some(prepared.clone());
            current.removal = Some(prepared);
            current.revision += 1;
            Ok(())
        }
        CommandEnvelope::ConfirmRemoval { token } => {
            let (settings, prepared, reservation) = {
                let mut current = state.lock().unwrap();
                if current.operation.is_some()
                    || lifecycle::is_running()
                    || matches!(
                        current.phase,
                        lifecycle::Phase::Running
                            | lifecycle::Phase::Starting
                            | lifecycle::Phase::Stopping
                            | lifecycle::Phase::Removing
                    )
                {
                    bail!("stop Studio and wait for all operations before removal");
                }
                let prepared = removal.take().context("no active removal confirmation")?;
                current.removal = None;
                let reservation = uuid::Uuid::new_v4().to_string();
                current.operation = Some(lifecycle::Operation {
                    id: reservation.clone(),
                    kind: "remove".into(),
                    detail: "Removing launcher-owned files".into(),
                });
                current.phase = lifecycle::Phase::Removing;
                current.revision += 1;
                (current.settings.clone(), prepared, reservation)
            };
            let result = lifecycle::remove(&settings, &prepared, &token);
            let mut current = state.lock().unwrap();
            if current
                .operation
                .as_ref()
                .is_some_and(|op| op.id == reservation)
            {
                current.operation = None;
            }
            current.phase = if result.is_ok() {
                lifecycle::Phase::Absent
            } else {
                lifecycle::Phase::Failed
            };
            current.last_error = result.as_ref().err().map(|error| lifecycle::ErrorInfo {
                code: "removal_failed".into(),
                message: format!("{error:#}"),
            });
            current.removal = None;
            current.revision += 1;
            result
        }
        CommandEnvelope::Exit => {
            exit_requested.store(true, std::sync::atomic::Ordering::Release);
            Ok(())
        }
    }
}

fn open_studio(state: &SharedState) -> Result<()> {
    let current = state.lock().unwrap();
    if !matches!(current.phase, lifecycle::Phase::Running) {
        bail!("Studio is not running; use Start first");
    }
    let url = format!("http://127.0.0.1:{}", current.settings.port);
    drop(current);
    #[cfg(windows)]
    {
        use std::ffi::OsStr;
        use std::os::windows::ffi::OsStrExt;
        use windows::Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL};
        let url: Vec<u16> = OsStr::new(&url).encode_wide().chain(Some(0)).collect();
        let operation = unsafe {
            ShellExecuteW(
                None,
                windows::core::PCWSTR::null(),
                windows::core::PCWSTR(url.as_ptr()),
                None,
                None,
                SW_SHOWNORMAL,
            )
        };
        if operation.0 as isize <= 32 {
            bail!("could not open Studio URL in the default browser");
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = url;
        bail!("Opening Studio requires Windows")
    }
}

fn send_snapshot(stream: &mut PipeIo<'_>, state: &SharedState) -> Result<()> {
    let snapshot =
        serde_json::json!({"version":1,"type":"snapshot","state":state.lock().unwrap().snapshot()});
    write_frame(stream, &snapshot)
}

#[cfg(windows)]
struct Pipe(windows::Win32::Foundation::HANDLE);
#[cfg(windows)]
unsafe impl Send for Pipe {}
#[cfg(windows)]
impl Drop for Pipe {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

#[cfg(windows)]
struct PipeIo<'a>(&'a Pipe);
#[cfg(windows)]
impl Read for PipeIo<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        use windows::Win32::Storage::FileSystem::ReadFile;
        let mut count = 0;
        unsafe {
            ReadFile(self.0 .0, Some(buffer), Some(&mut count), None)
                .map_err(|e| std::io::Error::other(e.to_string()))?;
        }
        Ok(count as usize)
    }
}
#[cfg(windows)]
impl Write for PipeIo<'_> {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        use windows::Win32::Storage::FileSystem::WriteFile;
        let mut count = 0;
        unsafe {
            WriteFile(self.0 .0, Some(buffer), Some(&mut count), None)
                .map_err(|e| std::io::Error::other(e.to_string()))?;
        }
        Ok(count as usize)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(windows)]
fn write_frame(writer: &mut impl Write, value: &serde_json::Value) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > MAX_FRAME {
        bail!("IPC response exceeds frame limit");
    }
    writer.write_all(&(bytes.len() as u32).to_le_bytes())?;
    writer.write_all(&bytes)?;
    Ok(())
}

#[cfg(not(windows))]
fn write_frame(_: &mut impl Write, _: &serde_json::Value) -> Result<()> {
    bail!("IPC is Windows-only")
}
