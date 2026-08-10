// SPDX-License-Identifier: Apache-2.0

//! Typed application errors shown by the command-line interface.

use std::path::PathBuf;

/// Errors returned by Suzumushi's current commands.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// No configured root could be selected.
    #[error("no Suzumushi root found; run `suzumushi init ./suzumushi`")]
    RootMissing,
    /// A selected root is not safe or usable.
    #[error("invalid Suzumushi root {path:?}: {reason}")]
    InvalidRoot { path: PathBuf, reason: String },
    /// Initialization cannot safely use the destination.
    #[error("cannot initialize {path:?}: {reason}")]
    InitRefused { path: PathBuf, reason: String },
    /// A configuration file failed validation.
    #[error("invalid config: {0}")]
    InvalidConfig(String),
    /// Keeps parser diagnostics separate so the executable owns the error prefix.
    #[error("{0}")]
    InvalidArguments(String),
    /// A filesystem operation failed.
    #[error("{operation} {path:?}: {source}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// A secure Linux primitive was unavailable.
    #[error("unsupported secure operation: {0}")]
    Unsupported(String),
    /// A bounded helper process failed.
    #[error("metadata helper failed: {0}")]
    MetadataHelper(String),
    /// A lock cannot be acquired or stored safely.
    #[error("lock unavailable: {0}")]
    Lock(String),
    /// A bounded in-memory model could not reserve its validated capacity.
    #[error("resource unavailable: {0}")]
    Resource(String),
    /// Preserves both failures when logging fails beside the main operation.
    #[error("{primary}; additionally, {secondary}")]
    Multiple {
        primary: Box<AppError>,
        secondary: Box<AppError>,
    },
}

impl AppError {
    /// Adds stable operation and path context to an I/O error.
    #[must_use]
    pub fn io(operation: &'static str, path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            operation,
            path: path.into(),
            source,
        }
    }
}

/// Result type used by the application library.
pub type AppResult<T> = Result<T, AppError>;
