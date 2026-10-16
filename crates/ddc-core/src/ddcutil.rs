//! A [`DdcBackend`] that shells out to the `ddcutil` command line tool.
//!
//! This mirrors what the original `ddcui` does through `libddcutil`, but keeps
//! the process boundary explicit so the core crate stays free of native
//! dependencies and is easy to test.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::backend::DdcBackend;
use crate::error::{DdcError, Result};
use crate::model::{MonitorId, VcpCode, VcpValue};

/// How long a single `ddcutil` call may take before it is killed.
///
/// DDC/CI is slow (a bad monitor can make `ddcutil` retry for several
/// seconds), so the limit is generous; it exists to stop a wedged call from
/// blocking a worker forever.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// How often a running command is checked for completion.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// Backend that drives the external `ddcutil` executable.
#[derive(Debug, Clone)]
pub struct DdcutilCli {
    program: String,
    timeout: Duration,
}

impl Default for DdcutilCli {
    fn default() -> Self {
        Self {
            program: "ddcutil".to_string(),
            timeout: DEFAULT_TIMEOUT,
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
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// Change how long one call may take before it is killed.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Run `ddcutil` with a time limit and return its standard output.
    fn run(&self, args: &[&str]) -> Result<String> {
        let mut child = Command::new(&self.program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| match err.kind() {
                std::io::ErrorKind::NotFound => {
                    DdcError::Unavailable(format!("'{}' was not found in PATH", self.program))
                }
                _ => DdcError::Io(err),
            })?;

        let deadline = Instant::now() + self.timeout;
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => std::thread::sleep(POLL_INTERVAL),
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(DdcError::Timeout(self.timeout));
                }
                Err(err) => return Err(DdcError::Io(err)),
            }
        }

        let output = child.wait_with_output().map_err(DdcError::Io)?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(DdcError::Command {
                status: output.status.code(),
                stderr: if stderr.is_empty() {
                    "the command printed nothing on stderr".to_string()
                } else {
                    stderr
                },
            });
        }

        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// Whether `program` exists and can be executed.
    fn is_executable(program: &str) -> bool {
        if program.contains('/') {
            return Path::new(program).is_file();
        }
        std::env::var_os("PATH").is_some_and(|paths| {
            std::env::split_paths(&paths).any(|dir| dir.join(program).is_file())
        })
    }
}

impl DdcBackend for DdcutilCli {
    fn name(&self) -> &'static str {
        "ddcutil"
    }

    fn is_available(&self) -> bool {
        Self::is_executable(&self.program) && self.run(&["--version"]).is_ok()
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
            set_value(&mut monitor.bus, value);
        } else if let Some(value) = line.strip_prefix("DRM connector:") {
            set_value(&mut monitor.connector, value);
        } else if let Some(value) = line.strip_prefix("Mfg id:") {
            apply_mfg_id(monitor, value);
        } else if let Some(value) = line.strip_prefix("Model:") {
            set_value(&mut monitor.model, value);
        } else if let Some(value) = line.strip_prefix("Serial number:") {
            set_value(&mut monitor.serial, value);
        }
    }

    monitors
}

fn apply_mfg_id(monitor: &mut MonitorId, value: &str) {
    let value = value.trim();
    match value.split_once(" - ") {
        Some((manufacturer, model)) => {
            set_value(&mut monitor.manufacturer, manufacturer);
            // The text after " - " is a human-readable name; it is only a
            // fallback for monitors that do not report a separate "Model:" line.
            fill_if_empty(&mut monitor.model, model);
        }
        None => set_value(&mut monitor.manufacturer, value),
    }
}

/// Set `slot` to `value` when `value` is non-empty. Later sources win.
fn set_value(slot: &mut Option<String>, value: &str) {
    let value = value.trim();
    if !value.is_empty() {
        *slot = Some(value.to_string());
    }
}

/// Set `slot` only if it is still empty (lower-priority source).
fn fill_if_empty(slot: &mut Option<String>, value: &str) {
    if slot.is_none() {
        set_value(slot, value);
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

    /// Write an executable stand-in for `ddcutil` and return its path.
    ///
    /// The script body is run with `/bin/sh`, so it can log its arguments and
    /// produce whatever output the test needs.
    fn fake_ddcutil(name: &str, body: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("ddcui-rs-fake-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // Write to a temporary name and rename it into place: tests run in
        // parallel, and a child process that inherits the write descriptor
        // between fork and exec makes executing the file fail with ETXTBSY.
        // After the rename the descriptor points at an unreachable inode.
        let temp = dir.join("ddcutil.tmp");
        std::fs::write(&temp, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o755)).unwrap();

        let path = dir.join("ddcutil");
        std::fs::rename(&temp, &path).unwrap();
        path
    }

    /// A log file the fake command can append to.
    fn log_path(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("ddcui-rs-fake-{name}.log"));
        let _ = std::fs::remove_file(&path);
        path
    }

    fn read_log(path: &std::path::Path) -> String {
        std::fs::read_to_string(path).unwrap_or_default()
    }

    #[test]
    fn detect_reads_monitors_from_the_command() {
        let path = fake_ddcutil("detect-ok", &format!("cat <<'EOF'\n{DETECT_OUTPUT}\nEOF"));
        let backend = DdcutilCli::with_program(path.to_string_lossy().to_string());

        let monitors = backend.detect().unwrap();
        assert_eq!(monitors.len(), 2);
        assert_eq!(monitors[0].display, 1);
        assert_eq!(monitors[1].manufacturer.as_deref(), Some("GSM"));
    }

    #[test]
    fn get_vcp_asks_for_the_right_display() {
        let log = log_path("getvcp-args");
        let body = format!(
            "echo \"$*\" >> {log}\ncat <<'EOF'\nVCP 10 C 42 100\nEOF",
            log = log.display()
        );
        let path = fake_ddcutil("getvcp-args", &body);
        let backend = DdcutilCli::with_program(path.to_string_lossy().to_string());

        let monitor = MonitorId {
            display: 2,
            bus: None,
            connector: None,
            manufacturer: None,
            model: None,
            serial: None,
        };
        let value = backend.get_vcp(&monitor, VcpCode::BRIGHTNESS).unwrap();
        assert_eq!(value.current, 42);
        assert_eq!(value.max, 100);

        let logged = read_log(&log);
        assert_eq!(logged.trim(), "getvcp 10 --display 2 --brief");
    }

    #[test]
    fn set_vcp_passes_the_value_through() {
        let log = log_path("setvcp-args");
        let body = format!("echo \"$*\" >> {log}", log = log.display());
        let path = fake_ddcutil("setvcp-args", &body);
        let backend = DdcutilCli::with_program(path.to_string_lossy().to_string());

        let monitor = MonitorId {
            display: 1,
            bus: None,
            connector: None,
            manufacturer: None,
            model: None,
            serial: None,
        };
        backend.set_vcp(&monitor, VcpCode::CONTRAST, 55).unwrap();

        let logged = read_log(&log);
        assert_eq!(logged.trim(), "setvcp 12 55 --display 1");
    }

    #[test]
    fn a_failing_command_reports_its_error_text() {
        let body = "echo 'monitor is switched off' >&2\nexit 3";
        let path = fake_ddcutil("failing", body);
        let backend = DdcutilCli::with_program(path.to_string_lossy().to_string());

        let error = backend.detect().unwrap_err();
        match error {
            DdcError::Command { status, stderr } => {
                assert_eq!(status, Some(3));
                assert!(stderr.contains("monitor is switched off"), "{stderr}");
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn a_silent_failure_still_says_something() {
        let path = fake_ddcutil("silent-failure", "exit 1");
        let backend = DdcutilCli::with_program(path.to_string_lossy().to_string());

        let error = backend.detect().unwrap_err();
        assert!(error.to_string().contains("printed nothing"), "{error}");
    }

    #[test]
    fn a_hanging_command_is_killed() {
        let path = fake_ddcutil("hanging", "sleep 30");
        let backend = DdcutilCli::with_program(path.to_string_lossy().to_string())
            .with_timeout(Duration::from_millis(250));

        let started = Instant::now();
        let error = backend.detect().unwrap_err();
        let elapsed = started.elapsed();

        assert!(matches!(error, DdcError::Timeout(_)), "{error:?}");
        assert!(elapsed < Duration::from_secs(10), "took {elapsed:?}");
    }

    #[test]
    fn a_missing_program_is_unavailable() {
        let backend = DdcutilCli::with_program("/nonexistent/ddcutil");
        assert!(!backend.is_available());

        let error = backend.detect().unwrap_err();
        assert!(matches!(error, DdcError::Unavailable(_)), "{error:?}");
    }

    #[test]
    fn a_present_program_is_available() {
        let path = fake_ddcutil("available", "exit 0");
        let backend = DdcutilCli::with_program(path.to_string_lossy().to_string());
        assert!(backend.is_available());
    }
}
