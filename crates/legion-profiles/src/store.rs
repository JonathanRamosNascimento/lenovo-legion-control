//! Storage of user profiles as JSON files in a directory
//! (`~/.config/legion-control/profiles` by default).
//!
//! Names are validated with [`crate::custom::validate_name`],
//! so a profile name can never escape the directory. Writes are atomic
//! (temporary file + rename).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use legion_core::{HwError, Result};

use crate::custom::{CustomProfile, validate_name};

/// A profile directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileStore {
    dir: PathBuf,
}

fn io(path: &Path, e: std::io::Error) -> HwError {
    HwError::Io {
        path: path.to_owned(),
        source: e,
    }
}

impl ProfileStore {
    /// Store in an explicit directory.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// Store in the user's default directory.
    pub fn user() -> Result<Self> {
        crate::custom::profiles_dir()
            .map(Self::new)
            .ok_or_else(|| HwError::NotSupported("cannot determine $HOME".into()))
    }

    /// Directory of this store.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path(&self, name: &str) -> Result<PathBuf> {
        validate_name(name)?;
        Ok(self.dir.join(format!("{name}.json")))
    }

    /// Profile names, sorted.
    pub fn list(&self) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(&self.dir)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .filter_map(|e| {
                        let n = e.file_name().to_string_lossy().into_owned();
                        let stem = n.strip_suffix(".json")?.to_owned();
                        validate_name(&stem).is_ok().then_some(stem)
                    })
                    .collect()
            })
            .unwrap_or_default();
        v.sort();
        v
    }

    /// Loads a profile.
    pub fn load(&self, name: &str) -> Result<CustomProfile> {
        let p = self.path(name)?;
        let text = fs::read_to_string(&p).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => {
                HwError::InvalidValue(format!("profile {name:?} does not exist"))
            }
            _ => io(&p, e),
        })?;
        CustomProfile::from_json(&text)
    }

    /// Saves a profile atomically.
    pub fn save(&self, name: &str, profile: &CustomProfile) -> Result<PathBuf> {
        let p = self.path(name)?;
        fs::create_dir_all(&self.dir).map_err(|e| io(&self.dir, e))?;
        let tmp = self.dir.join(format!(".{name}.json.tmp"));
        let text = serde_json::to_string_pretty(profile)
            .map_err(|e| HwError::InvalidValue(e.to_string()))?;
        let mut f = fs::File::create(&tmp).map_err(|e| io(&tmp, e))?;
        f.write_all(text.as_bytes())
            .and_then(|_| f.write_all(b"\n"))
            .and_then(|_| f.sync_all())
            .map_err(|e| io(&tmp, e))?;
        fs::rename(&tmp, &p).map_err(|e| io(&p, e))?;
        Ok(p)
    }

    /// Deletes a profile.
    pub fn delete(&self, name: &str) -> Result<()> {
        let p = self.path(name)?;
        fs::remove_file(&p).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => {
                HwError::InvalidValue(format!("profile {name:?} does not exist"))
            }
            _ => io(&p, e),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_load_list_delete() {
        let dir = tempfile::tempdir().unwrap();
        let store = ProfileStore::new(dir.path().join("profiles"));
        assert!(store.list().is_empty());
        let p = CustomProfile {
            platform_profile: Some("low-power".into()),
            battery_conservation: Some(true),
            ..Default::default()
        };
        store.save("quiet", &p).unwrap();
        store.save("gaming-1", &CustomProfile::default()).unwrap();
        std::fs::write(store.dir().join("bad name.json"), "{}").unwrap();
        assert_eq!(store.list(), ["gaming-1", "quiet"]);
        assert_eq!(store.load("quiet").unwrap(), p);
        store.delete("quiet").unwrap();
        assert!(store.load("quiet").is_err());
        assert!(store.delete("quiet").is_err());
        assert!(store.save("../evil", &p).is_err());
        assert!(store.load("../../etc/passwd").is_err());
    }
}
