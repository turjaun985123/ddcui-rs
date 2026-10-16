//! An in-memory [`DdcBackend`] for tests and the fallback demo UI.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::backend::DdcBackend;
use crate::error::{DdcError, Result};
use crate::model::{MonitorId, VcpCode, VcpValue};

/// A fake monitor with a fixed set of feature values.
#[derive(Debug, Clone)]
pub struct MockMonitor {
    /// Identity of the fake monitor.
    pub id: MonitorId,
    /// Feature values it reports.
    pub values: HashMap<VcpCode, VcpValue>,
}

impl MockMonitor {
    /// Create a monitor with no features.
    pub fn new(id: MonitorId) -> Self {
        Self {
            id,
            values: HashMap::new(),
        }
    }

    /// Add (or replace) a feature value.
    pub fn with_value(mut self, value: VcpValue) -> Self {
        self.values.insert(value.code, value);
        self
    }
}

/// A backend backed entirely by memory.
#[derive(Debug)]
pub struct MockBackend {
    monitors: Mutex<Vec<MockMonitor>>,
    available: bool,
}

impl Default for MockBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl MockBackend {
    /// An empty, available backend.
    pub fn new() -> Self {
        Self {
            monitors: Mutex::new(Vec::new()),
            available: true,
        }
    }

    /// A backend that reports itself as unavailable.
    pub fn unavailable() -> Self {
        Self {
            monitors: Mutex::new(Vec::new()),
            available: false,
        }
    }

    /// A backend with a single demo monitor (used when `ddcutil` is missing).
    pub fn demo() -> Self {
        let id = MonitorId {
            display: 1,
            bus: Some("/dev/i2c-4".into()),
            connector: Some("card1-HDMI-A-1".into()),
            manufacturer: Some("Demo".into()),
            model: Some("Mock Monitor".into()),
            serial: None,
        };
        let monitor = MockMonitor::new(id)
            .with_value(VcpValue {
                code: VcpCode::BRIGHTNESS,
                current: 50,
                max: 100,
            })
            .with_value(VcpValue {
                code: VcpCode::CONTRAST,
                current: 75,
                max: 100,
            });
        Self::new().with_monitor(monitor)
    }

    /// Add a monitor to the backend.
    pub fn with_monitor(self, monitor: MockMonitor) -> Self {
        self.monitors
            .lock()
            .expect("mock mutex poisoned")
            .push(monitor);
        self
    }
}

impl DdcBackend for MockBackend {
    fn name(&self) -> &'static str {
        "mock"
    }

    fn is_available(&self) -> bool {
        self.available
    }

    fn detect(&self) -> Result<Vec<MonitorId>> {
        let monitors = self.monitors.lock().expect("mock mutex poisoned");
        Ok(monitors.iter().map(|monitor| monitor.id.clone()).collect())
    }

    fn capabilities(&self, monitor: &MonitorId) -> Result<Vec<VcpCode>> {
        let monitors = self.monitors.lock().expect("mock mutex poisoned");
        let found = monitors
            .iter()
            .find(|candidate| candidate.id.display == monitor.display)
            .ok_or(DdcError::MonitorNotFound(monitor.display))?;
        let mut codes: Vec<VcpCode> = found.values.keys().copied().collect();
        codes.sort_unstable();
        Ok(codes)
    }

    fn get_vcp(&self, monitor: &MonitorId, code: VcpCode) -> Result<VcpValue> {
        let monitors = self.monitors.lock().expect("mock mutex poisoned");
        let found = monitors
            .iter()
            .find(|candidate| candidate.id.display == monitor.display)
            .ok_or(DdcError::MonitorNotFound(monitor.display))?;
        found
            .values
            .get(&code)
            .copied()
            .ok_or(DdcError::FeatureNotSupported {
                monitor: monitor.display,
                code: code.raw(),
            })
    }

    fn set_vcp(&self, monitor: &MonitorId, code: VcpCode, value: u16) -> Result<()> {
        let mut monitors = self.monitors.lock().expect("mock mutex poisoned");
        let found = monitors
            .iter_mut()
            .find(|candidate| candidate.id.display == monitor.display)
            .ok_or(DdcError::MonitorNotFound(monitor.display))?;
        let slot = found
            .values
            .get_mut(&code)
            .ok_or(DdcError::FeatureNotSupported {
                monitor: monitor.display,
                code: code.raw(),
            })?;
        if value > slot.max {
            return Err(DdcError::OutOfRange {
                value,
                max: slot.max,
            });
        }
        slot.current = value;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_and_set_brightness() {
        let backend = MockBackend::demo();
        let monitor = backend.detect().unwrap().remove(0);

        let initial = backend.get_vcp(&monitor, VcpCode::BRIGHTNESS).unwrap();
        assert_eq!(initial.current, 50);

        backend.set_vcp(&monitor, VcpCode::BRIGHTNESS, 80).unwrap();
        let updated = backend.get_vcp(&monitor, VcpCode::BRIGHTNESS).unwrap();
        assert_eq!(updated.current, 80);
    }

    #[test]
    fn set_out_of_range_is_rejected() {
        let backend = MockBackend::demo();
        let monitor = backend.detect().unwrap().remove(0);
        assert!(backend.set_vcp(&monitor, VcpCode::BRIGHTNESS, 101).is_err());
    }

    #[test]
    fn unsupported_feature_is_reported() {
        let backend = MockBackend::demo();
        let monitor = backend.detect().unwrap().remove(0);
        assert!(backend.get_vcp(&monitor, VcpCode::VOLUME).is_err());
    }

    #[test]
    fn capabilities_are_sorted() {
        let backend = MockBackend::demo();
        let monitor = backend.detect().unwrap().remove(0);
        let caps = backend.capabilities(&monitor).unwrap();
        assert_eq!(caps, vec![VcpCode::BRIGHTNESS, VcpCode::CONTRAST]);
    }
}
