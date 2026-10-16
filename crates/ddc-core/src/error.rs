//! Error type shared by all [`crate::DdcBackend`] implementations.

use thiserror::Error;

/// Errors produced while talking to a monitor.
#[derive(Debug, Error)]
pub enum DdcError {
    /// The backend cannot be used (e.g. the `ddcutil` executable is missing).
    #[error("backend unavailable: {0}")]
    Unavailable(String),

    /// No monitor with the requested display number was found.
    #[error("monitor {0} not found")]
    MonitorNotFound(u32),

    /// The monitor does not expose the requested VCP feature.
    #[error("monitor {monitor} does not support VCP feature 0x{code:02x}")]
    FeatureNotSupported { monitor: u32, code: u8 },

    /// A requested value is outside the feature's valid range.
    #[error("value {value} out of range 0..={max}")]
    OutOfRange { value: u16, max: u16 },

    /// An external command exited with a failure status.
    #[error("command failed (status {status:?}): {stderr}")]
    Command { status: Option<i32>, stderr: String },

    /// A backend output could not be parsed.
    #[error("failed to parse backend output: {0}")]
    Parse(String),

    /// Underlying I/O failure.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, DdcError>;
