//! Error type shared by the Legion Control crates.

use std::path::PathBuf;

/// Result alias using [`HwError`].
pub type Result<T, E = HwError> = std::result::Result<T, E>;

/// Errors produced while talking to hardware interfaces.
#[derive(Debug, thiserror::Error)]
pub enum HwError {
    /// The interface does not exist on this machine.
    #[error("not supported on this hardware: {0}")]
    NotSupported(String),

    /// The interface exists but the current user is not allowed to use it.
    #[error("administrator permission required: {}", .0.display())]
    PermissionDenied(PathBuf),

    /// Any other I/O error while reading an interface.
    #[error("I/O error on {}: {source}", path.display())]
    Io {
        /// Path that failed.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },

    /// A value read from the kernel could not be parsed.
    #[error("could not parse {what} from {}: {value:?}", path.display())]
    Parse {
        /// Which value we were trying to parse.
        what: &'static str,
        /// Path it came from.
        path: PathBuf,
        /// Raw value.
        value: String,
    },

    /// A value supplied by the user/caller was rejected by validation.
    #[error("invalid value: {0}")]
    InvalidValue(String),

    /// The operation modifies hardware state and is not available yet.
    ///
    /// Phase 1 of Legion Control is strictly read-only.
    #[error("{0} is not available yet: this build of Legion Control is read-only")]
    ReadOnly(&'static str),

    /// Error from an external backend (D-Bus, NVML, ...).
    #[error("{backend}: {message}")]
    Backend {
        /// Backend name.
        backend: &'static str,
        /// Human readable message.
        message: String,
    },
}

impl HwError {
    /// Builds an [`HwError`] from an I/O error, mapping permission errors
    /// to [`HwError::PermissionDenied`].
    pub fn from_io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        let path = path.into();
        match source.kind() {
            std::io::ErrorKind::PermissionDenied => HwError::PermissionDenied(path),
            std::io::ErrorKind::NotFound => HwError::NotSupported(path.display().to_string()),
            _ => HwError::Io { path, source },
        }
    }
}
