use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct TuiConfig {
    pub url:   String,
    pub token: String,
}

fn config_path() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(PathBuf::from(home)
        .join(".config")
        .join("freight-registry")
        .join("tui.toml"))
}

impl TuiConfig {
    pub fn load() -> Option<Self> {
        let s = std::fs::read_to_string(config_path()?).ok()?;
        toml::from_str(&s).ok()
    }

    pub fn save(&self) {
        let Some(path) = config_path() else { return };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(s) = toml::to_string(self) {
            let _ = std::fs::write(path, s);
        }
    }
}
