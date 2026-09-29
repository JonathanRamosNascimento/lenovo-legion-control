//! GUI preferences (`~/.config/legion-control/gui.json`).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Stored preferences.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Interface language code (`en`, `pt-BR`); `None` follows the system.
    pub language: Option<String>,
}

fn path() -> PathBuf {
    gtk::glib::user_config_dir()
        .join("legion-control")
        .join("gui.json")
}

impl Settings {
    /// Loads the preferences (defaults when missing or invalid).
    pub fn load() -> Self {
        std::fs::read_to_string(path())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    /// Saves the preferences.
    pub fn save(&self) -> std::io::Result<()> {
        let p = path();
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(p, text + "\n")
    }

    /// Language to use: the stored one, else the system's.
    pub fn effective_lang(&self) -> legion_i18n::Lang {
        self.language
            .as_deref()
            .and_then(legion_i18n::Lang::parse)
            .unwrap_or_else(legion_i18n::detect)
    }
}
