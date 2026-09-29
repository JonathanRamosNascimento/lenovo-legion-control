//! `~/.config/autostart/legion-control.desktop` management.
//!
//! Only created/removed when the user toggles "Start at Login".

use std::path::PathBuf;

const ENTRY: &str = "[Desktop Entry]
Type=Application
Name=Legion Control
Comment=Lenovo Legion monitoring and control
Comment[pt_BR]=Monitoramento e controle de notebooks Lenovo Legion
Exec=legion-control --hidden
Icon=com.legion.Control
Terminal=false
Categories=System;Settings;HardwareSettings;
X-GNOME-Autostart-enabled=true
";

fn path() -> PathBuf {
    gtk::glib::user_config_dir()
        .join("autostart")
        .join("legion-control.desktop")
}

/// Whether the autostart entry exists.
pub fn is_enabled() -> bool {
    path().exists()
}

/// Creates or removes the autostart entry.
pub fn set(enabled: bool) -> std::io::Result<()> {
    let p = path();
    if enabled {
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&p, ENTRY)
    } else {
        match std::fs::remove_file(&p) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}
