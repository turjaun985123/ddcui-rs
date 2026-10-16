//! A [`DdcBackend`] that shells out to the `ddcutil` command line tool.
//!
//! This mirrors what the original `ddcui` does through `libddcutil`, but keeps
//! the process boundary explicit so the core crate stays free of native
//! dependencies and is easy to test.

use std::process::Command;

use crate::backend::DdcBackend;
use crate::error::{DdcError, Result};
use crate::model::{MonitorId, VcpCode, VcpValue};

/// Backend that drives the external `ddcutil` executable.
#[derive(Debug, Clone)]
pub struct DdcutilCli {
    program: String,
}

impl Default for DdcutilCli {
    fn default() -> Self {
        Self {
            program: "ddcutil".to_string(),
        }
    }
}

impl DdcutilCli {
    /// Use the `ddcutil` found on `PATH`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Use a specific executable instead of `ddcutil`.
    pub fn with_program(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
        }
    }

    fn run(&self, args: &[&str]) -> Result<String> {
        let output = Command::new(&self.program)
            .args(args)
            .output()
            .map_err(|err| {
                if err.kind() == std::io::ErrorKind::NotFound {
                    DdcError::Unavailable(format!("'{}' was not found in PATH", self.program))
                } else {
                    DdcError::Io(err)
                }
            })?;

        if !output.status.success() {
            return Err(DdcError::Command {
                status: output.status.code(),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
            });
        }

        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

impl DdcBackend for DdcutilCli {
    fn name(&self) -> &'static str {
        "ddcutil"
    }

    fn is_available(&self) -> bool {
        Command::new(&self.program)
            .arg("--version")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }

    fn detect(&self) -> Result<Vec<MonitorId>> {
        Ok(parse_detect(&self.run(&["detect"])?))
    }

    fn capabilities(&self, monitor: &MonitorId) -> Result<Vec<VcpCode>> {
        let display = monitor.display.to_string();
        let output = self.run(&["capabilities", "--display", &display, "--brief"])?;
        Ok(parse_capabilities(&output))
    }

    fn get_vcp(&self, monitor: &MonitorId, code: VcpCode) -> Result<VcpValue> {
        let display = monitor.display.to_string();
        let code_arg = format!("{:02x}", code.raw());
        let output = self.run(&["getvcp", &code_arg, "--display", &display, "--brief"])?;
        parse_getvcp(code, &output)
    }

    fn set_vcp(&self, monitor: &MonitorId, code: VcpCode, value: u16) -> Result<()> {
        let display = monitor.display.to_string();
        let code_arg = format!("{:02x}", code.raw());
        let value_arg = value.to_string();
        self.run(&["setvcp", &code_arg, &value_arg, "--display", &display])?;
        Ok(())
    }
}

/// Parse the output of `ddcutil detect`.
pub fn parse_detect(output: &str) -> Vec<MonitorId> {
    let mut monitors: Vec<MonitorId> = Vec::new();

    for raw in output.lines() {
        let line = raw.trim();

        if let Some(rest) = line.strip_prefix("Display ") {
            let display = rest
                .split_whitespace()
                .next()
                .and_then(|value| value.parse::<u32>().ok())
                .unwrap_or(monitors.len() as u32 + 1);
            monitors.push(MonitorId {
                display,
                bus: None,
                connector: None,
                manufacturer: None,
                model: None,
                serial: None,
            });
            continue;
        }

        let Some(monitor) = monitors.last_mut() else {
            continue;
        };

        if let Some(value) = line.strip_prefix("I2C bus:") {
            monitor.bus = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("DRM connector:") {
            monitor.connector = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("Mfg id:") {
            apply_mfg_id(monitor, value);
        } else if let Some(value) = line.strip_prefix("Model:") {
            fill_if_empty(&mut monitor.model, value);
        } else if let Some(value) = line.strip_prefix("Serial number:") {
            fill_if_empty(&mut monitor.serial, value);
        }
    }

    monitors
}

fn apply_mfg_id(monitor: &mut MonitorId, value: &str) {
    let value = value.trim();
    match value.split_once(" - ") {
        Some((manufacturer, model)) => {
            if !manufacturer.trim().is_empty() {
                monitor.manufacturer = Some(manufacturer.trim().to_string());
            }
            fill_if_empty(&mut monitor.model, model);
        }
        None => fill_if_empty(&mut monitor.manufacturer, value),
    }
}

fn fill_if_empty(slot: &mut Option<String>, value: &str) {
    let value = value.trim();
    if slot.is_none() && !value.is_empty() {
        *slot = Some(value.to_string());
    }
}

/// Parse the `--brief` output of `ddcutil capabilities` into a feature list.
pub fn parse_capabilities(output: &str) -> Vec<VcpCode> {
    let mut codes = Vec::new();
    for raw in output.lines() {
        let line = raw.trim();
        if let Some(rest) = line.strip_prefix("Feature:") {
            let token = rest.split_whitespace().next().unwrap_or("");
            if let Ok(value) = u8::from_str_radix(token.trim_start_matches("0x"), 16) {
                codes.push(VcpCode::new(value));
            }
        }
    }
    codes
}

/// Parse the output of `ddcutil getvcp <code>`.
///
/// Handles both the `--brief` form (`VCP 10 C 50 100`) and the long form
/// (`VCP code 0x10 (Brightness): current value = 50, max value = 100`).
pub fn parse_getvcp(code: VcpCode, output: &str) -> Result<VcpValue> {
    for raw in output.lines() {
        let line = raw.trim();

        let parsed = line
            .strip_prefix("VCP code")
            .and_then(|rest| parse_long_form(code, rest))
            .or_else(|| {
                line.strip_prefix("VCP ")
                    .and_then(|rest| parse_brief_form(code, rest))
            });

        if let Some(value) = parsed {
            return Ok(value);
        }
    }

    Err(DdcError::Parse(format!(
        "no VCP value found in output for 0x{:02x}",
        code.raw()
    )))
}

fn parse_brief_form(code: VcpCode, rest: &str) -> Option<VcpValue> {
    let mut tokens = rest.split_whitespace();
    let _feature = tokens.next()?;
    let _kind = tokens.next()?;
    let current = tokens.next()?.parse::<u16>().ok()?;
    let max = tokens.next()?.parse::<u16>().ok()?;
    Some(VcpValue { code, current, max })
}

fn parse_long_form(code: VcpCode, rest: &str) -> Option<VcpValue> {
    let current = number_after(rest, "current value =")?;
    let max = number_after(rest, "max value =")?;
    Some(VcpValue { code, current, max })
}

fn number_after(haystack: &str, needle: &str) -> Option<u16> {
    let tail = haystack.split(needle).nth(1)?;
    let digits = tail
        .trim_start()
        .split(|c: char| !c.is_ascii_digit())
        .next()?;
    digits.parse::<u16>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DETECT_OUTPUT: &str = "\
Display 1
   I2C bus:  /dev/i2c-4
   DRM connector:           card1-HDMI-A-1
   EDID synopsis:
      Mfg id:               DEL - Dell Inc.
      Model:                DELL U2419H
      Product code:         1234
      Serial number:        ABC123
      Binary serial number: 16843009 (0x01010101)
      Manufacture year:     2019,  Week: 22
   VCP version:         2.1

Display 2
   I2C bus:  /dev/i2c-5
   Mfg id:               GSM - LG Electronics
";

    #[test]
    fn parses_two_monitors() {
        let monitors = parse_detect(DETECT_OUTPUT);
        assert_eq!(monitors.len(), 2);

        assert_eq!(monitors[0].display, 1);
        assert_eq!(monitors[0].bus.as_deref(), Some("/dev/i2c-4"));
        assert_eq!(monitors[0].connector.as_deref(), Some("card1-HDMI-A-1"));
        assert_eq!(monitors[0].manufacturer.as_deref(), Some("DEL"));
        assert_eq!(monitors[0].model.as_deref(), Some("DELL U2419H"));
        assert_eq!(monitors[0].serial.as_deref(), Some("ABC123"));

        assert_eq!(monitors[1].display, 2);
        assert_eq!(monitors[1].manufacturer.as_deref(), Some("GSM"));
        assert_eq!(monitors[1].model.as_deref(), Some("LG Electronics"));
    }

    #[test]
    fn parses_brief_getvcp() {
        let value = parse_getvcp(VcpCode::BRIGHTNESS, "VCP 10 C 50 100\n").unwrap();
        assert_eq!(value.current, 50);
        assert_eq!(value.max, 100);
    }

    #[test]
    fn parses_long_getvcp() {
        let output = "VCP code 0x10 (Brightness): current value =    42, max value =   100\n";
        let value = parse_getvcp(VcpCode::BRIGHTNESS, output).unwrap();
        assert_eq!(value.current, 42);
        assert_eq!(value.max, 100);
    }

    #[test]
    fn getvcp_missing_value_is_error() {
        assert!(parse_getvcp(VcpCode::BRIGHTNESS, "Display 1\n").is_err());
    }

    #[test]
    fn parses_capabilities() {
        let output = "VCP Features:\n   Feature: 10\n   Feature: 12\n   Feature: 60\n";
        assert_eq!(
            parse_capabilities(output),
            vec![
                VcpCode::BRIGHTNESS,
                VcpCode::CONTRAST,
                VcpCode::INPUT_SOURCE
            ]
        );
    }
}
