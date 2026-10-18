use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    env, fs,
    path::{Component, Path, PathBuf},
    process::Command,
};

fn main() {
    println!("cargo:rerun-if-env-changed=UNSLOTH_STUDIO_UI");
    println!("cargo:rerun-if-env-changed=UNSLOTH_NATIVE_PAYLOAD_DIR");

    let repo = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    if env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows") {
        embed_windows_icon(&repo);
    }
    let manifest = repo.join("native-ui/portable-files.txt");
    let native_helper = repo.join("scripts/build-native.ps1");
    println!("cargo:rerun-if-changed={}", manifest.display());
    println!("cargo:rerun-if-changed={}", native_helper.display());
    track_native_inputs(&repo.join("native-ui"));

    let profile = env::var("PROFILE").unwrap_or_default();
    let native_dir_override = env::var_os("UNSLOTH_NATIVE_PAYLOAD_DIR").map(PathBuf::from);
    let exe_override = env::var_os("UNSLOTH_STUDIO_UI").map(PathBuf::from);
    if native_dir_override.is_some() && exe_override.is_some() {
        panic!("set only one of UNSLOTH_NATIVE_PAYLOAD_DIR or UNSLOTH_STUDIO_UI");
    }

    let payload_root = if let Some(root) = native_dir_override.as_ref() {
        root.clone()
    } else if let Some(exe) = exe_override.as_ref() {
        if profile == "release" {
            panic!(
                "UNSLOTH_STUDIO_UI is a development override and cannot be used for release builds"
            );
        }
        if exe
            .file_name()
            .is_none_or(|name| !name.eq_ignore_ascii_case("StudioUI.exe"))
            || !exe.is_file()
        {
            panic!("UNSLOTH_STUDIO_UI must name an existing StudioUI.exe");
        }
        exe.parent()
            .expect("StudioUI.exe has a parent directory")
            .to_owned()
    } else {
        repo.join("native-ui/x64/Release")
    };

    if profile != "release"
        && native_dir_override.is_none()
        && exe_override.is_none()
        && env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows")
    {
        prepare_native_ui(&repo, &native_helper);
    }

    let payload = collect_payload(&manifest, &payload_root);
    validate_payload(&payload);
    let track_override_payload = native_dir_override.is_some() || exe_override.is_some();
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let mut encoded = Vec::new();
    encoded.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    for (name, bytes, path) in payload {
        if track_override_payload {
            println!("cargo:rerun-if-changed={}", path.display());
        }
        let name = name.into_bytes();
        encoded.extend_from_slice(&(name.len() as u32).to_le_bytes());
        encoded.extend_from_slice(&name);
        encoded.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        encoded.extend_from_slice(&Sha256::digest(&bytes));
        encoded.extend_from_slice(&bytes);
    }
    fs::write(out.join("native_payload.bin"), encoded).expect("write embedded payload");
}

fn embed_windows_icon(repo: &Path) {
    let resource = repo.join("assets/unsloth-launcher/icon.rc");
    let icon = repo.join("assets/unsloth-launcher/icon.ico");
    println!("cargo:rerun-if-changed={}", resource.display());
    println!("cargo:rerun-if-changed={}", icon.display());

    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let compiled_resource = out.join("unsloth-launcher-icon.res");
    let compiler = find_resource_compiler();
    let status = Command::new(&compiler)
        .arg("/nologo")
        .arg(format!("/fo{}", compiled_resource.display()))
        .arg("icon.rc")
        .current_dir(resource.parent().expect("icon resource directory"))
        .status()
        .unwrap_or_else(|error| panic!("could not start Windows resource compiler: {error}"));
    if !status.success() {
        panic!("Windows resource compiler failed to embed the launcher icon");
    }
    println!("cargo:rustc-link-arg={}", compiled_resource.display());
}

fn find_resource_compiler() -> PathBuf {
    if let Some(path) = env::var_os("RC") {
        return PathBuf::from(path);
    }
    if Command::new("rc.exe").arg("/?").output().is_ok() {
        return PathBuf::from("rc.exe");
    }

    let mut roots = Vec::new();
    if let Some(root) = env::var_os("WindowsSdkDir") {
        roots.push(PathBuf::from(root).join("bin"));
    }
    roots.push(PathBuf::from(r"C:\Program Files (x86)\Windows Kits\10\bin"));
    for root in roots {
        let Ok(versions) = fs::read_dir(root) else {
            continue;
        };
        let mut versions: Vec<_> = versions
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .collect();
        versions.sort();
        versions.reverse();
        for version in versions {
            let candidate = version.join("x64/rc.exe");
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    panic!("Windows SDK resource compiler rc.exe was not found");
}

fn prepare_native_ui(repo: &Path, helper: &Path) {
    if !cfg!(windows) {
        panic!("automatic WinUI preparation requires a Windows MSVC build host");
    }
    println!("cargo:warning=Preparing the self-contained WinUI companion with Framework MSBuild");
    let status = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(helper)
        .arg("-RepoRoot")
        .arg(repo)
        .current_dir(repo)
        .status()
        .unwrap_or_else(|error| {
            panic!("could not start PowerShell for native WinUI preparation: {error}")
        });
    if !status.success() {
        panic!("native WinUI preparation failed; install Visual Studio 2022 C++ Build Tools, Windows SDK 10.0.26100.0, and Framework MSBuild/XAML build tasks, then rerun cargo run");
    }
}

fn track_native_inputs(root: &Path) {
    fn visit(path: &Path) {
        let entries = fs::read_dir(path).unwrap_or_else(|error| {
            panic!(
                "read native UI source directory {}: {error}",
                path.display()
            )
        });
        for entry in entries {
            let entry =
                entry.unwrap_or_else(|error| panic!("read native UI source entry: {error}"));
            let child = entry.path();
            if child.is_dir() {
                let name = child
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or_default();
                if !matches!(name, "obj" | "x64" | "packages" | ".vs") {
                    visit(&child);
                }
            } else if is_native_input(&child) {
                println!("cargo:rerun-if-changed={}", child.display());
            }
        }
    }
    visit(root);
}

fn is_native_input(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("cpp" | "h" | "xaml" | "idl" | "vcxproj" | "manifest" | "props" | "targets")
    ) || path
        .file_name()
        .is_some_and(|name| name == "portable-files.txt")
}

fn collect_payload(manifest: &Path, root: &Path) -> Vec<(String, Vec<u8>, PathBuf)> {
    let lines = fs::read_to_string(manifest).unwrap_or_else(|error| {
        panic!(
            "read native payload manifest {}: {error}",
            manifest.display()
        )
    });
    let mut payload = Vec::new();
    let mut names = HashSet::new();
    for line in lines.lines() {
        let relative = line.trim();
        if relative.is_empty() || relative.starts_with('#') {
            continue;
        }
        let relative_path = Path::new(relative);
        if relative_path.is_absolute()
            || relative_path
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            panic!("unsafe native payload manifest path: {relative}");
        }
        let candidates = matching_files(root, relative);
        if candidates.is_empty() {
            panic!(
                "missing required native payload {}",
                root.join(relative).display()
            );
        }
        for path in candidates {
            let name = path
                .strip_prefix(root)
                .expect("payload candidate is under the payload root")
                .to_string_lossy()
                .replace('\\', "/");
            if !names.insert(name.clone()) {
                panic!("duplicate native payload entry: {name}");
            }
            let bytes = fs::read(&path)
                .unwrap_or_else(|error| panic!("read native payload {}: {error}", path.display()));
            payload.push((name, bytes, path));
        }
    }
    payload
}

fn validate_payload(payload: &[(String, Vec<u8>, PathBuf)]) {
    if payload.is_empty() {
        panic!("native payload manifest selected no files");
    }
    for required in [
        "StudioUI.exe",
        "Microsoft.WindowsAppRuntime.Bootstrap.dll",
        "Microsoft.ui.xaml.dll",
    ] {
        if !payload
            .iter()
            .any(|(name, _, _)| name.eq_ignore_ascii_case(required))
        {
            panic!("native payload is incomplete; manifest payload is missing {required}");
        }
    }
}

fn matching_files(root: &Path, relative: &str) -> Vec<PathBuf> {
    if !relative.contains('*') {
        let path = root.join(relative);
        return if path.is_file() {
            vec![path]
        } else {
            Vec::new()
        };
    }
    let pattern_path = Path::new(relative);
    let parent = root.join(pattern_path.parent().unwrap_or(Path::new("")));
    let pattern = pattern_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    let Some((prefix, suffix)) = pattern.split_once('*') else {
        return Vec::new();
    };
    let mut matches: Vec<_> = fs::read_dir(parent)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(prefix) && name.ends_with(suffix))
        })
        .collect();
    matches.sort();
    matches
}
