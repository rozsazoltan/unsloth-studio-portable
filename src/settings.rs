use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub root: String,
    pub hf_home: String,
    pub port: u16,
}

impl Default for Settings {
    fn default() -> Self {
        let root = std::env::var("USERPROFILE").unwrap_or_else(|_| ".".into());
        let root = PathBuf::from(root).join("Unsloth Studio Desktop");
        Self {
            hf_home: root.join("huggingface").to_string_lossy().into(),
            root: root.to_string_lossy().into(),
            port: 8888,
        }
    }
}

impl Settings {
    pub fn load_or_default() -> Result<Self> {
        let path = config_path()?;
        if path.exists() {
            Ok(serde_json::from_slice(&fs::read(path)?)?)
        } else {
            Ok(Self::default())
        }
    }
    pub fn save(&self) -> Result<()> {
        let path = config_path()?;
        fs::create_dir_all(path.parent().context("settings path has no parent")?)?;
        fs::write(path, serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }
}

fn config_path() -> Result<PathBuf> {
    Ok(PathBuf::from(
        std::env::var_os("LOCALAPPDATA")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .context("missing user directory")?,
    )
    .join("Unsloth Studio Desktop/launcher-settings.json"))
}
