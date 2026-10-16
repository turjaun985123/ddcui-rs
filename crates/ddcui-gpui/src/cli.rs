//! Headless command-line entry points.
//!
//! The GPUI window is only opened when no command is given. The flags here back
//! the desktop "Quick Actions" (`packaging/*.desktop`), so brightness can be
//! changed from the launcher's right-click menu without opening a window, and
//! they make the binary scriptable from a terminal or a shell script.

use ddc_core::{DdcBackend, MonitorId, VcpCode, VcpValue};

/// What the process was asked to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// Open the GUI (the default when no arguments are given).
    Gui,
    /// Print usage information.
    Help,
    /// List the detected monitors.
    List,
    /// Read a feature from every selected monitor.
    Get {
        /// Feature to read.
        feature: VcpCode,
        /// Restrict to a single 1-based display number.
        display: Option<u32>,
    },
    /// Write a percentage to a feature on every selected monitor.
    Set {
        /// Feature to write.
        feature: VcpCode,
        /// Target value as a percentage (`0..=100`).
        percent: u8,
        /// Restrict to a single 1-based display number.
        display: Option<u32>,
    },
}

/// Usage text shown by `--help`.
pub const HELP: &str = "\
ddcui-rs - control external monitors over DDC/CI

USAGE:
    ddcui-rs [OPTIONS] [COMMAND]

With no command the graphical window is opened.

COMMANDS:
    --list                    List detected monitors
    --get-brightness          Print the brightness of each monitor
    --set-brightness <0-100>  Set brightness as a percentage
    --get-contrast            Print the contrast of each monitor
    --set-contrast <0-100>    Set contrast as a percentage
    -h, --help                Print this help

OPTIONS:
    --display <N>             Only act on monitor N (1-based, see --list)
";

/// Parse command-line arguments (everything after the program name).
pub fn parse(args: &[String]) -> Result<Command, String> {
    let mut command: Option<Command> = None;
    let mut display: Option<u32> = None;
    let mut index = 0;

    while index < args.len() {
        let arg = args[index].clone();
        index += 1;

        match arg.as_str() {
            "-h" | "--help" => return Ok(Command::Help),
            "--list" => command = Some(Command::List),
            "--display" => {
                let text = take_value(args, &mut index, "--display")?;
                display = Some(parse_display(text)?);
            }
            "--get-brightness" => command = Some(get_command(VcpCode::BRIGHTNESS)),
            "--get-contrast" => command = Some(get_command(VcpCode::CONTRAST)),
            "--set-brightness" => {
                let text = take_value(args, &mut index, "--set-brightness")?;
                let percent = parse_percent(text)?;
                command = Some(set_command(VcpCode::BRIGHTNESS, percent));
            }
            "--set-contrast" => {
                let text = take_value(args, &mut index, "--set-contrast")?;
                let percent = parse_percent(text)?;
                command = Some(set_command(VcpCode::CONTRAST, percent));
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }

    let Some(mut command) = command else {
        return Ok(Command::Gui);
    };
    if let Command::Get { display: slot, .. } | Command::Set { display: slot, .. } = &mut command {
        *slot = display;
    }
    Ok(command)
}

/// Build a read command for `feature`.
fn get_command(feature: VcpCode) -> Command {
    Command::Get {
        feature,
        display: None,
    }
}

/// Build a write command for `feature`; `--display` is applied by the caller.
fn set_command(feature: VcpCode, percent: u8) -> Command {
    Command::Set {
        feature,
        percent,
        display: None,
    }
}

/// Consume the value that follows a flag.
fn take_value<'a>(args: &'a [String], index: &mut usize, flag: &str) -> Result<&'a str, String> {
    let value = args.get(*index);
    *index += 1;
    match value {
        Some(value) => Ok(value.as_str()),
        None => Err(format!("{flag} needs a value")),
    }
}

fn parse_display(value: &str) -> Result<u32, String> {
    value.parse().map_err(|_| format!("invalid monitor number: {value}"))
}

/// Parse a percentage, rejecting values outside `0..=100`.
fn parse_percent(value: &str) -> Result<u8, String> {
    let percent: u8 = value.parse().map_err(|_| format!("invalid percentage: {value}"))?;
    if percent > 100 {
        return Err(format!("percentage out of range (0-100): {value}"));
    }
    Ok(percent)
}

/// Run a non-GUI command, returning the process exit code.
pub fn run(backend: &dyn DdcBackend, command: &Command) -> i32 {
    match *command {
        Command::Help | Command::Gui => {
            print!("{HELP}");
            0
        }
        Command::List => run_list(backend),
        Command::Get { feature, display } => run_get(backend, feature, display),
        Command::Set { feature, percent, display } => run_set(backend, feature, percent, display),
    }
}

/// Resolve the monitors a command applies to.
fn select(backend: &dyn DdcBackend, display: Option<u32>) -> Result<Vec<MonitorId>, String> {
    let monitors = backend.detect().map_err(|err| format!("detection failed: {err}"))?;
    if monitors.is_empty() {
        return Err(format!("{}: no DDC/CI monitors found", backend.name()));
    }
    let Some(number) = display else {
        return Ok(monitors);
    };
    monitors
        .into_iter()
        .find(|monitor| monitor.display == number)
        .map(|monitor| vec![monitor])
        .ok_or_else(|| format!("monitor {number} not found"))
}

/// A one-word description of a monitor's brightness support.
fn brightness_label(backend: &dyn DdcBackend, monitor: &MonitorId) -> String {
    match backend.get_vcp(monitor, VcpCode::BRIGHTNESS) {
        Ok(value) if value.max > 0 => format!("{}%", value.percent()),
        Ok(_) => "max 0".to_string(),
        Err(_) => "unsupported".to_string(),
    }
}

fn run_list(backend: &dyn DdcBackend) -> i32 {
    let monitors = match select(backend, None) {
        Ok(monitors) => monitors,
        Err(message) => {
            eprintln!("ddcui-rs: {message}");
            return 1;
        }
    };

    println!("{}: {} monitor(s)", backend.name(), monitors.len());
    for monitor in &monitors {
        let brightness = brightness_label(backend, monitor);
        let label = monitor.label();
        println!("  display {}  {label}  brightness {brightness}", monitor.display);
    }
    0
}

fn run_get(backend: &dyn DdcBackend, feature: VcpCode, display: Option<u32>) -> i32 {
    let monitors = match select(backend, display) {
        Ok(monitors) => monitors,
        Err(message) => {
            eprintln!("ddcui-rs: {message}");
            return 1;
        }
    };

    let mut failed = false;
    for monitor in &monitors {
        let label = monitor.label();
        let name = feature.name();
        match backend.get_vcp(monitor, feature) {
            Ok(value) => {
                let percent = value.percent();
                let (current, max) = (value.current, value.max);
                println!("{label}: {name} {percent}% ({current}/{max})");
            }
            Err(err) => {
                eprintln!("{label}: {name} failed: {err}");
                failed = true;
            }
        }
    }
    i32::from(failed)
}

fn run_set(backend: &dyn DdcBackend, feature: VcpCode, percent: u8, display: Option<u32>) -> i32 {
    let monitors = match select(backend, display) {
        Ok(monitors) => monitors,
        Err(message) => {
            eprintln!("ddcui-rs: {message}");
            return 1;
        }
    };

    let mut failed = false;
    for monitor in &monitors {
        let label = monitor.label();
        let name = feature.name();
        // Use the monitor's reported range when it has one, otherwise assume
        // 0..100 (some monitors do not answer a read but accept a write).
        let max = match backend.get_vcp(monitor, feature) {
            Ok(value) if value.max > 0 => value.max,
            _ => 100,
        };
        let target = VcpValue::from_percent(feature, percent, max).current;

        match backend.set_vcp(monitor, feature, target) {
            Ok(()) => println!("{label}: {name} -> {percent}% ({target})"),
            Err(err) => {
                eprintln!("{label}: {name} failed: {err}");
                failed = true;
            }
        }
    }
    i32::from(failed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ddc_core::mock::MockBackend;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|value| (*value).to_string()).collect()
    }

    fn set_brightness(percent: u8, display: Option<u32>) -> Command {
        Command::Set {
            feature: VcpCode::BRIGHTNESS,
            percent,
            display,
        }
    }

    #[test]
    fn no_arguments_open_the_gui() {
        assert_eq!(parse(&args(&[])).unwrap(), Command::Gui);
    }

    #[test]
    fn help_is_recognised() {
        assert_eq!(parse(&args(&["-h"])).unwrap(), Command::Help);
        assert_eq!(parse(&args(&["--help"])).unwrap(), Command::Help);
    }

    #[test]
    fn parses_set_brightness() {
        let parsed = parse(&args(&["--set-brightness", "50"])).unwrap();
        assert_eq!(parsed, set_brightness(50, None));
    }

    #[test]
    fn display_applies_regardless_of_position() {
        let first = args(&["--display", "2", "--set-brightness", "50"]);
        let second = args(&["--set-brightness", "50", "--display", "2"]);
        assert_eq!(parse(&first).unwrap(), set_brightness(50, Some(2)));
        assert_eq!(parse(&second).unwrap(), set_brightness(50, Some(2)));
    }

    #[test]
    fn rejects_out_of_range_percent() {
        assert!(parse(&args(&["--set-brightness", "150"])).is_err());
        assert!(parse(&args(&["--set-brightness", "300"])).is_err());
        assert!(parse(&args(&["--set-brightness", "abc"])).is_err());
    }

    #[test]
    fn rejects_unknown_argument() {
        assert!(parse(&args(&["--nope"])).is_err());
    }

    #[test]
    fn set_brightness_succeeds_on_mock_backend() {
        let backend = MockBackend::demo();
        let command = set_brightness(25, None);
        assert_eq!(run(&backend, &command), 0);
    }

    #[test]
    fn set_on_missing_display_fails() {
        let backend = MockBackend::demo();
        let command = set_brightness(25, Some(9));
        assert_eq!(run(&backend, &command), 1);
    }
}
