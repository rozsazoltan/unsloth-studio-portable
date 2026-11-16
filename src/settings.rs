use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub root: String,
    pub hf_home: String,
    pub port: u16,
    pub folder_name: String,
    pub force_local_mise: bool,
}

pub const DEFAULT_FOLDER_NAME: &str = "Unsloth Studio Desktop";

impl Default for Settings {
    fn default() -> Self {
        let parent = std::env::var("USERPROFILE").unwrap_or_else(|_| ".".into());
        let root = PathBuf::from(parent).join(DEFAULT_FOLDER_NAME);
        Self {
            hf_home: root.join("huggingface").to_string_lossy().into(),
            root: root.to_string_lossy().into(),
            port: 8888,
            folder_name: DEFAULT_FOLDER_NAME.into(),
            force_local_mise: false,
        }
    }
}

impl Settings {
    pub fn normalized_folder_name(&self) -> String {
        let name = self.folder_name.trim();
        if name.is_empty() {
            DEFAULT_FOLDER_NAME.into()
        } else {
            name.into()
        }
    }

    /// Rename the product folder inside the same parent directory, move an
    /// existing installation on disk when `move_folder` is set, and rebase a
    /// Hugging Face folder that lived inside the old root. The caller re-checks
    /// the new path.
    pub fn apply_folder_name(&mut self, move_folder: bool) -> Result<()> {
        let name = self.normalized_folder_name();
        let old_root = PathBuf::from(&self.root);
        self.folder_name = name.clone();
        let Some(parent) = old_root.parent() else {
            return Ok(());
        };
        let new_root = parent.join(&name);
        self.root = new_root.to_string_lossy().into();
        if PathBuf::from(&self.hf_home).starts_with(&old_root) {
            if let Ok(rest) = PathBuf::from(&self.hf_home).strip_prefix(&old_root) {
                self.hf_home = new_root.join(rest).to_string_lossy().into();
            }
        }
        if move_folder && old_root != new_root && old_root.is_dir() {
            if new_root.exists() {
                anyhow::bail!("invalid_argument: a folder with that name already exists. Pick another name, or keep the old folder and use the new one.");
            }
            std::fs::rename(&old_root, &new_root).context("failed to rename the install folder")?;
        }
        Ok(())
    }

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
