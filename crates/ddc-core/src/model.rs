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

    /// Manufacturer and model without the connection, e.g. `MSI MP225`.
    pub fn name(&self) -> String {
        let manufacturer = self.manufacturer.as_deref().unwrap_or("").trim();
        let model = self.model.as_deref().unwrap_or("").trim();
        let name = format!("{manufacturer} {model}").trim().to_string();
        if name.is_empty() {
            format!("Display {}", self.display)
        } else {
            name
        }
    }

    /// Where the monitor is attached, e.g. `display 1 · card1-HDMI-A-1`.
    pub fn detail(&self) -> String {
        let mut parts = vec![format!("display {}", self.display)];
        if let Some(connector) = self
            .connector
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            parts.push(connector.to_string());
        }
        if let Some(bus) = self.bus.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            parts.push(bus.to_string());
        }
        parts.join(" · ")
    }

    /// A key that stays the same for one physical monitor.
    ///
    /// The display number is handed out in the order monitors are found, so it
    /// is only the last resort: the serial number in the EDID names the monitor
    /// itself, and the connector names the port it is plugged into.
    pub fn key(&self) -> String {
        let serial = self.serial.as_deref().unwrap_or("").trim();
        if !serial.is_empty() {
            let manufacturer = self
                .manufacturer
                .as_deref()
                .unwrap_or("")
                .trim()
                .to_lowercase();
            let model = self.model.as_deref().unwrap_or("").trim().to_lowercase();
            return format!("{manufacturer}:{model}:{serial}");
        }
        let connector = self.connector.as_deref().unwrap_or("").trim();
        if !connector.is_empty() {
            return format!("connector:{connector}");
        }
        let bus = self.bus.as_deref().unwrap_or("").trim();
        if !bus.is_empty() {
            return format!("bus:{bus}");
        }
        format!("display:{}", self.display)
    }
}

/// Keys for a whole monitor list, numbered when two monitors share one.
///
/// Two monitors of the same model whose EDID carries no serial number cannot be
/// told apart, so they keep the order they were found in rather than silently
/// sharing one identity.
pub fn unique_keys(monitors: &[MonitorId]) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    for monitor in monitors {
        let base = monitor.key();
        if !keys.contains(&base) {
            keys.push(base);
            continue;
        }
        let mut number = 2;
        loop {
            let candidate = format!("{base}#{number}");
            if !keys.contains(&candidate) {
                keys.push(candidate);
                break;
            }
            number += 1;
        }
    }
    keys
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
        assert_eq!(
            VcpValue::from_percent(VcpCode::BRIGHTNESS, 50, 100).current,
            50
        );
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

    /// A monitor with everything the EDID reports.
    fn well_known() -> MonitorId {
        MonitorId {
            display: 1,
            bus: Some("/dev/i2c-4".into()),
            connector: Some("card1-HDMI-A-1".into()),
            manufacturer: Some("MSI".into()),
            model: Some("MP225".into()),
            serial: Some("PC6M076302771".into()),
        }
    }

    #[test]
    fn name_and_detail_describe_a_card() {
        let monitor = well_known();
        assert_eq!(monitor.name(), "MSI MP225");
        assert_eq!(monitor.detail(), "display 1 · card1-HDMI-A-1 · /dev/i2c-4");
        assert_eq!(monitor.key(), "msi:mp225:PC6M076302771");
    }

    #[test]
    fn the_key_survives_a_new_display_number() {
        let mut monitor = well_known();
        let before = monitor.key();
        monitor.display = 2;
        assert_eq!(monitor.key(), before);
    }

    #[test]
    fn the_key_falls_back_from_serial_to_connection_to_bus() {
        let mut monitor = well_known();
        monitor.serial = None;
        assert_eq!(monitor.key(), "connector:card1-HDMI-A-1");
        monitor.connector = None;
        assert_eq!(monitor.key(), "bus:/dev/i2c-4");
        monitor.bus = None;
        assert_eq!(monitor.key(), "display:1");
    }

    #[test]
    fn identical_monitors_get_their_own_keys() {
        let mut monitor = well_known();
        monitor.serial = None;
        let mut second = monitor.clone();
        second.display = 2;
        let mut third = monitor.clone();
        third.display = 3;
        third.connector = Some("card1-HDMI-A-2".into());

        let keys = unique_keys(&[monitor.clone(), second, third]);
        assert_eq!(
            keys,
            vec![
                "connector:card1-HDMI-A-1".to_string(),
                "connector:card1-HDMI-A-1#2".to_string(),
                "connector:card1-HDMI-A-2".to_string(),
            ]
        );
        assert_eq!(keys.len(), 3);
    }

    #[test]
    fn name_falls_back_to_the_display_number() {
        let monitor = MonitorId {
            display: 4,
            bus: None,
            connector: None,
            manufacturer: None,
            model: None,
            serial: None,
        };
        assert_eq!(monitor.name(), "Display 4");
        assert_eq!(monitor.detail(), "display 4");
        assert_eq!(monitor.key(), "display:4");
        assert!(unique_keys(&[]).is_empty());
    }
}
