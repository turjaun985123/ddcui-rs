//! Headless command-line entry points.
//!
//! The GPUI window is only opened when no command is given. The flags here back
//! the desktop "Quick Actions" (`packaging/*.desktop`), so brightness can be
//! changed from the launcher's right-click menu without opening a window, and
//! they make the binary scriptable from a terminal or a shell script.
//!
//! The `--plugin-*` flags manage the desktop integrations described in
//! [`crate::plugins`], such as the GNOME Quick Settings button.

use ddc_core::{DdcBackend, MonitorId, VcpCode, VcpValue};

use crate::plugins;
use crate::theme::Theme;

/// What the process was asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Open the GUI (the default when no arguments are given).
    Gui {
        /// Window colours, when `--theme` was given.
        theme: Option<Theme>,
    },
    /// Print usage information.
    Help,
    /// Print the version.
    Version,
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
    /// List the plugins that are available.
    PluginList,
    /// Install a plugin by id.
    PluginInstall(String),
    /// Remove an installed plugin by id.
    PluginUninstall(String),
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
    --plugin-list             List the available plugins
    --plugin-install <id>     Install a plugin (for example gnome-quick-menu)
    --plugin-uninstall <id>   Remove an installed plugin
    -h, --help                Print this help
    -V, --version             Print the version

OPTIONS:
    --display <N>             Only act on monitor N (1-based, see --list)
    --theme <T>               Window colours: system, light or dark
                              (system follows the desktop, the default)
";

/// Parse command-line arguments (everything after the program name).
pub fn parse(args: &[String]) -> Result<Command, String> {
    let mut command: Option<Command> = None;
    let mut display: Option<u32> = None;
    let mut theme: Option<Theme> = None;
    let mut index = 0;

    while index < args.len() {
        let arg = args[index].clone();
        index += 1;

        match arg.as_str() {
            "-h" | "--help" => return Ok(Command::Help),
            "-V" | "--version" => return Ok(Command::Version),
            "--theme" => {
                if theme.is_some() {
                    return Err("only one --theme may be given".to_string());
                }
                let text = take_value(args, &mut index, "--theme")?;
                theme =
                    Some(Theme::parse(text).ok_or_else(|| {
                        format!("unknown theme: {text} (use system, light or dark)")
                    })?);
            }
            "--list" => choose(&mut command, Command::List)?,
            "--display" => {
                if display.is_some() {
                    return Err("only one --display may be given".to_string());
                }
                let text = take_value(args, &mut index, "--display")?;
                display = Some(parse_display(text)?);
            }
            "--get-brightness" => choose(&mut command, get_command(VcpCode::BRIGHTNESS))?,
            "--get-contrast" => choose(&mut command, get_command(VcpCode::CONTRAST))?,
            "--set-brightness" => {
                let text = take_value(args, &mut index, "--set-brightness")?;
                let percent = parse_percent(text)?;
                choose(&mut command, set_command(VcpCode::BRIGHTNESS, percent))?;
            }
            "--set-contrast" => {
                let text = take_value(args, &mut index, "--set-contrast")?;
                let percent = parse_percent(text)?;
                choose(&mut command, set_command(VcpCode::CONTRAST, percent))?;
            }
            "--plugin-list" => choose(&mut command, Command::PluginList)?,
            "--plugin-install" => {
                let text = take_value(args, &mut index, "--plugin-install")?;
                choose(&mut command, Command::PluginInstall(text.to_string()))?;
            }
            "--plugin-uninstall" => {
                let text = take_value(args, &mut index, "--plugin-uninstall")?;
                choose(&mut command, Command::PluginUninstall(text.to_string()))?;
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }

    let Some(mut command) = command else {
        return Ok(Command::Gui { theme });
    };
    if theme.is_some() {
        return Err(
            "--theme only applies to the window; it cannot be combined with a command".to_string(),
        );
    }
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

/// Remember the chosen command, refusing to guess between two of them.
///
/// Letting the last flag win is how `--set-brightness 50 --plugin-list` turns
/// into "why did nothing happen?".
fn choose(slot: &mut Option<Command>, command: Command) -> Result<(), String> {
    if slot.is_some() {
        return Err("only one command may be given (see --help)".to_string());
    }
    *slot = Some(command);
    Ok(())
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

/// Parse a 1-based monitor number.
fn parse_display(value: &str) -> Result<u32, String> {
    let number: u32 = value
        .parse()
        .map_err(|_| format!("invalid monitor number: {value}"))?;
    if number == 0 {
        return Err("invalid monitor number: monitor numbers start at 1".to_string());
    }
    Ok(number)
}

/// Parse a percentage, rejecting values outside `0..=100`.
fn parse_percent(value: &str) -> Result<u8, String> {
    let percent: u8 = value
        .parse()
        .map_err(|_| format!("invalid percentage: {value}"))?;
    if percent > 100 {
        return Err(format!("percentage out of range (0-100): {value}"));
    }
    Ok(percent)
}

/// The line printed by `--version`.
pub fn version_line() -> String {
    format!("ddcui-rs {}", env!("CARGO_PKG_VERSION"))
}

/// Run a non-GUI command, returning the process exit code.
pub fn run(backend: &dyn DdcBackend, command: &Command) -> i32 {
    match command {
        Command::Help | Command::Gui { .. } => {
            print!("{HELP}");
            0
        }
        Command::Version => {
            println!("{}", version_line());
            0
        }
        Command::List => run_list(backend),
        Command::Get { feature, display } => run_get(backend, *feature, *display),
        Command::Set {
            feature,
            percent,
            display,
        } => run_set(backend, *feature, *percent, *display),
        Command::PluginList => plugins::run_list(),
        Command::PluginInstall(id) => plugins::run_install(id),
        Command::PluginUninstall(id) => plugins::run_uninstall(id),
    }
}

/// Resolve the monitors a command applies to.
fn select(backend: &dyn DdcBackend, display: Option<u32>) -> Result<Vec<MonitorId>, String> {
    let monitors = backend
        .detect()
        .map_err(|err| format!("detection failed: {err}"))?;
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
        println!(
            "  display {}  {label}  brightness {brightness}",
            monitor.display
        );
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
        assert_eq!(parse(&args(&[])).unwrap(), Command::Gui { theme: None });
    }

    #[test]
    fn the_theme_is_a_window_option() {
        assert_eq!(
            parse(&args(&["--theme", "dark"])).unwrap(),
            Command::Gui {
                theme: Some(Theme::Dark)
            }
        );
        assert_eq!(
            parse(&args(&["--theme", "Light"])).unwrap(),
            Command::Gui {
                theme: Some(Theme::Light)
            }
        );
        assert_eq!(
            parse(&args(&["--theme", "system"])).unwrap(),
            Command::Gui {
                theme: Some(Theme::System)
            }
        );
        // It is not a way to change what another command does.
        assert!(parse(&args(&["--theme", "dark", "--list"])).is_err());
    }

    #[test]
    fn odd_theme_values_are_rejected() {
        assert!(parse(&args(&["--theme"])).is_err());
        assert!(parse(&args(&["--theme", "sepia"])).is_err());
        assert!(parse(&args(&["--theme", "dark", "--theme", "light"])).is_err());
    }

    #[test]
    fn help_is_recognised() {
        assert_eq!(parse(&args(&["-h"])).unwrap(), Command::Help);
        assert_eq!(parse(&args(&["--help"])).unwrap(), Command::Help);
    }

    #[test]
    fn version_is_recognised() {
        assert_eq!(parse(&args(&["-V"])).unwrap(), Command::Version);
        assert_eq!(parse(&args(&["--version"])).unwrap(), Command::Version);
        // The printed line names the program and a version number.
        let line = version_line();
        assert!(line.starts_with("ddcui-rs "), "{line}");
        let number = line.split(' ').nth(1).unwrap_or_default();
        assert!(number.contains('.'), "{line}");
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
    fn display_numbers_start_at_one() {
        assert!(parse(&args(&["--display", "0", "--get-brightness"])).is_err());
        assert!(parse(&args(&["--display", "-1", "--get-brightness"])).is_err());
        assert!(parse(&args(&["--display", "abc", "--get-brightness"])).is_err());
        assert!(parse(&args(&["--display", "", "--get-brightness"])).is_err());

        // Out of range for u32, i.e. a number no monitor can have.
        let too_big = (u64::from(u32::MAX) + 1).to_string();
        assert!(parse(&args(&["--display", &too_big, "--get-brightness"])).is_err());

        assert_eq!(
            parse(&args(&["--display", "1", "--get-brightness"])).unwrap(),
            Command::Get {
                feature: VcpCode::BRIGHTNESS,
                display: Some(1),
            }
        );
    }

    #[test]
    fn one_command_at_a_time() {
        let two_sets = args(&["--set-brightness", "50", "--set-contrast", "60"]);
        assert!(parse(&two_sets).is_err());

        let set_and_plugin = args(&["--set-brightness", "50", "--plugin-list"]);
        assert!(parse(&set_and_plugin).is_err());

        let repeated = args(&["--set-brightness", "50", "--set-brightness", "60"]);
        assert!(parse(&repeated).is_err());

        let list_after_get = args(&["--list", "--get-brightness"]);
        assert!(parse(&list_after_get).is_err());
    }

    #[test]
    fn one_display_option_at_a_time() {
        let repeated = args(&["--display", "1", "--display", "2", "--get-brightness"]);
        assert!(parse(&repeated).is_err());
    }

    #[test]
    fn missing_values_are_rejected() {
        assert!(parse(&args(&["--set-brightness"])).is_err());
        assert!(parse(&args(&["--set-contrast"])).is_err());
        assert!(parse(&args(&["--display"])).is_err());
    }

    #[test]
    fn rejects_unknown_argument() {
        assert!(parse(&args(&["--nope"])).is_err());
    }

    #[test]
    fn parses_plugin_commands() {
        let list = parse(&args(&["--plugin-list"])).unwrap();
        assert_eq!(list, Command::PluginList);

        let install = parse(&args(&["--plugin-install", "gnome-quick-menu"])).unwrap();
        let expected = Command::PluginInstall("gnome-quick-menu".to_string());
        assert_eq!(install, expected);

        let remove = parse(&args(&["--plugin-uninstall", "gnome-quick-menu"])).unwrap();
        let expected = Command::PluginUninstall("gnome-quick-menu".to_string());
        assert_eq!(remove, expected);
    }

    #[test]
    fn plugin_flags_need_a_value() {
        assert!(parse(&args(&["--plugin-install"])).is_err());
        assert!(parse(&args(&["--plugin-uninstall"])).is_err());
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

    /// A backend that fails the test if it is asked anything at all.
    struct MustNotBeAsked;

    impl DdcBackend for MustNotBeAsked {
        fn name(&self) -> &'static str {
            "must-not-be-asked"
        }

        fn is_available(&self) -> bool {
            panic!("--version asked the backend whether it is available")
        }

        fn detect(&self) -> ddc_core::Result<Vec<ddc_core::MonitorId>> {
            panic!("--version asked the backend to detect monitors")
        }

        fn capabilities(
            &self,
            _monitor: &ddc_core::MonitorId,
        ) -> ddc_core::Result<Vec<ddc_core::VcpCode>> {
            panic!("--version asked the backend for capabilities")
        }

        fn get_vcp(
            &self,
            _monitor: &ddc_core::MonitorId,
            _code: ddc_core::VcpCode,
        ) -> ddc_core::Result<ddc_core::VcpValue> {
            panic!("--version read a feature")
        }

        fn set_vcp(
            &self,
            _monitor: &ddc_core::MonitorId,
            _code: ddc_core::VcpCode,
            _value: u16,
        ) -> ddc_core::Result<()> {
            panic!("--version wrote a feature")
        }
    }

    #[test]
    fn version_names_the_program_without_asking_the_monitors() {
        assert_eq!(run(&MustNotBeAsked, &Command::Version), 0);
    }
}
