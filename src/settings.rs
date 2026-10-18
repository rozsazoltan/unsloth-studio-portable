use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Component, Path, PathBuf},
};

pub const PRODUCT_DIR: &str = "Unsloth Studio Desktop";
const SETTINGS_FILE: &str = "launcher-settings.json";
const OWNERSHIP_FILE: &str = ".unsloth-studio-desktop-owned";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Settings {
    pub root: PathBuf,
    pub hf_home: PathBuf,
    pub tray_enabled: bool,
    pub startup_enabled: bool,
    pub desktop_shortcut: bool,
    pub start_menu_shortcut: bool,
    pub port: u16,
}

impl Settings {
    pub fn load_or_default() -> Result<Self> {
        let config = config_dir()?.join(SETTINGS_FILE);
        if config.exists() {
            let s: Self = serde_json::from_slice(&fs::read(&config).context("read settings")?)?;
            s.validate()?;
            return Ok(s);
        }
        let parent = env_or_home("USERPROFILE")?.join("Documents");
        let root = parent.join(PRODUCT_DIR);
        Ok(Self {
            hf_home: root.join("huggingface"),
            root,
            tray_enabled: true,
            startup_enabled: false,
            desktop_shortcut: false,
            start_menu_shortcut: false,
            port: 8888,
        })
    }

    pub fn save(&self) -> Result<()> {
        self.validate()?;
        let file = config_dir()?.join(SETTINGS_FILE);
        fs::create_dir_all(file.parent().unwrap())?;
        let temp = file.with_extension("tmp");
        fs::write(&temp, serde_json::to_vec_pretty(self)?)?;
        fs::rename(temp, file)?;
        Ok(())
    }

    pub fn validate(&self) -> Result<()> {
        let root = normalized_path(&self.root)?;
        let hf_home = normalized_path(&self.hf_home)?;
        if !root
            .file_name()
            .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(PRODUCT_DIR))
        {
            bail!("product root must end in {PRODUCT_DIR:?}");
        }
        validate_protected_path(&root, "product root")?;
        validate_protected_path(&hf_home, "Hugging Face directory")?;
        reject_reparse_components(&root)?;
        reject_reparse_components(&hf_home)?;
        let hf_physical = physical_path(&hf_home)?;
        for managed in [self.app_dir(), self.root.join("runtime")] {
            reject_reparse_components(&managed)?;
            let managed_physical = physical_path(&normalized_path(&managed)?)?;
            if paths_overlap(&hf_physical, &managed_physical)? {
                bail!("Hugging Face directory cannot overlap launcher-managed paths");
            }
        }
        if self.port == 0 {
            bail!("port must be between 1 and 65535");
        }
        Ok(())
    }

    pub fn is_owned(&self) -> bool {
        let marker = self.root.join(OWNERSHIP_FILE);
        if reject_reparse_components(&marker).is_err() {
            return false;
        }
        fs::read_to_string(marker).is_ok_and(|contents| {
            let mut lines = contents.lines();
            lines.next() == Some("unsloth-studio-portable-v1")
                && lines
                    .next()
                    .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())
                && lines.next().is_none()
        })
    }

    pub fn has_active_content(&self) -> Result<bool> {
        for path in [
            self.app_dir().join("source"),
            self.app_dir().join("unsloth_studio"),
            self.root.join("runtime"),
        ] {
            reject_reparse_components(&path)?;
            match fs::symlink_metadata(&path) {
                Ok(_) => return Ok(true),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("inspect launcher-managed path {}", path.display())
                    })
                }
            }
        }
        Ok(false)
    }

    pub fn mark_owned(&self) -> Result<()> {
        self.validate()?;
        if !self.root.exists() {
            fs::create_dir_all(&self.root)?;
        }
        let marker = self.root.join(OWNERSHIP_FILE);
        if marker.exists() {
            if self.is_owned() {
                return Ok(());
            }
            bail!("product root contains an invalid launcher ownership marker");
        }
        let entries = fs::read_dir(&self.root)?;
        if entries.into_iter().next().is_some() {
            bail!("refusing to claim a nonempty product root without a launcher ownership marker");
        }
        let marker_contents = format!("unsloth-studio-portable-v1\n{}\n", uuid::Uuid::new_v4());
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        use std::io::Write;
        options
            .open(marker)?
            .write_all(marker_contents.as_bytes())?;
        Ok(())
    }

    pub fn app_dir(&self) -> PathBuf {
        self.root.join("app")
    }
}

pub fn config_dir() -> Result<PathBuf> {
    Ok(env_or_home("LOCALAPPDATA")?.join("Unsloth Studio Desktop"))
}

fn env_or_home(key: &str) -> Result<PathBuf> {
    std::env::var_os(key)
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
        .context("could not determine current-user data directory")
}

fn validate_absolute(path: &Path) -> Result<()> {
    if !path.is_absolute() {
        bail!("path must be absolute: {}", path.display());
    }
    if path
        .components()
        .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        bail!("path cannot contain . or .. components: {}", path.display());
    }
    Ok(())
}

fn normalized_path(path: &Path) -> Result<PathBuf> {
    validate_absolute(path)?;
    if !path
        .components()
        .any(|component| matches!(component, Component::Normal(_)))
    {
        bail!(
            "path cannot be a drive or filesystem root: {}",
            path.display()
        );
    }
    #[cfg(windows)]
    {
        use std::path::Prefix;
        match path.components().next() {
            Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::Disk(_)) => {}
            _ => bail!("path must use a local drive letter: {}", path.display()),
        }
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::Normal(part) => normalized.push(part),
            Component::CurDir | Component::ParentDir => unreachable!(),
        }
    }
    #[cfg(windows)]
    for component in path.components() {
        if let Component::Normal(part) = component {
            let name = part.to_string_lossy();
            let lower = name.to_ascii_lowercase();
            let base = lower.split('.').next().unwrap_or_default();
            if name.ends_with('.') || name.ends_with(' ') || name.contains(':') {
                bail!(
                    "path component has an unsafe Windows alias: {}",
                    path.display()
                );
            }
            if matches!(base, "con" | "prn" | "aux" | "nul")
                || ["com", "lpt"].iter().any(|prefix| {
                    base.strip_prefix(prefix).is_some_and(|digit| {
                        matches!(digit, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
                    })
                })
            {
                bail!(
                    "path contains a reserved Windows device name: {}",
                    path.display()
                );
            }
        }
    }
    Ok(normalized)
}

fn paths_overlap(a: &Path, b: &Path) -> Result<bool> {
    Ok(path_is_within(a, b)? || path_is_within(b, a)?)
}

fn path_is_within(path: &Path, parent: &Path) -> Result<bool> {
    let path_parts: Vec<_> = path.components().collect();
    let parent_parts: Vec<_> = parent.components().collect();
    if path_parts.len() < parent_parts.len() {
        return Ok(false);
    }
    for (a, b) in path_parts.iter().zip(parent_parts.iter()) {
        if !component_eq(a.as_os_str(), b.as_os_str())? {
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg(windows)]
fn component_eq(a: &std::ffi::OsStr, b: &std::ffi::OsStr) -> Result<bool> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    extern "system" {
        fn CompareStringOrdinal(
            string1: *const u16,
            count1: i32,
            string2: *const u16,
            count2: i32,
            ignore_case: i32,
        ) -> i32;
    }
    let a: Vec<u16> = a.encode_wide().collect();
    let b: Vec<u16> = b.encode_wide().collect();
    if a.len() > i32::MAX as usize || b.len() > i32::MAX as usize {
        bail!("Windows path component is too long to compare safely");
    }
    match unsafe { CompareStringOrdinal(a.as_ptr(), a.len() as i32, b.as_ptr(), b.len() as i32, 1) }
    {
        1 | 3 => Ok(false),
        2 => Ok(true),
        _ => Err(std::io::Error::last_os_error())
            .context("compare Windows path components case-insensitively"),
    }
}

#[cfg(not(windows))]
fn component_eq(a: &std::ffi::OsStr, b: &std::ffi::OsStr) -> Result<bool> {
    Ok(a.to_string_lossy()
        .eq_ignore_ascii_case(&b.to_string_lossy()))
}

fn same_path(a: &Path, b: &Path) -> Result<bool> {
    Ok(path_is_within(a, b)? && path_is_within(b, a)?)
}

pub(crate) fn physical_path_eq(a: &Path, b: &Path) -> Result<bool> {
    same_path(a, b)
}

pub(crate) fn physical_path(path: &Path) -> Result<PathBuf> {
    let path = normalized_path(path)?;
    reject_reparse_components(&path)?;
    let mut ancestor = path.as_path();
    let mut missing = Vec::new();
    loop {
        match fs::metadata(ancestor) {
            Ok(metadata) if metadata.is_dir() => break,
            Ok(_) => bail!("path ancestor is not a directory: {}", ancestor.display()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let component = ancestor
                    .file_name()
                    .context("path has no existing directory ancestor")?;
                missing.push(component.to_os_string());
                ancestor = ancestor
                    .parent()
                    .context("path has no existing directory ancestor")?;
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("resolve physical path ancestor {}", ancestor.display())
                })
            }
        }
    }

    #[cfg(windows)]
    let mut resolved = windows_final_path(ancestor)?;
    #[cfg(not(windows))]
    let mut resolved = fs::canonicalize(ancestor)
        .with_context(|| format!("resolve physical path ancestor {}", ancestor.display()))?;
    for component in missing.iter().rev() {
        resolved.push(component);
    }
    Ok(resolved)
}

#[cfg(windows)]
fn windows_final_path(path: &Path) -> Result<PathBuf> {
    use std::{
        ffi::c_void,
        os::windows::ffi::{OsStrExt, OsStringExt},
    };

    const FILE_READ_ATTRIBUTES: u32 = 0x0080;
    const FILE_SHARE_READ: u32 = 0x0000_0001;
    const FILE_SHARE_WRITE: u32 = 0x0000_0002;
    const FILE_SHARE_DELETE: u32 = 0x0000_0004;
    const OPEN_EXISTING: u32 = 3;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const VOLUME_NAME_GUID: u32 = 0x0000_0001;

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateFileW(
            name: *const u16,
            access: u32,
            share_mode: u32,
            security_attributes: *mut c_void,
            creation_disposition: u32,
            flags_and_attributes: u32,
            template_file: *mut c_void,
        ) -> *mut c_void;
        fn GetFinalPathNameByHandleW(
            file: *mut c_void,
            path: *mut u16,
            path_size: u32,
            flags: u32,
        ) -> u32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }

    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null_mut(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            std::ptr::null_mut(),
        )
    };
    if handle as isize == -1 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("open directory identity {}", path.display()));
    }

    let result = (|| -> Result<PathBuf> {
        let mut buffer = vec![0u16; 512];
        let mut length = unsafe {
            GetFinalPathNameByHandleW(
                handle,
                buffer.as_mut_ptr(),
                buffer.len() as u32,
                VOLUME_NAME_GUID,
            )
        };
        if length == 0 {
            return Err(std::io::Error::last_os_error())
                .context("resolve directory identity with volume GUID");
        }
        if length >= buffer.len() as u32 {
            buffer.resize(length as usize + 1, 0);
            length = unsafe {
                GetFinalPathNameByHandleW(
                    handle,
                    buffer.as_mut_ptr(),
                    buffer.len() as u32,
                    VOLUME_NAME_GUID,
                )
            };
            if length == 0 || length >= buffer.len() as u32 {
                return Err(std::io::Error::last_os_error())
                    .context("resolve complete directory identity with volume GUID");
            }
        }
        let result = PathBuf::from(std::ffi::OsString::from_wide(&buffer[..length as usize]));
        let result_text = result.as_os_str().to_string_lossy();
        if !result.is_absolute()
            || !result_text
                .get(..11)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("\\\\?\\Volume{"))
        {
            bail!("Windows did not return a local volume-GUID directory identity");
        }
        Ok(result)
    })();
    unsafe { CloseHandle(handle) };
    result
}

fn validate_protected_path(path: &Path, label: &str) -> Result<()> {
    let text = path.to_string_lossy();
    if text.starts_with("\\\\") || text.starts_with("\\?\\") || text.starts_with("\\.\\") {
        bail!("{label} cannot use a network or device path");
    }
    let profile = std::env::var_os("USERPROFILE").map(PathBuf::from);
    let profile_lexical = profile.as_deref().map(normalized_path).transpose()?;
    let protected = vec![
        std::env::var_os("SystemRoot").map(PathBuf::from),
        std::env::var_os("WINDIR").map(PathBuf::from),
        std::env::var_os("ProgramFiles").map(PathBuf::from),
        std::env::var_os("ProgramFiles(x86)").map(PathBuf::from),
        std::env::var_os("ProgramData").map(PathBuf::from),
        std::env::var_os("CommonProgramFiles").map(PathBuf::from),
        std::env::var_os("CommonProgramFiles(x86)").map(PathBuf::from),
        std::env::var_os("APPDATA").map(PathBuf::from),
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from),
        profile.clone(),
    ];
    for root in protected.iter().flatten() {
        let protected_root = normalized_path(root)?;
        if paths_overlap(path, &protected_root)? {
            let profile_child = if let Some(profile) = profile_lexical.as_deref() {
                same_path(profile, &protected_root)?
                    && path_is_within(path, profile)?
                    && !path_is_within(profile, path)?
            } else {
                false
            };
            if profile_child {
                continue;
            }
            bail!(
                "{label} overlaps a protected Windows or profile path: {}",
                root.display()
            );
        }
    }

    let path_physical = physical_path(path)?;
    let profile_physical = profile.as_deref().map(physical_path).transpose()?;
    for root in protected.iter().flatten() {
        let root_physical = physical_path(&normalized_path(root)?)?;
        if paths_overlap(&path_physical, &root_physical)? {
            let profile_child = if let Some(profile) = profile_physical.as_deref() {
                same_path(profile, &root_physical)?
                    && path_is_within(&path_physical, profile)?
                    && !path_is_within(profile, &path_physical)?
            } else {
                false
            };
            if profile_child {
                continue;
            }
            bail!(
                "{label} physically overlaps a protected Windows or profile path: {}",
                root.display()
            );
        }
    }
    Ok(())
}

pub fn safe_owned_paths(settings: &Settings, scope: &str) -> Result<Vec<PathBuf>> {
    settings.validate()?;
    if !settings.is_owned() {
        bail!("launcher ownership marker is missing; refusing deletion");
    }
    let mut paths = vec![
        settings.app_dir().join("source"),
        settings.app_dir().join("unsloth_studio"),
        settings.root.join("runtime"),
    ];
    let root_identity = physical_path(&normalized_path(&settings.root)?)?;
    let hf_identity = physical_path(&normalized_path(&settings.hf_home)?)?;
    let default_hf = settings.root.join("huggingface");
    let default_hf_identity = physical_path(&normalized_path(&default_hf)?)?;
    if scope == "all_owned_data" {
        if same_path(&hf_identity, &default_hf_identity)? {
            paths.push(default_hf);
        }
    } else if scope != "runtime" {
        bail!("unsupported removal scope");
    }

    let mut identities: Vec<PathBuf> = Vec::with_capacity(paths.len());
    for path in &paths {
        let path_identity = physical_path(&normalized_path(path)?)?;
        if !path_is_within(&path_identity, &root_identity)? {
            bail!("managed path escaped product root");
        }
        reject_reparse_components(path)?;
        reject_reparse_descendants(path)?;
        if paths_overlap(&hf_identity, &path_identity)? {
            let is_explicit_default_cache = scope == "all_owned_data"
                && same_path(&path_identity, &default_hf_identity)?
                && same_path(&hf_identity, &default_hf_identity)?;
            if !is_explicit_default_cache {
                bail!("Hugging Face directory physically overlaps a deletable launcher path");
            }
        }
        for previous in &identities {
            if paths_overlap(previous, &path_identity)? {
                bail!("launcher deletion paths physically overlap; refusing ambiguous removal");
            }
        }
        identities.push(path_identity);
    }
    Ok(paths)
}

pub(crate) fn reject_reparse_components(path: &Path) -> Result<()> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            use windows::Win32::Storage::FileSystem::{
                GetFileAttributesW, FILE_ATTRIBUTE_REPARSE_POINT,
            };
            let wide: Vec<u16> = current.as_os_str().encode_wide().chain(Some(0)).collect();
            let attrs = unsafe { GetFileAttributesW(windows::core::PCWSTR(wide.as_ptr())) };
            if attrs != u32::MAX && attrs & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
                bail!(
                    "reparse point in managed path; refusing deletion: {}",
                    current.display()
                );
            }
        }
        #[cfg(not(windows))]
        {
            if let Ok(metadata) = fs::symlink_metadata(&current) {
                if metadata.file_type().is_symlink() {
                    bail!(
                        "symlink in managed path; refusing deletion: {}",
                        current.display()
                    );
                }
            }
        }
    }
    Ok(())
}

fn reject_reparse_descendants(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    reject_reparse_components(path)?;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let child = entry.path();
        reject_reparse_components(&child)?;
        if child.is_dir() {
            reject_reparse_descendants(&child)?;
        }
    }
    Ok(())
}

pub(crate) fn reject_reparse_tree(path: &Path) -> Result<()> {
    reject_reparse_descendants(path)
}
