//! GPUI front-end for `ddcui-rs`.
//!
//! This is the Rust/GPUI replacement for the Qt UI in the original `ddcui`.
//! The window lists every DDC/CI monitor discovered by the active backend and
//! lets you read and change its continuous VCP features (brightness, contrast)
//! with a clickable level bar, stepper buttons, quick-set percentages and saved
//! brightness/contrast presets. The same binary is a headless command line
//! (`--list`, `--set-brightness`, ...) and installs desktop integrations with
//! `--plugin-install`.

mod cli;
mod plugins;

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    div, prelude::*, px, rgb, App, Application, Bounds, Context, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, TitlebarOptions, Window, WindowBounds, WindowDecorations,
    WindowOptions,
};

use ddc_core::mock::MockBackend;
use ddc_core::{DdcBackend, DdcutilCli, MonitorId, VcpCode, VcpValue};
use serde::{Deserialize, Serialize};

const BG: u32 = 0x18181b;
const HEADER: u32 = 0x1f1f23;
const CARD: u32 = 0x27272a;
const BUTTON: u32 = 0x3f3f46;
const BUTTON_HOVER: u32 = 0x52525b;
const TRACK: u32 = 0x0f0f11;
const ACCENT: u32 = 0x3b82f6;
const TEXT: u32 = 0xf4f4f5;
const TEXT_DIM: u32 = 0xa1a1aa;

/// Application id used for window grouping and the desktop entry.
const APP_ID: &str = "io.github.jerinshop7_maker.DdcuiRs";

/// How far the `-`/`+` buttons move a feature, in percent.
const STEP: i32 = 5;

/// Space between the window edge and a card: root padding (16) + card padding (12).
const CONTENT_INSET: f32 = 28.0;

/// Width reserved for a feature's name label, in pixels.
const NAME_WIDTH: f32 = 76.0;

/// Gap between the elements of a feature row, in pixels.
const ROW_GAP: f32 = 12.0;

/// Width of a feature's level bar, in pixels.
const TRACK_WIDTH: f32 = 140.0;

/// Left edge of every level bar, relative to the window, in pixels.
const TRACK_LEFT: f32 = CONTENT_INSET + NAME_WIDTH + ROW_GAP;

/// Features shown for every monitor, in order.
const FEATURES: [VcpCode; 2] = [VcpCode::BRIGHTNESS, VcpCode::CONTRAST];

/// Quick-set percentages offered next to each feature.
const PRESETS: [u8; 5] = [0, 25, 50, 75, 100];

/// One controllable VCP feature of a monitor.
struct FeatureRow {
    value: VcpValue,
    supported: bool,
}

/// One monitor and the features we are showing for it.
struct MonitorRow {
    id: MonitorId,
    features: Vec<FeatureRow>,
}

/// A saved brightness/contrast pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct Profile {
    brightness: u8,
    contrast: u8,
}

impl Profile {
    /// The `brightness/contrast` label shown on the preset buttons.
    fn label(self) -> String {
        format!("{}/{}", self.brightness, self.contrast)
    }
}

/// Root application state and view.
struct AppState {
    backend: Arc<dyn DdcBackend>,
    monitors: Vec<MonitorRow>,
    profiles: Vec<Profile>,
    status: String,
}

impl AppState {
    fn new(backend: Arc<dyn DdcBackend>) -> Self {
        let mut state = Self {
            backend,
            monitors: Vec::new(),
            profiles: load_profiles(),
            status: String::new(),
        };
        state.refresh();
        state
    }

    /// Re-detect monitors and re-read every feature.
    fn refresh(&mut self) {
        self.monitors.clear();
        match self.backend.detect() {
            Ok(ids) if ids.is_empty() => {
                self.status = format!(
                    "{} backend: no DDC/CI monitors found. \
                     Is a monitor connected and is DDC/CI enabled in its menu?",
                    self.backend.name()
                );
            }
            Ok(ids) => {
                for id in ids {
                    let mut features = Vec::new();
                    for code in FEATURES {
                        let (value, supported) = match self.backend.get_vcp(&id, code) {
                            Ok(value) => (value, true),
                            Err(_) => (
                                VcpValue {
                                    code,
                                    current: 0,
                                    max: 100,
                                },
                                false,
                            ),
                        };
                        features.push(FeatureRow { value, supported });
                    }
                    self.monitors.push(MonitorRow { id, features });
                }
                self.status = format!(
                    "{} backend: {} monitor(s).",
                    self.backend.name(),
                    self.monitors.len()
                );
            }
            Err(err) => self.status = format!("Detection failed: {err}"),
        }
    }

    /// The identity, code, and range needed to write a feature, if supported.
    fn target(&self, monitor: usize, feature: usize) -> Option<(MonitorId, VcpCode, u16)> {
        let monitor = self.monitors.get(monitor)?;
        let feature = monitor.features.get(feature)?;
        if !feature.supported {
            return None;
        }
        Some((monitor.id.clone(), feature.value.code, feature.value.max))
    }

    /// Map a window x coordinate onto a level bar, as a percentage.
    fn percent_at(x: f32) -> u8 {
        let fraction = ((x - TRACK_LEFT) / TRACK_WIDTH).clamp(0.0, 1.0);
        (fraction * 100.0).round() as u8
    }

    /// Move a feature by one step in `direction` (-1 or +1).
    fn step(&mut self, monitor: usize, feature: usize, direction: i32) {
        let Some(current) = self.percent(monitor, feature) else {
            return;
        };
        let next = i32::from(current) + direction * STEP;
        let next = u8::try_from(next.clamp(0, 100)).unwrap_or(0);
        self.set_percent(monitor, feature, next);
    }

    /// The current value of a supported feature, as a percentage.
    fn percent(&self, monitor: usize, feature: usize) -> Option<u8> {
        let feature = self.monitors.get(monitor)?.features.get(feature)?;
        if !feature.supported {
            return None;
        }
        Some(feature.value.percent())
    }

    /// Write a feature as a percentage of its range.
    ///
    /// Returns whether the monitor was written to; unsupported features and
    /// backend failures both report `false`.
    fn set_percent(&mut self, monitor: usize, feature: usize, percent: u8) -> bool {
        let Some((id, code, max)) = self.target(monitor, feature) else {
            return false;
        };
        let percent = percent.min(100);
        let target = VcpValue::from_percent(code, percent, max.max(1)).current;

        match self.backend.set_vcp(&id, code, target) {
            Ok(()) => {
                if let Some(row) = self.monitors.get_mut(monitor) {
                    if let Some(feature) = row.features.get_mut(feature) {
                        feature.value.current = target;
                    }
                }
                self.status = format!("{}: {} set to {}%", id.label(), code.name(), percent);
                true
            }
            Err(err) => {
                self.status = format!("{}: {} failed: {err}", id.label(), code.name());
                false
            }
        }
    }

    /// The brightness and contrast the first monitor currently reports.
    fn current_pair(&self) -> Option<Profile> {
        let monitor = self.monitors.first()?;
        let value = |code| {
            monitor
                .features
                .iter()
                .find(|feature| feature.value.code == code && feature.supported)
                .map(|feature| feature.value.percent())
        };
        Some(Profile {
            brightness: value(VcpCode::BRIGHTNESS)?,
            contrast: value(VcpCode::CONTRAST)?,
        })
    }

    /// Remember the current brightness/contrast as a preset.
    fn save_profile(&mut self) {
        let Some(profile) = self.current_pair() else {
            self.status = "No monitor reports both brightness and contrast.".to_string();
            return;
        };
        let label = profile.label();
        if self.profiles.contains(&profile) {
            self.status = format!("Preset {label} is already saved");
            return;
        }
        self.profiles.push(profile);
        match self.store_profiles() {
            Ok(()) => self.status = format!("Saved preset {label}"),
            Err(err) => self.status = format!("Could not save preset: {err}"),
        }
    }

    /// Apply a preset to every monitor that supports the features.
    fn apply_profile(&mut self, index: usize) {
        let Some(profile) = self.profiles.get(index).copied() else {
            return;
        };
        let monitors = self.monitors.len();
        let mut applied = 0;
        for monitor in 0..monitors {
            for (feature, code) in FEATURES.into_iter().enumerate() {
                let percent = if code == VcpCode::BRIGHTNESS {
                    profile.brightness
                } else {
                    profile.contrast
                };
                if self.set_percent(monitor, feature, percent) {
                    applied += 1;
                }
            }
        }
        let label = profile.label();
        self.status = if applied == 0 {
            format!("Preset {label} did not apply to any monitor")
        } else {
            format!("Applied preset {label} ({applied} value(s))")
        };
    }

    /// Forget a preset.
    fn delete_profile(&mut self, index: usize) {
        if index >= self.profiles.len() {
            return;
        }
        let profile = self.profiles.remove(index);
        let label = profile.label();
        match self.store_profiles() {
            Ok(()) => self.status = format!("Deleted preset {label}"),
            Err(err) => self.status = format!("Could not update presets: {err}"),
        }
    }

    /// Write the presets to disk.
    fn store_profiles(&self) -> Result<(), String> {
        let Some(path) = profiles_path() else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            if let Err(err) = std::fs::create_dir_all(parent) {
                return Err(format!("{}: {err}", parent.display()));
            }
        }
        let text = serde_json::to_string_pretty(&self.profiles)
            .map_err(|err| format!("encoding presets failed: {err}"))?;
        if let Err(err) = std::fs::write(&path, text) {
            return Err(format!("writing {} failed: {err}", path.display()));
        }
        Ok(())
    }
}

impl Render for AppState {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut monitors = Vec::new();
        for index in 0..self.monitors.len() {
            monitors.push(self.render_monitor(index, cx));
        }
        let subtitle = format!(
            "{} backend · {} monitor(s)",
            self.backend.name(),
            self.monitors.len()
        );

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(BG))
            .text_color(rgb(TEXT))
            .child(self.render_title_bar())
            .child(self.render_toolbar(subtitle, cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .gap_3()
                    .p_4()
                    .overflow_hidden()
                    .children(monitors)
                    .child(self.render_presets(cx)),
            )
            .child(self.render_status_bar())
    }
}

impl AppState {
    /// The draggable title bar with the window buttons.
    fn render_title_bar(&self) -> impl IntoElement {
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .px_4()
            .py_2()
            .bg(rgb(HEADER))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .flex_1()
                    .on_mouse_down(MouseButton::Left, |_, window: &mut Window, _| {
                        window.start_window_move();
                    })
                    .child(div().text_lg().child("ddcui-rs"))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(rgb(TEXT_DIM))
                            .child("drag here to move the window · click a level bar to set it"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(window_button("–", minimize_window))
                    .child(window_button("✕", close_window)),
            )
    }

    /// Backend summary plus the refresh control.
    fn render_toolbar(&self, subtitle: String, cx: &mut Context<Self>) -> impl IntoElement {
        let refresh = cx.listener(
            |this: &mut Self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>| {
                this.refresh();
                cx.notify();
            },
        );

        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .px_4()
            .py_2()
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(TEXT_DIM))
                    .child(subtitle),
            )
            .child(button("Refresh", refresh))
    }

    fn render_monitor(&self, index: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let monitor = &self.monitors[index];
        let title = monitor.id.label();
        let display = monitor.id.display;
        let feature_count = monitor.features.len();

        let mut features = Vec::new();
        for feature in 0..feature_count {
            features.push(self.render_feature(index, feature, cx));
        }

        div()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .bg(rgb(CARD))
            .rounded_md()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(div().child(title))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(rgb(TEXT_DIM))
                            .child(format!("display {display}")),
                    ),
            )
            .children(features)
    }

    fn render_feature(
        &self,
        monitor: usize,
        feature: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let row = &self.monitors[monitor].features[feature];
        let name = row.value.code.name();
        let supported = row.supported;
        let percent = u32::from(row.value.percent());

        if !supported {
            return div()
                .flex()
                .flex_row()
                .items_center()
                .gap_3()
                .child(
                    div()
                        .w(px(NAME_WIDTH))
                        .text_color(rgb(TEXT_DIM))
                        .child(name),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(rgb(TEXT_DIM))
                        .child("not reported by this monitor"),
                );
        }

        let decrease = cx.listener(
            move |this: &mut Self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>| {
                this.step(monitor, feature, -1);
                cx.notify();
            },
        );
        let increase = cx.listener(
            move |this: &mut Self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>| {
                this.step(monitor, feature, 1);
                cx.notify();
            },
        );
        let click = cx.listener(move |this: &mut Self, event: &MouseDownEvent, _, cx| {
            let percent = Self::percent_at(f32::from(event.position.x));
            this.set_percent(monitor, feature, percent);
            cx.notify();
        });
        let drag = cx.listener(move |this: &mut Self, event: &MouseMoveEvent, _, cx| {
            if event.pressed_button == Some(MouseButton::Left) {
                let percent = Self::percent_at(f32::from(event.position.x));
                this.set_percent(monitor, feature, percent);
                cx.notify();
            }
        });

        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .child(
                div()
                    .w(px(NAME_WIDTH))
                    .text_color(rgb(TEXT_DIM))
                    .child(name),
            )
            .child(
                div()
                    .w(px(TRACK_WIDTH))
                    .h(px(22.0))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, click)
                    .on_mouse_move(drag)
                    .child(
                        div()
                            .w(px(TRACK_WIDTH))
                            .h(px(10.0))
                            .rounded_md()
                            .bg(rgb(TRACK))
                            .child(
                                div()
                                    .h(px(10.0))
                                    .w(px((percent as f32 / 100.0) * TRACK_WIDTH))
                                    .rounded_md()
                                    .bg(rgb(ACCENT)),
                            ),
                    ),
            )
            .child(div().w(px(40.0)).child(format!("{percent}%")))
            .child(button("−", decrease))
            .child(button("+", increase))
            .child(self.render_presets_row(monitor, feature, cx))
    }

    /// The quick-set percentage buttons for one feature.
    fn render_presets_row(
        &self,
        monitor: usize,
        feature: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mut row = div().flex().flex_row().gap_1();
        for preset in PRESETS {
            let listener = cx.listener(
                move |this: &mut Self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>| {
                    this.set_percent(monitor, feature, preset);
                    cx.notify();
                },
            );
            row = row.child(chip(preset.to_string(), listener));
        }
        row
    }

    /// Saved brightness/contrast presets, plus a button to capture the current
    /// values.
    fn render_presets(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let save = cx.listener(
            |this: &mut Self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>| {
                this.save_profile();
                cx.notify();
            },
        );

        let mut body = div().flex().flex_row().flex_wrap().gap_2();
        if self.profiles.is_empty() {
            body = body.child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(TEXT_DIM))
                    .child("No presets yet — set brightness and contrast, then Save current."),
            );
        }
        for (index, profile) in self.profiles.iter().enumerate() {
            let apply = cx.listener(
                move |this: &mut Self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>| {
                    this.apply_profile(index);
                    cx.notify();
                },
            );
            let remove = cx.listener(
                move |this: &mut Self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>| {
                    this.delete_profile(index);
                    cx.notify();
                },
            );
            body = body.child(chip(profile.label(), apply));
            body = body.child(button("✕", remove));
        }

        div()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .bg(rgb(CARD))
            .rounded_md()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(div().child("Presets (brightness/contrast)"))
                    .child(button("Save current", save)),
            )
            .child(body)
    }

    fn render_status_bar(&self) -> impl IntoElement {
        div()
            .px_4()
            .py_2()
            .bg(rgb(HEADER))
            .text_size(px(12.0))
            .text_color(rgb(TEXT_DIM))
            .child(self.status.clone())
    }
}

/// Where the presets live: `$XDG_CONFIG_HOME/ddcui-rs/profiles.json`.
fn profiles_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(base.join("ddcui-rs").join("profiles.json"))
}

/// Read the saved presets, treating a missing or unreadable file as "none".
fn load_profiles() -> Vec<Profile> {
    let Some(path) = profiles_path() else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

/// Minimize the window at the platform level.
fn minimize_window(_: &MouseUpEvent, window: &mut Window, _: &mut App) {
    window.minimize_window();
}

/// Close the window.
fn close_window(_: &MouseUpEvent, window: &mut Window, _: &mut App) {
    window.remove_window();
}

/// A minimize/close button that drives the platform window directly.
fn window_button(
    label: &'static str,
    on_click: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .px_3()
        .py_1()
        .rounded_md()
        .bg(rgb(BUTTON))
        .cursor_pointer()
        .hover(|style| style.bg(rgb(BUTTON_HOVER)))
        .on_mouse_up(MouseButton::Left, on_click)
        .child(label)
}

/// A normal action button.
fn button(
    label: &'static str,
    on_click: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .px_3()
        .py_1()
        .rounded_md()
        .bg(rgb(BUTTON))
        .cursor_pointer()
        .hover(|style| style.bg(rgb(BUTTON_HOVER)))
        .on_mouse_up(MouseButton::Left, on_click)
        .child(label)
}

/// A compact button used for the quick-set percentages and presets.
fn chip(
    label: String,
    on_click: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .px_2()
        .py_1()
        .rounded_md()
        .bg(rgb(BUTTON))
        .text_size(px(12.0))
        .cursor_pointer()
        .hover(|style| style.bg(rgb(BUTTON_HOVER)))
        .on_mouse_up(MouseButton::Left, on_click)
        .child(label)
}

/// The real backend, without the demo fallback.
///
/// Command-line use must report honest errors, so it never silently talks to the
/// mock monitors.
fn real_backend() -> Arc<dyn DdcBackend> {
    Arc::new(DdcutilCli::new())
}

/// The GUI backend, which falls back to the demo monitor when `ddcutil` is
/// missing so the window is still usable during development.
fn make_backend() -> Arc<dyn DdcBackend> {
    let cli = DdcutilCli::new();
    if cli.is_available() {
        Arc::new(cli)
    } else {
        Arc::new(MockBackend::demo())
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = match cli::parse(&args) {
        Ok(command) => command,
        Err(message) => {
            eprintln!("ddcui-rs: {message}");
            eprintln!("Try 'ddcui-rs --help' for more information.");
            std::process::exit(2);
        }
    };

    if command != cli::Command::Gui {
        let backend = real_backend();
        std::process::exit(cli::run(backend.as_ref(), &command));
    }

    run_gui();
}

fn run_gui() {
    let backend = make_backend();

    Application::new().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, gpui::size(px(820.0), px(620.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(gpui::size(px(700.0), px(420.0))),
                titlebar: Some(TitlebarOptions {
                    title: Some("ddcui-rs".into()),
                    ..Default::default()
                }),
                // Without decorations Wayland gives the window no titlebar at
                // all, so moving, minimising and closing would be impossible.
                window_decorations: Some(WindowDecorations::Server),
                app_id: Some(APP_ID.to_string()),
                is_movable: true,
                is_resizable: true,
                is_minimizable: true,
                ..Default::default()
            },
            {
                let backend = backend.clone();
                move |_window, cx| cx.new(|_| AppState::new(backend.clone()))
            },
        )
        .expect("failed to open window");
        cx.activate(true);
    });
}
