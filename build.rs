use sha2::{Digest, Sha256};
use std::{
    env, fs,
    path::{Path, PathBuf},
};

fn main() {
    println!("cargo:rerun-if-env-changed=UNSLOTH_STUDIO_UI");
    println!("cargo:rerun-if-changed=native-ui/portable-files.txt");
    let profile = env::var("PROFILE").unwrap_or_default();
    let manifest = Path::new("native-ui/portable-files.txt");
    let payload_root = env::var_os("UNSLOTH_NATIVE_PAYLOAD_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("native-ui"));
    println!("cargo:rerun-if-env-changed=UNSLOTH_NATIVE_PAYLOAD_DIR");
    let mut payload = Vec::new();
    if profile != "release" {
        if let Ok(exe) = env::var("UNSLOTH_STUDIO_UI") {
            let bytes =
                fs::read(&exe).unwrap_or_else(|e| panic!("read UNSLOTH_STUDIO_UI={exe}: {e}"));
            payload.push(("StudioUI.exe".to_owned(), bytes));
        }
    }
    if payload.is_empty() && manifest.exists() {
        for line in fs::read_to_string(manifest)
            .expect("read native payload manifest")
            .lines()
        {
            let relative = line.trim();
            if relative.is_empty() || relative.starts_with('#') {
                continue;
            }
            let candidates = matching_files(&payload_root, relative);
            if candidates.is_empty() {
                if profile == "release" && !relative.contains('*') {
                    panic!(
                        "missing native payload {}",
                        payload_root.join(relative).display()
                    );
                }
                continue;
            }
            for path in candidates {
                println!("cargo:rerun-if-changed={}", path.display());
                let bytes = fs::read(&path)
                    .unwrap_or_else(|e| panic!("read native payload {}: {e}", path.display()));
                let name = path
                    .strip_prefix(&payload_root)
                    .expect("payload under payload root")
                    .to_string_lossy()
                    .replace('\\', "/");
                payload.push((name, bytes));
            }
        }
    }
    if payload.is_empty() && profile == "release" {
        panic!("release requires native-ui/portable-files.txt and the built StudioUI.exe payload");
    }
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let mut encoded = Vec::new();
    encoded.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    for (name, bytes) in payload {
        let name = name.into_bytes();
        encoded.extend_from_slice(&(name.len() as u32).to_le_bytes());
        encoded.extend_from_slice(&name);
        encoded.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        encoded.extend_from_slice(&Sha256::digest(&bytes));
        encoded.extend_from_slice(&bytes);
    }
    fs::write(out.join("native_payload.bin"), encoded).expect("write embedded payload");
}

fn matching_files(root: &Path, relative: &str) -> Vec<PathBuf> {
    if !relative.contains('*') {
        let p = root.join(relative);
        return if p.is_file() { vec![p] } else { Vec::new() };
    }
    let path = Path::new(relative);
    let parent = root.join(path.parent().unwrap_or(Path::new("")));
    let pattern = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let Some((prefix, suffix)) = pattern.split_once('*') else {
        return Vec::new();
    };
    fs::read_dir(parent)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with(prefix) && n.ends_with(suffix))
        })
        .collect()
}
