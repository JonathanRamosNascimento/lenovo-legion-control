//! Read-only access to sysfs / procfs.
//!
//! All hardware parsers receive a [`SysRoot`]. In production it points to `/`;
//! in tests it points to a temporary directory populated with fake kernel
//! files. Paths are always *compile-time constants or kernel-enumerated
//! names*; this module never receives paths from users, the GUI or D-Bus.
//!
//! The **only** write primitive of the whole project is [`write_attr`].
//! Every hardware change goes through it, so it can be audited in one place:
//! callers must validate values first; this function additionally refuses
//! paths outside `/sys`, logs the old and new value, and reads the attribute
//! back.

use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use crate::error::{HwError, Result};

/// Outcome of probing a file without reading it fully.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    /// File exists and can be read by the current user.
    Readable,
    /// File exists but reading requires more privileges.
    PermissionDenied,
    /// File does not exist.
    Missing,
    /// Some other error happened.
    Error,
}

/// Root of the (possibly fake) filesystem that contains `sys/` and `proc/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SysRoot {
    root: PathBuf,
}

impl Default for SysRoot {
    fn default() -> Self {
        Self::host()
    }
}

impl SysRoot {
    /// The real host filesystem (`/`).
    pub fn host() -> Self {
        Self {
            root: PathBuf::from("/"),
        }
    }

    /// A custom root, used for tests and fixtures.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Returns `true` when this root is the real host filesystem.
    pub fn is_host(&self) -> bool {
        self.root == Path::new("/")
    }

    /// The root directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolves an absolute kernel path (e.g. `/sys/class/hwmon`) inside this root.
    pub fn path(&self, abs: impl AsRef<Path>) -> PathBuf {
        let abs = abs.as_ref();
        let rel = abs.strip_prefix("/").unwrap_or(abs);
        self.root.join(rel)
    }

    /// Converts a path inside this root back to its kernel (absolute) form,
    /// for display and reports.
    pub fn display_path(&self, p: &Path) -> String {
        match p.strip_prefix(&self.root) {
            Ok(rel) => format!("/{}", rel.display()),
            Err(_) => p.display().to_string(),
        }
    }

    /// Returns `true` if the path exists.
    pub fn exists(&self, abs: impl AsRef<Path>) -> bool {
        self.path(abs).exists()
    }

    /// Reads a file and trims surrounding whitespace.
    pub fn read(&self, abs: impl AsRef<Path>) -> Result<String> {
        read_trimmed(&self.path(abs))
    }

    /// Reads a file; returns `None` if it is missing or unreadable.
    pub fn read_opt(&self, abs: impl AsRef<Path>) -> Option<String> {
        read_opt(&self.path(abs))
    }

    /// Reads and parses a file.
    pub fn read_parse<T: FromStr>(&self, abs: impl AsRef<Path>, what: &'static str) -> Result<T> {
        read_parse(&self.path(abs), what)
    }

    /// Reads and parses a file, returning `None` on any failure.
    pub fn read_parse_opt<T: FromStr>(&self, abs: impl AsRef<Path>) -> Option<T> {
        read_parse_opt(&self.path(abs))
    }

    /// Lists entry names of a directory, sorted. Missing directory → empty list.
    pub fn list(&self, abs: impl AsRef<Path>) -> Vec<String> {
        list_dir(&self.path(abs))
    }

    /// Probes whether a file can be read.
    pub fn access(&self, abs: impl AsRef<Path>) -> Access {
        access(&self.path(abs))
    }
}

/// Writes a sysfs attribute (kernel path, resolved inside `root`).
///
/// * Only paths below `/sys/` are accepted.
/// * The attribute must already exist (never creates files).
/// * The previous value and the new value are logged at `info` level.
/// * Returns the value read back after the write.
pub fn write_attr(root: &SysRoot, abs: &str, value: &str) -> Result<String> {
    use std::io::Write;
    if !abs.starts_with("/sys/") || abs.contains("..") {
        return Err(HwError::InvalidValue(format!(
            "refusing to write outside /sys: {abs}"
        )));
    }
    if value.contains('\n') || value.len() > 64 {
        return Err(HwError::InvalidValue(
            "attribute value too long or multi-line".into(),
        ));
    }
    let path = root.path(abs);
    let old = read_opt(&path);
    let mut f = fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(&path)
        .map_err(|e| HwError::from_io(&path, e))?;
    f.write_all(value.as_bytes())
        .map_err(|e| match e.raw_os_error() {
            Some(16) => HwError::Backend {
                backend: "sysfs",
                message: format!("{abs}: device or resource busy"),
            },
            Some(22) => HwError::InvalidValue(format!("kernel rejected {value:?} for {abs}")),
            _ => HwError::from_io(&path, e),
        })?;
    drop(f);
    let new = read_opt(&path).unwrap_or_default();
    tracing::info!(target: "legion_audit", path = abs, old = old.as_deref().unwrap_or("?"), requested = value, new = %new, "hardware setting changed");
    Ok(new)
}

/// Reads a file (already resolved) and trims whitespace.
pub fn read_trimmed(path: &Path) -> Result<String> {
    fs::read_to_string(path)
        .map(|s| s.trim().to_owned())
        .map_err(|e| HwError::from_io(path, e))
}

/// Reads a file; `None` if missing/unreadable (logged at `trace`).
pub fn read_opt(path: &Path) -> Option<String> {
    match read_trimmed(path) {
        Ok(v) => Some(v),
        Err(e) => {
            tracing::trace!(path = %path.display(), error = %e, "optional read failed");
            None
        }
    }
}

/// Reads and parses a file.
pub fn read_parse<T: FromStr>(path: &Path, what: &'static str) -> Result<T> {
    let raw = read_trimmed(path)?;
    raw.parse::<T>().map_err(|_| HwError::Parse {
        what,
        path: path.to_owned(),
        value: raw,
    })
}

/// Reads and parses a file, `None` on failure.
pub fn read_parse_opt<T: FromStr>(path: &Path) -> Option<T> {
    read_opt(path).and_then(|s| s.parse().ok())
}

/// Sorted directory listing. Missing directory → empty list.
pub fn list_dir(path: &Path) -> Vec<String> {
    let mut out: Vec<String> = match fs::read_dir(path) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect(),
        Err(_) => Vec::new(),
    };
    out.sort_by(|a, b| natural_cmp(a, b));
    out
}

/// Probes a file for readability by opening it (no data is consumed from
/// regular sysfs attributes beyond the open call).
pub fn access(path: &Path) -> Access {
    match fs::File::open(path) {
        Ok(_) => Access::Readable,
        Err(e) => match e.kind() {
            std::io::ErrorKind::NotFound => Access::Missing,
            std::io::ErrorKind::PermissionDenied => Access::PermissionDenied,
            _ => Access::Error,
        },
    }
}

/// Returns `true` if the file mode has any write bit set.
///
/// This only inspects permission bits; it never opens the file for writing.
pub fn is_writable_mode(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path)
        .map(|m| m.permissions().mode() & 0o222 != 0)
        .unwrap_or(false)
}

/// Name of the target of a symlink (e.g. the `driver` link of a device).
pub fn link_name(path: &Path) -> Option<String> {
    fs::read_link(path)
        .ok()
        .and_then(|t| t.file_name().map(|n| n.to_string_lossy().into_owned()))
}

/// Natural ordering so that `hwmon10` sorts after `hwmon2`.
pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    fn split(s: &str) -> (&str, Option<u64>) {
        let idx = s
            .char_indices()
            .rev()
            .take_while(|(_, c)| c.is_ascii_digit())
            .last()
            .map(|(i, _)| i);
        match idx {
            Some(i) => (&s[..i], s[i..].parse().ok()),
            None => (s, None),
        }
    }
    let (pa, na) = split(a);
    let (pb, nb) = split(b);
    pa.cmp(pb).then(na.cmp(&nb)).then(a.cmp(b))
}

/// Parses a kernel CPU list such as `0-7,16,18-19` into a count.
pub fn cpu_list_len(list: &str) -> Option<usize> {
    let mut n = 0usize;
    for part in list.trim().split(',').filter(|p| !p.is_empty()) {
        match part.split_once('-') {
            Some((a, b)) => {
                let a: usize = a.trim().parse().ok()?;
                let b: usize = b.trim().parse().ok()?;
                if b < a {
                    return None;
                }
                n += b - a + 1;
            }
            None => {
                part.trim().parse::<usize>().ok()?;
                n += 1;
            }
        }
    }
    Some(n)
}

/// Parses a kernel "choice" list such as `Fast Standard [Long_Life]`,
/// returning `(choices, selected)`.
pub fn parse_bracket_choices(raw: &str) -> (Vec<String>, Option<String>) {
    let mut selected = None;
    let choices = raw
        .split_whitespace()
        .map(|tok| {
            if let Some(inner) = tok.strip_prefix('[').and_then(|t| t.strip_suffix(']')) {
                selected = Some(inner.to_owned());
                inner.to_owned()
            } else {
                tok.to_owned()
            }
        })
        .collect();
    (choices, selected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_sort() {
        let mut v = vec!["hwmon10", "hwmon2", "hwmon1"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, ["hwmon1", "hwmon2", "hwmon10"]);
    }

    #[test]
    fn cpu_lists() {
        assert_eq!(cpu_list_len("0-7"), Some(8));
        assert_eq!(cpu_list_len("8-19"), Some(12));
        assert_eq!(cpu_list_len("0,2,4-5\n"), Some(4));
        assert_eq!(cpu_list_len(""), Some(0));
        assert_eq!(cpu_list_len("5-2"), None);
        assert_eq!(cpu_list_len("x"), None);
    }

    #[test]
    fn bracket_choices() {
        let (c, s) = parse_bracket_choices("Fast Standard [Long_Life]");
        assert_eq!(c, ["Fast", "Standard", "Long_Life"]);
        assert_eq!(s.as_deref(), Some("Long_Life"));
        let (c, s) = parse_bracket_choices("performance powersave");
        assert_eq!(c.len(), 2);
        assert!(s.is_none());
    }

    #[test]
    fn root_resolution() {
        let r = SysRoot::new("/tmp/fake");
        assert_eq!(r.path("/sys/class"), PathBuf::from("/tmp/fake/sys/class"));
        assert_eq!(r.display_path(Path::new("/tmp/fake/sys/x")), "/sys/x");
        assert!(SysRoot::host().is_host());
        assert!(!r.is_host());
    }

    #[test]
    fn write_attr_rules() {
        let dir = tempfile::tempdir().unwrap();
        let root = SysRoot::new(dir.path());
        std::fs::create_dir_all(root.path("/sys/a")).unwrap();
        std::fs::write(root.path("/sys/a/v"), "old\n").unwrap();
        assert_eq!(write_attr(&root, "/sys/a/v", "new").unwrap(), "new");
        // never creates files
        assert!(write_attr(&root, "/sys/a/missing", "1").is_err());
        assert!(!root.exists("/sys/a/missing"));
        // outside /sys or traversal
        assert!(write_attr(&root, "/etc/passwd", "x").is_err());
        assert!(write_attr(&root, "/sys/../etc/passwd", "x").is_err());
        assert!(write_attr(&root, "/sys/a/v", "a\nb").is_err());
    }

    #[test]
    fn read_helpers() {
        let dir = tempfile::tempdir().unwrap();
        let root = SysRoot::new(dir.path());
        std::fs::create_dir_all(root.path("/sys/a")).unwrap();
        std::fs::write(root.path("/sys/a/v"), " 42\n").unwrap();
        assert_eq!(root.read("/sys/a/v").unwrap(), "42");
        assert_eq!(root.read_parse_opt::<u32>("/sys/a/v"), Some(42));
        assert!(root.read_opt("/sys/a/missing").is_none());
        assert!(matches!(
            root.read("/sys/a/missing"),
            Err(HwError::NotSupported(_))
        ));
        assert_eq!(root.access("/sys/a/v"), Access::Readable);
        assert_eq!(root.access("/sys/a/nope"), Access::Missing);
        assert_eq!(root.list("/sys/a"), ["v"]);
        assert!(root.list("/sys/none").is_empty());
    }
}
