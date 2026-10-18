use crate::{
    ipc,
    lifecycle::{self, SharedState, State},
    settings::Settings,
};
use anyhow::{bail, Context, Result};
use std::{
    process::Child,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

const MENU_OPEN: u32 = 1001;
const MENU_START: u32 = 1002;
const MENU_STOP: u32 = 1003;
const MENU_EXIT: u32 = 1004;
const WM_TRAY: u32 = 0x8001;
const WM_ACTIVATE_HOST: u32 = 0x8002;
const TIMER_ID: usize = 1;

pub fn run(start_in_tray: bool) -> Result<()> {
    let settings = Settings::load_or_default().context("load launcher settings")?;
    settings.validate()?;
    if start_in_tray && !settings.tray_enabled {
        bail!("tray mode is disabled in settings; enable tray mode from the native UI first");
    }
    let Some(_instance) = acquire_instance()? else {
        return Ok(());
    };
    let state = Arc::new(Mutex::new(State::new(settings.clone())));
    let open_requested = Arc::new(AtomicBool::new(false));
    let exit_requested = Arc::new(AtomicBool::new(false));
    lifecycle::run_operation(state.clone(), "check_updates".into())?;
    let mut host = Host {
        state,
        open_requested,
        exit_requested,
        ui: None,
        tray_enabled: settings.tray_enabled,
        exit_after_stop: false,
        exit_wait_started: None,
        exit_stop_attempted: false,
    };
    let result = run_message_window(&mut host, start_in_tray);
    if let Err(error) = &result {
        if !start_in_tray {
            show_error(&format!(
                "Could not start Unsloth Studio Desktop UI:\n\n{error:#}"
            ));
        }
    }
    result
}

struct Host {
    state: SharedState,
    open_requested: Arc<AtomicBool>,
    exit_requested: Arc<AtomicBool>,
    ui: Option<Child>,
    tray_enabled: bool,
    exit_after_stop: bool,
    exit_wait_started: Option<Instant>,
    exit_stop_attempted: bool,
}

fn activate_single_instance() {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows::{
        core::PCWSTR,
        Win32::{
            Foundation::HWND,
            UI::WindowsAndMessaging::{FindWindowExW, PostMessageW, HWND_MESSAGE},
        },
    };
    let class: Vec<u16> = OsStr::new("UnslothStudioDesktopHost")
        .encode_wide()
        .chain(Some(0))
        .collect();
    for _ in 0..40 {
        if let Ok(window) = unsafe {
            FindWindowExW(
                HWND_MESSAGE,
                HWND::default(),
                PCWSTR(class.as_ptr()),
                PCWSTR::null(),
            )
        } {
            if window.is_invalid() {
                continue;
            }
            unsafe {
                let _ = PostMessageW(
                    window,
                    WM_ACTIVATE_HOST,
                    windows::Win32::Foundation::WPARAM(0),
                    windows::Win32::Foundation::LPARAM(0),
                );
            }
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn acquire_instance() -> Result<Option<InstanceHandle>> {
    use std::{ffi::OsStr, os::windows::ffi::OsStrExt};
    use windows::{
        core::PCWSTR,
        Win32::{
            Foundation::{CloseHandle, ERROR_ALREADY_EXISTS},
            System::Threading::CreateMutexW,
        },
    };
    let sid = current_user_sid()?;
    let name: Vec<u16> = OsStr::new(&format!("Local\\UnslothStudioDesktop-{sid}"))
        .encode_wide()
        .chain(Some(0))
        .collect();
    let handle = unsafe { CreateMutexW(None, false, PCWSTR(name.as_ptr()))? };
    if unsafe { windows::Win32::Foundation::GetLastError() } == ERROR_ALREADY_EXISTS {
        unsafe {
            let _ = CloseHandle(handle);
        }
        activate_single_instance();
        return Ok(None);
    }
    Ok(Some(InstanceHandle(handle)))
}

struct InstanceHandle(windows::Win32::Foundation::HANDLE);
impl Drop for InstanceHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

fn current_user_sid() -> Result<String> {
    use windows::{
        core::PWSTR,
        Win32::{
            Foundation::{LocalFree, HANDLE},
            Security::Authorization::ConvertSidToStringSidW,
            Security::{GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER},
            System::Threading::{GetCurrentProcess, OpenProcessToken},
        },
    };
    let mut token = HANDLE::default();
    unsafe {
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)?;
    }
    let token = TokenHandle(token);
    let mut bytes = vec![0u8; 4096];
    let mut required = 0;
    unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            Some(bytes.as_mut_ptr().cast()),
            bytes.len() as u32,
            &mut required,
        )?;
    }
    let user = unsafe { &*(bytes.as_ptr().cast::<TOKEN_USER>()) };
    let mut sid_string = PWSTR::null();
    unsafe {
        ConvertSidToStringSidW(user.User.Sid, &mut sid_string)?;
    }
    let sid = unsafe { sid_string.to_string() }.context("convert current user SID")?;
    unsafe {
        LocalFree(windows::Win32::Foundation::HLOCAL(sid_string.0.cast()));
    }
    Ok(sid)
}

struct TokenHandle(windows::Win32::Foundation::HANDLE);
impl Drop for TokenHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

fn run_message_window(host: &mut Host, start_in_tray: bool) -> Result<()> {
    use std::{ffi::OsStr, os::windows::ffi::OsStrExt};
    use windows::{
        core::PCWSTR,
        Win32::{
            Foundation::HINSTANCE,
            System::LibraryLoader::GetModuleHandleW,
            UI::{
                Shell::{Shell_NotifyIconW, NIM_DELETE, NOTIFYICONDATAW},
                WindowsAndMessaging::{
                    CreateWindowExW, DispatchMessageW, PeekMessageW, PostQuitMessage,
                    RegisterClassW, SetTimer, ShowWindow, TranslateMessage, CS_HREDRAW, CS_VREDRAW,
                    HWND_MESSAGE, MSG, SW_HIDE, WNDCLASSW, WS_OVERLAPPED,
                },
            },
        },
    };
    let class_name: Vec<u16> = OsStr::new("UnslothStudioDesktopHost")
        .encode_wide()
        .chain(Some(0))
        .collect();
    let instance = unsafe { GetModuleHandleW(PCWSTR::null())? };
    let class = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(window_proc),
        hInstance: HINSTANCE(instance.0),
        lpszClassName: PCWSTR(class_name.as_ptr()),
        ..Default::default()
    };
    if unsafe { RegisterClassW(&class) } == 0 {
        bail!("register tray host window class failed");
    }
    let hwnd = unsafe {
        CreateWindowExW(
            Default::default(),
            PCWSTR(class_name.as_ptr()),
            PCWSTR::null(),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            None,
            instance,
            Some((host as *mut Host).cast()),
        )?
    };
    unsafe {
        let _ = ShowWindow(hwnd, SW_HIDE);
        SetTimer(hwnd, TIMER_ID, 400, None);
    }
    if host.tray_enabled {
        add_tray_icon(hwnd)?;
    }
    if !start_in_tray {
        host.ui = Some(ipc::launch_ui(
            host.state.clone(),
            host.exit_requested.clone(),
        )?);
    }

    let mut message = MSG::default();
    loop {
        while unsafe {
            PeekMessageW(
                &mut message,
                None,
                0,
                0,
                windows::Win32::UI::WindowsAndMessaging::PM_REMOVE,
            )
            .as_bool()
        } {
            if message.message == windows::Win32::UI::WindowsAndMessaging::WM_QUIT {
                if host.tray_enabled {
                    let icon = NOTIFYICONDATAW {
                        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                        hWnd: hwnd,
                        uID: 1,
                        ..Default::default()
                    };
                    unsafe {
                        let _ = Shell_NotifyIconW(NIM_DELETE, &icon);
                    }
                }
                unsafe {
                    let _ = windows::Win32::UI::WindowsAndMessaging::DestroyWindow(hwnd);
                }
                close_ui(host)?;
                return Ok(());
            }
            unsafe {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        let tray_setting = host.state.lock().unwrap().settings.tray_enabled;
        if tray_setting != host.tray_enabled {
            if tray_setting {
                add_tray_icon(hwnd)?;
            } else {
                remove_tray_icon(hwnd);
            }
            host.tray_enabled = tray_setting;
        }
        if let Some(child) = host.ui.as_mut() {
            if child.try_wait()?.is_some() {
                host.ui = None;
                if !host.tray_enabled {
                    host.exit_after_stop = true;
                }
            }
        }
        if host.open_requested.swap(false, Ordering::AcqRel) {
            if let Some(child) = host.ui.as_ref() {
                activate_window(child.id());
            } else {
                host.ui = Some(ipc::launch_ui(
                    host.state.clone(),
                    host.exit_requested.clone(),
                )?);
            }
        }
        if host.exit_requested.load(Ordering::Acquire) || host.exit_after_stop {
            let operation = host.state.lock().unwrap().operation.is_some();
            if operation {
                host.exit_wait_started.get_or_insert_with(Instant::now);
                let operation_kind = {
                    let mut state = host.state.lock().unwrap();
                    state.operation.as_mut().map(|operation| {
                        operation.detail = "Stopping before exit".into();
                        operation.kind.clone()
                    })
                };
                if !host.exit_stop_attempted {
                    host.exit_stop_attempted = true;
                    if operation_kind
                        .as_deref()
                        .is_some_and(|kind| matches!(kind, "install" | "update" | "check_updates"))
                    {
                        if let Err(error) = lifecycle::cancel_owned_operations() {
                            show_error(&format!(
                                "Could not safely cancel active launcher operation:\n\n{error:#}"
                            ));
                            host.exit_requested.store(false, Ordering::Release);
                            host.exit_after_stop = false;
                            host.exit_wait_started = None;
                            host.exit_stop_attempted = false;
                            continue;
                        }
                    }
                    if lifecycle::is_running() {
                        if let Err(error) = lifecycle::stop_owned() {
                            show_error(&format!(
                                "Could not stop launcher-owned Studio process:\n\n{error:#}"
                            ));
                            host.exit_requested.store(false, Ordering::Release);
                            host.exit_after_stop = false;
                            host.exit_wait_started = None;
                            host.exit_stop_attempted = false;
                            continue;
                        }
                    }
                }
                if host
                    .exit_wait_started
                    .is_some_and(|started| started.elapsed() >= Duration::from_secs(30))
                {
                    if let Some(operation) = host.state.lock().unwrap().operation.as_mut() {
                        operation.detail = "Exit blocked: this lifecycle operation cannot be cancelled safely; wait for it to finish".into();
                    }
                    show_error("Cannot exit yet. The active lifecycle operation cannot be cancelled safely. Wait for it to finish, then exit.");
                    host.exit_requested.store(false, Ordering::Release);
                    host.exit_after_stop = false;
                    host.exit_wait_started = None;
                    host.exit_stop_attempted = false;
                }
            } else {
                if lifecycle::is_running() {
                    lifecycle::stop_owned()?;
                }
                unsafe {
                    PostQuitMessage(0);
                }
                break;
            }
        }
        thread::sleep(Duration::from_millis(25));
    }
    if host.tray_enabled {
        let icon = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: 1,
            ..Default::default()
        };
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &icon);
        }
    }
    close_ui(host)?;
    unsafe {
        windows::Win32::UI::WindowsAndMessaging::DestroyWindow(hwnd)?;
    }
    Ok(())
}

fn add_tray_icon(hwnd: windows::Win32::Foundation::HWND) -> Result<()> {
    use std::{ffi::OsStr, os::windows::ffi::OsStrExt};
    use windows::Win32::UI::{
        Shell::{Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NOTIFYICONDATAW},
        WindowsAndMessaging::{LoadIconW, IDI_APPLICATION},
    };
    let mut icon = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
        uCallbackMessage: WM_TRAY,
        hIcon: unsafe { LoadIconW(None, IDI_APPLICATION)? },
        ..Default::default()
    };
    let tip: Vec<u16> = OsStr::new("Unsloth Studio Desktop")
        .encode_wide()
        .chain(Some(0))
        .collect();
    icon.szTip[..tip.len()].copy_from_slice(&tip);
    if unsafe { Shell_NotifyIconW(NIM_ADD, &icon).as_bool() } {
        Ok(())
    } else {
        bail!("add notification-area icon failed")
    }
}

fn remove_tray_icon(hwnd: windows::Win32::Foundation::HWND) {
    use windows::Win32::UI::Shell::{Shell_NotifyIconW, NIM_DELETE, NOTIFYICONDATAW};
    let icon = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        ..Default::default()
    };
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &icon);
    }
}

fn close_ui(host: &mut Host) -> Result<()> {
    if let Some(mut child) = host.ui.take() {
        if child.try_wait()?.is_none() {
            child.kill().context("close launcher-owned native UI")?;
            child.wait().context("wait for native UI to exit")?;
        }
    }
    Ok(())
}

fn activate_window(pid: u32) {
    use windows::Win32::{
        Foundation::{BOOL, LPARAM},
        UI::WindowsAndMessaging::{
            EnumWindows, GetWindowThreadProcessId, IsWindowVisible, SetForegroundWindow,
            ShowWindow, SW_RESTORE,
        },
    };
    let mut context: (u32, Option<windows::Win32::Foundation::HWND>) = (pid, None);
    unsafe {
        let _ = EnumWindows(
            Some(find_process_window),
            LPARAM(&mut context as *mut _ as isize),
        );
        if let Some(hwnd) = context.1 {
            let _ = ShowWindow(hwnd, SW_RESTORE);
            let _ = SetForegroundWindow(hwnd);
        }
    }

    unsafe extern "system" fn find_process_window(
        hwnd: windows::Win32::Foundation::HWND,
        parameter: LPARAM,
    ) -> BOOL {
        let context = &mut *(parameter.0 as *mut (u32, Option<windows::Win32::Foundation::HWND>));
        let mut owner_pid = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut owner_pid));
        if owner_pid == context.0 && IsWindowVisible(hwnd).as_bool() {
            context.1 = Some(hwnd);
            BOOL(0)
        } else {
            BOOL(1)
        }
    }
}

unsafe extern "system" fn window_proc(
    hwnd: windows::Win32::Foundation::HWND,
    msg: u32,
    w: windows::Win32::Foundation::WPARAM,
    l: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::{
        Foundation::LRESULT,
        UI::WindowsAndMessaging::{
            DefWindowProcW, GetWindowLongPtrW, SetWindowLongPtrW, GWLP_USERDATA, WM_COMMAND,
            WM_CREATE, WM_DESTROY, WM_LBUTTONDBLCLK, WM_NCCREATE, WM_RBUTTONUP, WM_TIMER,
        },
    };
    if msg == WM_NCCREATE {
        let create = &*(l.0 as *const windows::Win32::UI::WindowsAndMessaging::CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
    }
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Host;
    if ptr.is_null() {
        return DefWindowProcW(hwnd, msg, w, l);
    }
    let host = &mut *ptr;
    if msg == WM_TRAY {
        if l.0 as u32 == WM_RBUTTONUP {
            show_menu(hwnd);
        }
        if l.0 as u32 == WM_LBUTTONDBLCLK {
            host.open_requested.store(true, Ordering::Release);
        }
        return LRESULT(0);
    }
    if msg == WM_ACTIVATE_HOST {
        host.open_requested.store(true, Ordering::Release);
        return LRESULT(0);
    }
    if msg == WM_COMMAND {
        match w.0 as u32 & 0xffff {
            MENU_OPEN => host.open_requested.store(true, Ordering::Release),
            MENU_START => {
                if let Err(error) = lifecycle::run_operation(host.state.clone(), "start".into()) {
                    eprintln!("Start Studio: {error:#}");
                }
            }
            MENU_STOP => {
                if let Err(error) = lifecycle::run_operation(host.state.clone(), "stop".into()) {
                    eprintln!("Stop Studio: {error:#}");
                }
            }
            MENU_EXIT => host.exit_requested.store(true, Ordering::Release),
            _ => return DefWindowProcW(hwnd, msg, w, l),
        }
        return LRESULT(0);
    }
    if msg == WM_TIMER && w.0 == TIMER_ID {
        return LRESULT(0);
    }
    if msg == WM_CREATE {
        return LRESULT(0);
    }
    if msg == WM_DESTROY {
        windows::Win32::UI::WindowsAndMessaging::PostQuitMessage(0);
        return LRESULT(0);
    }
    DefWindowProcW(hwnd, msg, w, l)
}

unsafe fn show_menu(hwnd: windows::Win32::Foundation::HWND) {
    use std::{ffi::OsStr, os::windows::ffi::OsStrExt};
    use windows::{
        core::PCWSTR,
        Win32::UI::WindowsAndMessaging::{
            AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, SetForegroundWindow,
            TrackPopupMenu, MF_STRING, TPM_RETURNCMD, TPM_RIGHTBUTTON,
        },
    };
    let Ok(menu) = CreatePopupMenu() else {
        return;
    };
    let entries = [
        (MENU_OPEN, "Open"),
        (MENU_START, "Start Studio"),
        (MENU_STOP, "Stop Studio"),
        (MENU_EXIT, "Exit"),
    ];
    for (id, label) in entries {
        let text: Vec<u16> = OsStr::new(label).encode_wide().chain(Some(0)).collect();
        if AppendMenuW(menu, MF_STRING, id as usize, PCWSTR(text.as_ptr())).is_err() {
            let _ = DestroyMenu(menu);
            return;
        }
    }
    let mut point = windows::Win32::Foundation::POINT::default();
    if GetCursorPos(&mut point).is_err() {
        let _ = DestroyMenu(menu);
        return;
    }
    let _ = SetForegroundWindow(hwnd);
    let command = TrackPopupMenu(
        menu,
        TPM_RETURNCMD | TPM_RIGHTBUTTON,
        point.x,
        point.y,
        0,
        hwnd,
        None,
    );
    let _ = DestroyMenu(menu);
    if command.0 != 0 {
        let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
            hwnd,
            windows::Win32::UI::WindowsAndMessaging::WM_COMMAND,
            windows::Win32::Foundation::WPARAM(command.0 as usize),
            windows::Win32::Foundation::LPARAM(0),
        );
    }
}

pub fn apply_integrations(previous: &Settings, settings: &Settings) -> Result<()> {
    previous.validate()?;
    settings.validate()?;
    let exe = std::env::current_exe().context("find launcher executable path")?;
    let desktop = known_folder(&windows::Win32::UI::Shell::FOLDERID_Desktop)?
        .join("Unsloth Studio Desktop.lnk");
    let start_menu = known_folder(&windows::Win32::UI::Shell::FOLDERID_StartMenu)?
        .join(r"Programs\Unsloth Studio Desktop.lnk");
    let startup_changed = previous.startup_enabled != settings.startup_enabled;
    let desktop_changed = previous.desktop_shortcut != settings.desktop_shortcut;
    let start_menu_changed = previous.start_menu_shortcut != settings.start_menu_shortcut;
    let mut startup_attempted = false;
    let mut desktop_attempted = false;
    let mut start_menu_attempted = false;
    let result = (|| -> Result<()> {
        if startup_changed {
            startup_attempted = true;
            apply_startup(settings.startup_enabled, &exe)?;
        }
        if desktop_changed {
            desktop_attempted = true;
            apply_shortcut(&desktop, settings.desktop_shortcut, &exe)?;
        }
        if start_menu_changed {
            start_menu_attempted = true;
            apply_shortcut(&start_menu, settings.start_menu_shortcut, &exe)?;
        }
        Ok(())
    })();
    if let Err(error) = result {
        let mut rollback_errors = Vec::new();
        if start_menu_attempted {
            if let Err(rollback) = apply_shortcut(&start_menu, previous.start_menu_shortcut, &exe) {
                rollback_errors.push(format!("start-menu shortcut: {rollback:#}"));
            }
        }
        if desktop_attempted {
            if let Err(rollback) = apply_shortcut(&desktop, previous.desktop_shortcut, &exe) {
                rollback_errors.push(format!("desktop shortcut: {rollback:#}"));
            }
        }
        if startup_attempted {
            if let Err(rollback) = apply_startup(previous.startup_enabled, &exe) {
                rollback_errors.push(format!("startup setting: {rollback:#}"));
            }
        }
        if rollback_errors.is_empty() {
            return Err(error).context("integration update failed; attempted rollback");
        }
        bail!(
            "integration update failed: {error:#}; rollback also failed: {}",
            rollback_errors.join("; ")
        );
    }
    Ok(())
}

fn known_folder(id: &windows::core::GUID) -> Result<std::path::PathBuf> {
    use windows::{
        core::PCWSTR,
        Win32::{
            Foundation::HANDLE,
            System::Com::CoTaskMemFree,
            UI::Shell::{SHGetKnownFolderPath, KF_FLAG_DEFAULT},
        },
    };
    let path = unsafe { SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, HANDLE::default())? };
    let decoded = unsafe { PCWSTR(path.0).to_string() }.context("decode Windows known folder path");
    unsafe { CoTaskMemFree(Some(path.0.cast())) };
    Ok(decoded?.into())
}

fn apply_startup(enabled: bool, exe: &std::path::Path) -> Result<()> {
    const SCRIPT: &str = r#"$ErrorActionPreference='Stop'; $key='HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'; $name='Unsloth Studio Desktop'; $expected='"'+$env:UNSLOTH_LAUNCHER_EXE+'" --tray'; $current=(Get-ItemProperty -Path $key -Name $name -ErrorAction SilentlyContinue).$name; if($env:UNSLOTH_ENABLE -eq '1'){if($current -and $current -ne $expected){throw 'Startup entry name is already used by another command.'}; New-Item -Path $key -Force | Out-Null; Set-ItemProperty -Path $key -Name $name -Value $expected}else{if($current -eq $expected){Remove-ItemProperty -Path $key -Name $name}}"#;
    let status = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", SCRIPT])
        .env("UNSLOTH_LAUNCHER_EXE", exe)
        .env("UNSLOTH_ENABLE", if enabled { "1" } else { "0" })
        .status()?;
    if !status.success() {
        bail!("could not apply owned startup setting");
    }
    Ok(())
}

fn apply_shortcut(path: &std::path::Path, enabled: bool, exe: &std::path::Path) -> Result<()> {
    const SCRIPT: &str = r#"$ErrorActionPreference='Stop'; $path=$env:UNSLOTH_SHORTCUT_PATH; $target=$env:UNSLOTH_LAUNCHER_EXE; $shell=New-Object -ComObject WScript.Shell; $exists=Test-Path -LiteralPath $path; $owned=$false; if($exists){$link=$shell.CreateShortcut($path); $owned=($link.TargetPath -eq $target -and ($link.Arguments -eq '' -or $link.Arguments -eq '--tray'))}; if($env:UNSLOTH_ENABLE -eq '1'){if($exists -and -not $owned){throw 'Shortcut path is already used by another target.'}; New-Item -ItemType Directory -Force -Path (Split-Path -Parent $path) | Out-Null; $link=$shell.CreateShortcut($path); $link.TargetPath=$target; $link.Arguments=''; $link.WorkingDirectory=(Split-Path -Parent $target); $link.Save()}elseif($exists -and $owned){Remove-Item -LiteralPath $path}"#;
    let status = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", SCRIPT])
        .env("UNSLOTH_SHORTCUT_PATH", path)
        .env("UNSLOTH_LAUNCHER_EXE", exe)
        .env("UNSLOTH_ENABLE", if enabled { "1" } else { "0" })
        .status()?;
    if !status.success() {
        bail!(
            "could not apply owned shortcut setting at {}",
            path.display()
        );
    }
    Ok(())
}

fn show_error(message: &str) {
    use std::{ffi::OsStr, os::windows::ffi::OsStrExt};
    use windows::{
        core::PCWSTR,
        Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK},
    };
    let text: Vec<u16> = OsStr::new(message).encode_wide().chain(Some(0)).collect();
    let title: Vec<u16> = OsStr::new("Unsloth Studio Desktop")
        .encode_wide()
        .chain(Some(0))
        .collect();
    unsafe {
        let _ = MessageBoxW(
            None,
            PCWSTR(text.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OK | MB_ICONERROR,
        );
    }
}
