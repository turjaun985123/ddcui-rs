//! The transport-agnostic DDC/CI backend trait.

use crate::error::Result;
use crate::model::{MonitorId, VcpCode, VcpValue};

/// A source of monitors and their VCP features.
///
/// Implementations are expected to be cheap to share (they are used behind an
/// `Arc` by the GUI) and safe to call from any thread.
pub trait DdcBackend: Send + Sync {
    /// Short name of the backend, used for status messages.
    fn name(&self) -> &'static str;

    /// Whether the backend is usable on this machine (tool present, etc.).
    fn is_available(&self) -> bool;

    /// Enumerate connected DDC/CI capable monitors.
    fn detect(&self) -> Result<Vec<MonitorId>>;

    /// List the VCP features the monitor supports.
    fn capabilities(&self, monitor: &MonitorId) -> Result<Vec<VcpCode>>;

    /// Read the current value of a feature.
    fn get_vcp(&self, monitor: &MonitorId, code: VcpCode) -> Result<VcpValue>;

    /// Write a new value for a feature.
    fn set_vcp(&self, monitor: &MonitorId, code: VcpCode, value: u16) -> Result<()>;
}
