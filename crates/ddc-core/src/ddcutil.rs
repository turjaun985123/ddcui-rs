//! A [`DdcBackend`] that shells out to the `ddcutil` command line tool.
//!
//! This mirrors what the original `ddcui` does through `libddcutil`, but keeps
//! the process boundary explicit so the core crate stays free of native
//! dependencies and is easy to test.

use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Stdio};
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
/// Maximum stdout or stderr retained from one backend invocation.
const MAX_OUTPUT_BYTES: usize = 1024 * 1024;

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
        let mut command = Command::new(&self.program);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // Put the command in its own process group so a timeout also
            // terminates helper processes that inherited our output pipes.
            command.process_group(0);
        }
        let mut child = command
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

        // Drain both pipes while the process runs. Waiting before reading can
        // deadlock when ddcutil fills either OS pipe buffer with diagnostics.
        let mut stdout = child.stdout.take().expect("stdout was piped");
        let mut stderr = child.stderr.take().expect("stderr was piped");
        let stdout_reader = std::thread::spawn(move || read_bounded(&mut stdout));
        let stderr_reader = std::thread::spawn(move || read_bounded(&mut stderr));

        let deadline = Instant::now() + self.timeout;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() < deadline => std::thread::sleep(POLL_INTERVAL),
                Ok(None) => {
                    kill_child_tree(&mut child);
                    let _ = child.wait();
                    let _ = stdout_reader.join();
                    let _ = stderr_reader.join();
                    return Err(DdcError::Timeout(self.timeout));
                }
                Err(err) => {
                    kill_child_tree(&mut child);
                    let _ = child.wait();
                    let _ = stdout_reader.join();
                    let _ = stderr_reader.join();
                    return Err(DdcError::Io(err));
                }
            }
        };

        let (stdout, stdout_truncated) = stdout_reader
            .join()
            .map_err(|_| DdcError::Parse("stdout reader thread panicked".to_string()))?
            .map_err(DdcError::Io)?;
        let (stderr, stderr_truncated) = stderr_reader
            .join()
            .map_err(|_| DdcError::Parse("stderr reader thread panicked".to_string()))?
            .map_err(DdcError::Io)?;
        if !status.success() {
            let mut stderr = String::from_utf8_lossy(&stderr).trim().to_string();
            if stderr_truncated {
                stderr.push_str(" [output truncated]");
            }
            return Err(DdcError::Command {
                status: status.code(),
                stderr: if stderr.is_empty() {
                    "the command printed nothing on stderr".to_string()
                } else {
                    stderr
                },
            });
        }
        if stdout_truncated {
            return Err(DdcError::Parse(format!(
                "ddcutil stdout exceeded {MAX_OUTPUT_BYTES} bytes"
            )));
        }

        Ok(String::from_utf8_lossy(&stdout).into_owned())
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

/// Kill the command and any helper processes that share its process group.
#[cfg(unix)]
fn kill_child_tree(child: &mut Child) {
    const SIGKILL: i32 = 9;
    unsafe extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
    }

    // The child is the process-group leader because `process_group(0)` was set
    // before spawn. A negative pid targets the whole group.
    let process_group = -(child.id() as i32);
    // SAFETY: kill accepts a negative process-group id and does not access
    // memory through either argument.
    let _ = unsafe { kill(process_group, SIGKILL) };
    let _ = child.kill();
}

/// On non-Unix platforms, stop the direct child process.
#[cfg(not(unix))]
fn kill_child_tree(child: &mut Child) {
    let _ = child.kill();
}

/// Drain a child pipe while retaining only a bounded amount of output.
fn read_bounded(reader: &mut impl Read) -> std::io::Result<(Vec<u8>, bool)> {
    let mut output = Vec::with_capacity(8192);
    let mut buffer = [0u8; 8192];
    let mut truncated = false;
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        let remaining = MAX_OUTPUT_BYTES.saturating_sub(output.len());
        let keep = remaining.min(count);
        output.extend_from_slice(&buffer[..keep]);
        truncated |= keep < count;
    }
    Ok((output, truncated))
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

    /// Serializes the tests that create a fake command and run it.
    ///
    /// Executing a file that some other thread has open for writing fails with
    /// `ETXTBSY`, and every one of these tests writes a script before spawning
    /// it, so they take this lock for their whole body: that way no test forks
    /// while another test still has a script open for writing.
    fn spawn_guard() -> std::sync::MutexGuard<'static, ()> {
        use std::sync::{Mutex, OnceLock};

        static GUARD: OnceLock<Mutex<()>> = OnceLock::new();
        let guard = GUARD.get_or_init(|| Mutex::new(()));
        // A test that panicked while holding the lock must not stop the rest.
        guard.lock().unwrap_or_else(|err| err.into_inner())
    }

    /// Write an executable stand-in for `ddcutil` and return its path.
    ///
    /// The script body is run with `/bin/sh`, so it can log its arguments and
    /// produce whatever output the test needs. Callers hold [`spawn_guard`]
    /// while they use it.
    fn fake_ddcutil(name: &str, body: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("ddcui-rs-fake-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let path = dir.join("ddcutil");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
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
        let _guard = spawn_guard();
        let path = fake_ddcutil("detect-ok", &format!("cat <<'EOF'\n{DETECT_OUTPUT}\nEOF"));
        let backend = DdcutilCli::with_program(path.to_string_lossy().to_string());

        let monitors = backend.detect().unwrap();
        assert_eq!(monitors.len(), 2);
        assert_eq!(monitors[0].display, 1);
        assert_eq!(monitors[1].manufacturer.as_deref(), Some("GSM"));
    }

    #[test]
    fn get_vcp_asks_for_the_right_display() {
        let _guard = spawn_guard();
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
        let _guard = spawn_guard();
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
        let _guard = spawn_guard();
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
        let _guard = spawn_guard();
        let path = fake_ddcutil("silent-failure", "exit 1");
        let backend = DdcutilCli::with_program(path.to_string_lossy().to_string());

        let error = backend.detect().unwrap_err();
        assert!(error.to_string().contains("printed nothing"), "{error}");
    }

    #[test]
    fn a_hanging_command_is_killed() {
        let _guard = spawn_guard();
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
        // Still takes the guard: a failed spawn forks too, and the child
        // inherits any descriptor another test has open for writing.
        let _guard = spawn_guard();
        let backend = DdcutilCli::with_program("/nonexistent/ddcutil");
        assert!(!backend.is_available());

        let error = backend.detect().unwrap_err();
        assert!(matches!(error, DdcError::Unavailable(_)), "{error:?}");
    }

    #[test]
    fn a_present_program_is_available() {
        let _guard = spawn_guard();
        let path = fake_ddcutil("available", "exit 0");
        let backend = DdcutilCli::with_program(path.to_string_lossy().to_string());
        // Executing a file that some process still has open for writing fails
        // with ETXTBSY for a moment; the guard above prevents that here, and
        // these retries keep a stray fork elsewhere from turning into a flake.
        let mut error = None;
        for _ in 0..20 {
            if backend.is_available() {
                return;
            }
            error = backend.run(&["--version"]).err();
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!("{} should be usable: {error:?}", path.display());
    }
}
