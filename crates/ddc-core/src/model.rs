//! Monitor and VCP feature data model.

use serde::{Deserialize, Serialize};

/// A VESA MCCS VCP feature code.
///
/// Only the codes the port currently cares about are named; any other code can
/// be constructed with [`VcpCode::new`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct VcpCode(pub u8);

impl VcpCode {
    /// Display brightness (`0x10`).
    pub const BRIGHTNESS: Self = Self(0x10);
    /// Display contrast (`0x12`).
    pub const CONTRAST: Self = Self(0x12);
    /// Red channel gain (`0x16`).
    pub const RED_GAIN: Self = Self(0x16);
    /// Green channel gain (`0x18`).
    pub const GREEN_GAIN: Self = Self(0x18);
    /// Blue channel gain (`0x1a`).
    pub const BLUE_GAIN: Self = Self(0x1a);
    /// Audio volume (`0x62`).
    pub const VOLUME: Self = Self(0x62);
    /// Input source selection (`0x60`).
    pub const INPUT_SOURCE: Self = Self(0x60);
    /// Power mode (`0xd6`).
    pub const POWER_MODE: Self = Self(0xd6);

    /// Construct a feature code from its raw MCCS value.
    pub const fn new(code: u8) -> Self {
        Self(code)
    }

    /// Raw MCCS value.
    pub const fn raw(self) -> u8 {
        self.0
    }

    /// A short human-readable name, matching the labels used by `ddcui`.
    pub const fn name(self) -> &'static str {
        match self {
            Self::BRIGHTNESS => "Brightness",
            Self::CONTRAST => "Contrast",
            Self::RED_GAIN => "Red gain",
            Self::GREEN_GAIN => "Green gain",
            Self::BLUE_GAIN => "Blue gain",
            Self::VOLUME => "Volume",
            Self::INPUT_SOURCE => "Input source",
            Self::POWER_MODE => "Power mode",
            _ => "Unknown feature",
        }
    }
}

impl std::fmt::Display for VcpCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (0x{:02x})", self.name(), self.0)
    }
}

/// The current and maximum value of a continuous VCP feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VcpValue {
    /// Which feature this value belongs to.
    pub code: VcpCode,
    /// Current value reported by the monitor.
    pub current: u16,
    /// Maximum value reported by the monitor.
    pub max: u16,
}

impl VcpValue {
    /// The current value as a percentage of [`VcpValue::max`], `0..=100`.
    pub fn percent(&self) -> u8 {
        if self.max == 0 {
            0
        } else {
            ((u32::from(self.current) * 100) / u32::from(self.max)) as u8
        }
    }

    /// Build a value for `code` representing `percent` of `max`, rounding to the
    /// nearest whole step.
    pub fn from_percent(code: VcpCode, percent: u8, max: u16) -> Self {
        let clamped = percent.min(100);
        let current = ((u32::from(clamped) * u32::from(max) + 50) / 100) as u16;
        Self { code, current, max }
    }
}

/// Identifies a single DDC/CI capable monitor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonitorId {
    /// 1-based index used by `ddcutil --display`.
    pub display: u32,
    /// I2C bus device, e.g. `/dev/i2c-4`.
    pub bus: Option<String>,
    /// DRM connector, e.g. `card1-HDMI-A-1`.
    pub connector: Option<String>,
    /// Manufacturer reported in the EDID.
    pub manufacturer: Option<String>,
    /// Model reported in the EDID.
    pub model: Option<String>,
    /// Serial number reported in the EDID.
    pub serial: Option<String>,
}

impl MonitorId {
    /// A best-effort human-readable label for the monitor.
    pub fn label(&self) -> String {
        let manufacturer = self.manufacturer.as_deref().unwrap_or("").trim();
        let model = self.model.as_deref().unwrap_or("").trim();
        let mut label = format!("{manufacturer} {model}").trim().to_string();
        if label.is_empty() {
            label = format!("Display {}", self.display);
        }
        if let Some(connector) = &self.connector {
            label = format!("{label} ({connector})");
        }
        label
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_round_trips() {
        let value = VcpValue {
            code: VcpCode::BRIGHTNESS,
            current: 50,
            max: 100,
        };
        assert_eq!(value.percent(), 50);
        assert_eq!(VcpValue::from_percent(VcpCode::BRIGHTNESS, 50, 100).current, 50);
    }

    #[test]
    fn percent_handles_zero_max() {
        let value = VcpValue {
            code: VcpCode::BRIGHTNESS,
            current: 0,
            max: 0,
        };
        assert_eq!(value.percent(), 0);
    }

    #[test]
    fn from_percent_scales_to_range() {
        let value = VcpValue::from_percent(VcpCode::CONTRAST, 50, 255);
        assert_eq!(value.current, 128);
        assert_eq!(value.max, 255);
    }

    #[test]
    fn label_falls_back_to_display_number() {
        let monitor = MonitorId {
            display: 3,
            bus: None,
            connector: None,
            manufacturer: None,
            model: None,
            serial: None,
        };
        assert_eq!(monitor.label(), "Display 3");
    }

    #[test]
    fn label_includes_connector() {
        let monitor = MonitorId {
            display: 1,
            bus: Some("/dev/i2c-4".into()),
            connector: Some("card1-HDMI-A-1".into()),
            manufacturer: Some("Dell".into()),
            model: Some("U2419H".into()),
            serial: Some("ABC".into()),
        };
        assert_eq!(monitor.label(), "Dell U2419H (card1-HDMI-A-1)");
    }
}
