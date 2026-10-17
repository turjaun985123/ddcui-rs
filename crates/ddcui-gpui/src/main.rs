//! GPUI front-end for `ddcui-rs`.
//!
//! This is the Rust/GPUI replacement for the Qt UI in the original `ddcui`.
//! Every DDC/CI monitor gets a card with its name, where it is attached, and a
//! level bar per continuous feature (brightness, contrast) that can be clicked,
//! dragged, stepped and set from quick percentages or saved presets. Writes go
//! to a background worker, so the window never waits for the monitor.
//!
//! The same binary is a headless command line (`--list`, `--set-brightness`,
//! ...) and installs desktop integrations with `--plugin-install`. Window
//! colours come from [`theme`], and `--theme`/`DDCUI_THEME`/the desktop can
//! choose them.

mod cli;
mod layout;
mod plugins;
mod queue;
mod theme;

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    div, prelude::*, px, rgb, App, Application, Bounds, Context, FontWeight, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, TitlebarOptions, Window, WindowBounds,
    WindowDecorations, WindowOptions,
};

use ddc_core::mock::MockBackend;
use ddc_core::{unique_keys, DdcBackend, DdcError, DdcutilCli, MonitorId, VcpCode, VcpValue};
use serde::{Deserialize, Serialize};

use crate::queue::{WriteJob, WriteQueue};
use crate::theme::{Palette, Theme};

/// Application id used for window grouping and the desktop entry.
const APP_ID: &str = "io.github.jerinshop7_maker.DdcuiRs";

/// How far the `-`/`+` buttons move a feature, in percent.
const STEP: i32 = 5;

/// Features shown for every monitor, in order.
const FEATURES: [VcpCode; 2] = [VcpCode::BRIGHTNESS, VcpCode::CONTRAST];

/// Quick-set percentages offered next to each feature.
const PRESETS: [u8; 5] = [0, 25, 50, 75, 100];

/// One controllable VCP feature of a monitor.
struct FeatureRow {
    value: VcpValue,
    supported: bool,
    /// A write for this row is on its way to the monitor.
    pending: bool,
    /// The last write failed, so the value shown is the requested one, not a
    /// value the monitor confirmed.
    failed: bool,
}

/// One monitor and the features we are showing for it.
struct MonitorRow {
    id: MonitorId,
    /// A key that stays the same while the display number may not, so writes
    /// and (later) per-monitor presets follow the hardware, not the index.
    key: String,
    features: Vec<FeatureRow>,
}

/// What happened to a requested write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Requested {
    /// A write was queued for the worker.
    Queued,
    /// The monitor already reports this value, so nothing was written.
    Unchanged,
    /// The row has no writable feature.
    Unsupported,
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
    /// A standing warning about the environment, shown above the monitors.
    notice: Option<String>,
    /// Detection or reading failed, so the values below are the last known
    /// ones rather than freshly read ones.
    stale: bool,
    /// Writes waiting for the worker, at most one per monitor and feature.
    queue: WriteQueue,
    /// The write the worker is running right now.
    in_flight: Option<WriteJob>,
    /// The colours of the window.
    palette: Palette,
}

impl AppState {
    fn new(backend: Arc<dyn DdcBackend>, notice: Option<String>, palette: Palette) -> Self {
        let (profiles, problem) = load_profiles();
        let mut state = Self {
            backend,
            monitors: Vec::new(),
            profiles,
            status: String::new(),
            notice,
            stale: false,
            queue: WriteQueue::default(),
            in_flight: None,
            palette,
        };
        state.refresh();
        if let Some(problem) = problem {
            state.status = format!("{problem} · {}", state.status);
        }
        state
    }

    /// Re-detect monitors and re-read every feature.
    ///
    /// A monitor list is only replaced once a full read has succeeded, so a
    /// monitor that disappears for a moment leaves the last known values
    /// visible and clearly marked as stale instead of turning into zeros.
    fn refresh(&mut self) {
        match self.backend.detect() {
            Ok(ids) if ids.is_empty() => {
                self.monitors.clear();
                self.stale = false;
                self.status = format!(
                    "{} backend: no DDC/CI monitors found. \
                     Is a monitor connected and is DDC/CI enabled in its menu?",
                    self.backend.name()
                );
            }
            Ok(ids) => {
                let keys = unique_keys(&ids);
                let mut monitors = Vec::new();
                for (id, key) in ids.into_iter().zip(keys) {
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
                        features.push(FeatureRow {
                            value,
                            supported,
                            pending: false,
                            failed: false,
                        });
                    }
                    monitors.push(MonitorRow { id, key, features });
                }
                self.monitors = monitors;
                self.stale = false;
                self.status = format!(
                    "{} backend: {} monitor(s).",
                    self.backend.name(),
                    self.monitors.len()
                );
            }
            Err(err) => {
                self.stale = !self.monitors.is_empty();
                self.status = if self.stale {
                    format!("Detection failed ({err}); showing the last known values")
                } else {
                    format!("Detection failed: {err}")
                };
            }
        }
    }

    /// Where the monitor with this key is in the current list.
    fn monitor_index(&self, key: &str) -> Option<usize> {
        self.monitors.iter().position(|row| row.key == key)
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

    /// Move a feature by one step in `direction` (-1 or +1).
    fn step(&mut self, monitor: usize, feature: usize, direction: i32, cx: &mut Context<Self>) {
        let Some(current) = self.percent(monitor, feature) else {
            return;
        };
        let next = i32::from(current) + direction * STEP;
        let next = u8::try_from(next.clamp(0, 100)).unwrap_or(0);
        self.request_percent(monitor, feature, next, cx);
    }

    /// The current value of a supported feature, as a percentage.
    fn percent(&self, monitor: usize, feature: usize) -> Option<u8> {
        let feature = self.monitors.get(monitor)?.features.get(feature)?;
        if !feature.supported {
            return None;
        }
        Some(feature.value.percent())
    }

    /// Ask for a feature to be written, coalescing with writes already queued.
    ///
    /// The window never waits for `ddcutil`: the new value is shown right away,
    /// the write runs on the background executor, and only the newest value per
    /// monitor and feature is kept, so dragging a level bar cannot pile up a
    /// backlog of hardware writes.
    fn request_percent(
        &mut self,
        monitor: usize,
        feature: usize,
        percent: u8,
        cx: &mut Context<Self>,
    ) -> Requested {
        let Some((_, code, max)) = self.target(monitor, feature) else {
            return Requested::Unsupported;
        };
        let percent = percent.min(100);
        let target = VcpValue::from_percent(code, percent, max.max(1)).current;

        let (previous, key) = {
            let Some(row) = self.monitors.get_mut(monitor) else {
                return Requested::Unsupported;
            };
            let key = row.key.clone();
            let Some(row_feature) = row.features.get_mut(feature) else {
                return Requested::Unsupported;
            };
            if !row_feature.pending && row_feature.value.current == target {
                return Requested::Unchanged;
            }
            let previous = row_feature.value.current;
            row_feature.value.current = target;
            row_feature.pending = true;
            row_feature.failed = false;
            (previous, key)
        };

        let job = WriteJob {
            key,
            feature,
            percent,
            previous,
        };
        self.queue.push(job);

        let label = self.monitors[monitor].id.name();
        self.status = format!("{label}: {} → {percent}%", code.name());
        self.start_write(cx);
        Requested::Queued
    }

    /// Hand the next queued write to the background executor.
    ///
    /// Writes run one at a time, in the order they were asked for, so a slow
    /// monitor cannot be hit by overlapping commands. A write whose monitor has
    /// gone away in the meantime is dropped instead of landing on whatever now
    /// sits at that position in the list.
    fn start_write(&mut self, cx: &mut Context<Self>) {
        if self.in_flight.is_some() {
            return;
        }
        loop {
            let Some(job) = self.queue.pop() else {
                return;
            };
            let Some(index) = self.monitor_index(&job.key) else {
                self.status = "A monitor went away before its change was written".to_string();
                continue;
            };
            let Some((id, code, max)) = self.target(index, job.feature) else {
                continue;
            };
            let target = VcpValue::from_percent(code, job.percent, max.max(1)).current;

            self.in_flight = Some(job.clone());

            let backend = self.backend.clone();
            cx.spawn(async move |this, cx| {
                let outcome = cx
                    .background_executor()
                    .spawn(async move { backend.set_vcp(&id, code, target) })
                    .await;
                let _ = this.update(cx, |state, cx| {
                    state.finish_write(job, target, outcome, cx);
                    cx.notify();
                });
            })
            .detach();
            return;
        }
    }

    /// Record the result of a finished write and start the next one.
    fn finish_write(
        &mut self,
        job: WriteJob,
        target: u16,
        outcome: Result<(), DdcError>,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.monitor_index(&job.key) else {
            self.status = match &outcome {
                Ok(()) => format!(
                    "A monitor that is no longer listed accepted the change ({}%)",
                    job.percent
                ),
                Err(err) => {
                    format!("A monitor that is no longer listed rejected the change ({err})")
                }
            };
            self.in_flight = None;
            self.start_write(cx);
            return;
        };

        let row = &self.monitors[index];
        let label = row.id.name();
        let name = row
            .features
            .get(job.feature)
            .map(|feature| feature.value.code.name().to_string())
            .unwrap_or_else(|| "feature".to_string());

        if let Some(feature) = self.monitors[index].features.get_mut(job.feature) {
            feature.pending = false;
            match &outcome {
                Ok(()) => {
                    feature.value.current = target;
                    feature.failed = false;
                }
                Err(_) => {
                    feature.value.current = job.previous;
                    feature.failed = true;
                }
            }
        }

        self.status = match &outcome {
            Ok(()) => format!("{label}: {name} set to {}%", job.percent),
            Err(err) => format!("{label}: {name} failed: {err}"),
        };

        self.in_flight = None;
        self.start_write(cx);
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

        // Only keep the new preset once it is safely on disk, so a write
        // failure cannot leave the window and the file disagreeing.
        let mut next = self.profiles.clone();
        next.push(profile);
        match store_profiles(&next) {
            Ok(()) => {
                self.profiles = next;
                self.status = format!("Saved preset {label}");
            }
            Err(err) => self.status = format!("Could not save preset: {err}"),
        }
    }

    /// Apply a preset to every monitor that supports the features.
    fn apply_profile(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(profile) = self.profiles.get(index).copied() else {
            return;
        };
        let monitors = self.monitors.len();
        let mut queued = 0;
        let mut unchanged = 0;
        for monitor in 0..monitors {
            for (feature, code) in FEATURES.into_iter().enumerate() {
                let percent = if code == VcpCode::BRIGHTNESS {
                    profile.brightness
                } else {
                    profile.contrast
                };
                match self.request_percent(monitor, feature, percent, cx) {
                    Requested::Queued => queued += 1,
                    Requested::Unchanged => unchanged += 1,
                    Requested::Unsupported => {}
                }
            }
        }
        let label = profile.label();
        self.status = match (queued, unchanged) {
            (0, 0) => format!("Preset {label} does not apply to any monitor"),
            (0, _) => format!("Preset {label} is already in place"),
            _ => format!("Applying preset {label} ({queued} value(s))"),
        };
    }

    /// Forget a preset.
    fn delete_profile(&mut self, index: usize) {
        let Some(profile) = self.profiles.get(index).copied() else {
            return;
        };
        let label = profile.label();

        let mut next = self.profiles.clone();
        next.remove(index);
        match store_profiles(&next) {
            Ok(()) => {
                self.profiles = next;
                self.status = format!("Deleted preset {label}");
            }
            Err(err) => self.status = format!("Could not delete preset {label}: {err}"),
        }
    }
}

impl Render for AppState {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = self.palette;
        let mut monitors = Vec::new();
        for index in 0..self.monitors.len() {
            monitors.push(self.render_monitor(index, cx));
        }
        let mut subtitle = format!(
            "{} backend · {} monitor(s)",
            self.backend.name(),
            self.monitors.len()
        );
        if self.stale {
            subtitle.push_str(" · last known values");
        }

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(palette.bg))
            .text_color(rgb(palette.text))
            .child(self.render_title_bar())
            .child(self.render_toolbar(subtitle, cx))
            .child(self.render_notice())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .gap_3()
                    .p_4()
                    // Scrolling needs a stateful element, so the list gets an id.
                    .id("monitor-cards")
                    .overflow_y_scroll()
                    .scrollbar_width(px(8.0))
                    .children(monitors)
                    .child(self.render_presets(cx)),
            )
            .child(self.render_status_bar())
    }
}

impl AppState {
    /// The draggable title bar with the window buttons.
    fn render_title_bar(&self) -> impl IntoElement {
        let palette = self.palette;
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .px_4()
            .py_2()
            .bg(rgb(palette.header))
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
                            .text_size(px(11.0))
                            .text_color(rgb(palette.text_dim))
                            .child("drag here to move · click a level bar to set it"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(window_button("–", palette, minimize_window))
                    .child(window_button("✕", palette, close_window)),
            )
    }

    /// Backend summary plus the refresh control.
    fn render_toolbar(&self, subtitle: String, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = self.palette;
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
                    .text_color(rgb(palette.text_dim))
                    .child(subtitle),
            )
            .child(button("Refresh", palette, refresh))
    }

    /// A standing warning about the environment, when there is one.
    ///
    /// Without this the demo backend would look like a real monitor, which is
    /// exactly the kind of thing that ends up in a bug report as "the app shows
    /// a monitor I do not have".
    fn render_notice(&self) -> impl IntoElement {
        let palette = self.palette;
        let mut banner = div().flex().flex_row().gap_2().text_size(px(12.0));
        if let Some(text) = self.notice.as_deref() {
            banner = banner
                .px_4()
                .py_2()
                .bg(rgb(palette.notice_bg))
                .text_color(rgb(palette.notice_text))
                .child("⚠")
                .child(text.to_string());
        }
        banner
    }

    /// One monitor card: what it is, where it is attached, and its features.
    fn render_monitor(&self, index: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = self.palette;
        let monitor = &self.monitors[index];
        let title = monitor.id.name();
        let detail = monitor.id.detail();
        let stale = self.stale;

        let mut features = Vec::new();
        for feature in 0..monitor.features.len() {
            features.push(self.render_feature(index, feature, cx));
        }

        let mut header = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .text_ellipsis()
                    .text_size(px(16.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title),
            );
        if stale {
            header = header.child(
                div()
                    .flex_shrink_0()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(rgb(palette.surface_alt))
                    .text_size(px(10.0))
                    .text_color(rgb(palette.pending))
                    .child("last known"),
            );
        }
        header = header.child(
            div()
                .flex_shrink_0()
                .text_size(px(11.0))
                .text_color(rgb(palette.text_dim))
                .child(detail),
        );

        div()
            .flex()
            .flex_col()
            .gap_3()
            .w(px(layout::CARD_WIDTH))
            .p(px(layout::CARD_PADDING))
            .bg(rgb(palette.surface))
            .border_1()
            .border_color(rgb(palette.border))
            .rounded_lg()
            .child(header)
            .children(features)
    }

    fn render_feature(
        &self,
        monitor: usize,
        feature: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.palette;
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
                        .text_size(px(12.0))
                        .text_color(rgb(palette.text_dim))
                        .child(name),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(rgb(palette.text_dim))
                        .child("not reported by this monitor"),
                );
        }

        let (state_text, state_color) = if row.failed {
            ("write failed".to_string(), palette.failed)
        } else if row.pending {
            ("writing…".to_string(), palette.pending)
        } else {
            (String::new(), palette.pending)
        };

        let decrease = cx.listener(
            move |this: &mut Self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>| {
                this.step(monitor, feature, -1, cx);
                cx.notify();
            },
        );
        let increase = cx.listener(
            move |this: &mut Self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>| {
                this.step(monitor, feature, 1, cx);
                cx.notify();
            },
        );
        let click = cx.listener(move |this: &mut Self, event: &MouseDownEvent, _, cx| {
            let percent = layout::percent_at(f32::from(event.position.x));
            this.request_percent(monitor, feature, percent, cx);
            cx.notify();
        });
        let drag = cx.listener(move |this: &mut Self, event: &MouseMoveEvent, _, cx| {
            if event.pressed_button == Some(MouseButton::Left) {
                let percent = layout::percent_at(f32::from(event.position.x));
                this.request_percent(monitor, feature, percent, cx);
                cx.notify();
            }
        });

        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(rgb(palette.text_dim))
                            .child(name),
                    )
                    .child(
                        div()
                            .text_size(px(16.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(format!("{percent}%")),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(rgb(state_color))
                            .child(state_text),
                    )
                    .child(div().flex_1())
                    .child(button("−", palette, decrease))
                    .child(button("+", palette, increase)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .w(px(layout::TRACK_WIDTH))
                    .h(px(layout::TRACK_HEIGHT))
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, click)
                    .on_mouse_move(drag)
                    .child(
                        div()
                            .relative()
                            .flex()
                            .items_center()
                            .w(px(layout::TRACK_WIDTH))
                            .h(px(layout::BAR_HEIGHT))
                            .rounded_lg()
                            .bg(rgb(palette.track))
                            .child(
                                div()
                                    .h(px(layout::BAR_HEIGHT))
                                    .w(px(layout::fill_width(row.value.percent())))
                                    .rounded_lg()
                                    .bg(rgb(palette.accent)),
                            )
                            .children((0..layout::TICKS.len()).map(|index| {
                                div()
                                    .absolute()
                                    .left(px(layout::tick_offset(index)))
                                    .top(px((layout::BAR_HEIGHT - layout::TICK_HEIGHT) / 2.0))
                                    .w(px(layout::TICK_WIDTH))
                                    .h(px(layout::TICK_HEIGHT))
                                    .rounded_xs()
                                    .bg(rgb(palette.border))
                            })),
                    ),
            )
            .child(self.render_presets_row(monitor, feature, cx))
    }

    /// The quick-set percentage buttons for one feature.
    fn render_presets_row(
        &self,
        monitor: usize,
        feature: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let palette = self.palette;
        let mut row = div().flex().flex_row().items_center().gap_1();
        for preset in PRESETS {
            let listener = cx.listener(
                move |this: &mut Self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>| {
                    this.request_percent(monitor, feature, preset, cx);
                    cx.notify();
                },
            );
            row = row.child(chip(preset.to_string(), palette, listener));
        }
        row
    }

    /// Saved brightness/contrast presets, plus a button to capture the current
    /// values.
    fn render_presets(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = self.palette;
        let save = cx.listener(
            |this: &mut Self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>| {
                this.save_profile();
                cx.notify();
            },
        );

        let mut body = div().flex().flex_row().flex_wrap().items_center().gap_2();
        if self.profiles.is_empty() {
            body = body.child(
                div()
                    .text_size(px(12.0))
                    .text_color(rgb(palette.text_dim))
                    .child("No presets yet — set brightness and contrast, then Save current."),
            );
        }
        for (index, profile) in self.profiles.iter().enumerate() {
            let apply = cx.listener(
                move |this: &mut Self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>| {
                    this.apply_profile(index, cx);
                    cx.notify();
                },
            );
            let remove = cx.listener(
                move |this: &mut Self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>| {
                    this.delete_profile(index);
                    cx.notify();
                },
            );
            body = body.child(chip(profile.label(), palette, apply));
            body = body.child(button("✕", palette, remove));
        }

        div()
            .flex()
            .flex_col()
            .gap_2()
            .w(px(layout::CARD_WIDTH))
            .p(px(layout::CARD_PADDING))
            .bg(rgb(palette.surface))
            .border_1()
            .border_color(rgb(palette.border))
            .rounded_lg()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(13.0))
                            .child("Presets (brightness/contrast)"),
                    )
                    .child(button("Save current", palette, save)),
            )
            .child(body)
    }

    fn render_status_bar(&self) -> impl IntoElement {
        let palette = self.palette;
        div()
            .px_4()
            .py_2()
            .bg(rgb(palette.header))
            .text_size(px(12.0))
            .text_color(rgb(palette.text_dim))
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

/// Read the saved presets, reporting anything that had to be repaired.
fn load_profiles() -> (Vec<Profile>, Option<String>) {
    let Some(path) = profiles_path() else {
        return (Vec::new(), None);
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return (Vec::new(), None);
    };

    let parsed: Vec<Profile> = match serde_json::from_str(&text) {
        Ok(profiles) => profiles,
        Err(err) => {
            // Keep a copy for the user instead of silently overwriting a file
            // that could not be understood.
            let backup = path.with_extension("json.invalid");
            let note = if std::fs::copy(&path, &backup).is_ok() {
                format!(
                    "presets could not be read ({err}); a copy was kept at {}",
                    backup.display()
                )
            } else {
                format!("presets could not be read: {err}")
            };
            return (Vec::new(), Some(note));
        }
    };

    let (profiles, dropped) = checked_profiles(parsed);
    let note = if dropped == 0 {
        None
    } else {
        Some(format!(
            "{dropped} preset(s) with out-of-range values were ignored"
        ))
    };
    (profiles, note)
}

/// Keep only presets whose values make sense, and count the rest.
fn checked_profiles(profiles: Vec<Profile>) -> (Vec<Profile>, usize) {
    let mut kept = Vec::new();
    let mut dropped = 0;
    for profile in profiles {
        if profile.brightness <= 100 && profile.contrast <= 100 {
            kept.push(profile);
        } else {
            dropped += 1;
        }
    }
    (kept, dropped)
}

/// Write the presets to disk, replacing the file atomically.
fn store_profiles(profiles: &[Profile]) -> Result<(), String> {
    let Some(path) = profiles_path() else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        if let Err(err) = std::fs::create_dir_all(parent) {
            return Err(format!("{}: {err}", parent.display()));
        }
    }
    let text = serde_json::to_string_pretty(profiles)
        .map_err(|err| format!("encoding presets failed: {err}"))?;

    // Write beside the real file and rename it over the old one, so a failing
    // or interrupted write cannot leave a half-written preset list behind.
    let temp = path.with_extension("json.new");
    if let Err(err) = std::fs::write(&temp, text) {
        return Err(format!("writing {} failed: {err}", temp.display()));
    }
    std::fs::rename(&temp, &path)
        .map_err(|err| format!("replacing {} failed: {err}", path.display()))
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
    palette: Palette,
    on_click: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .px_3()
        .py_1()
        .rounded_md()
        .bg(rgb(palette.chip))
        .cursor_pointer()
        .hover(move |style| style.bg(rgb(palette.chip_hover)))
        .on_mouse_up(MouseButton::Left, on_click)
        .child(label)
}

/// A normal action button.
fn button(
    label: &'static str,
    palette: Palette,
    on_click: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .px_3()
        .py_1()
        .rounded_md()
        .bg(rgb(palette.chip))
        .text_size(px(12.0))
        .cursor_pointer()
        .hover(move |style| style.bg(rgb(palette.chip_hover)))
        .on_mouse_up(MouseButton::Left, on_click)
        .child(label)
}

/// A compact button used for the quick-set percentages and presets.
fn chip(
    label: String,
    palette: Palette,
    on_click: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .px_2()
        .py_1()
        .rounded_md()
        .bg(rgb(palette.chip))
        .text_size(px(12.0))
        .cursor_pointer()
        .hover(move |style| style.bg(rgb(palette.chip_hover)))
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

/// The GUI backend, plus a note when it is not talking to real hardware.
///
/// Falling back to the demo monitor keeps the window usable during
/// development, but the window has to say so: showing invented values as if
/// they came from a monitor is how people end up reporting bugs about hardware
/// they do not have.
fn make_backend() -> (Arc<dyn DdcBackend>, Option<String>) {
    let cli = DdcutilCli::new();
    if cli.is_available() {
        return (Arc::new(cli), None);
    }

    let note = "Demo mode: the 'ddcutil' command was not found, so the monitors \
                below are simulated. Install ddcutil (and make sure your user can \
                reach /dev/i2c-*) to control real hardware."
        .to_string();
    (Arc::new(MockBackend::demo()), Some(note))
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

    match command {
        cli::Command::Gui { theme } => run_gui(theme),
        command => {
            let backend = real_backend();
            std::process::exit(cli::run(backend.as_ref(), &command));
        }
    }
}

fn run_gui(theme: Option<Theme>) {
    let (backend, notice) = make_backend();
    let env_theme = std::env::var("DDCUI_THEME").ok();
    let palette = theme::resolve(theme, env_theme.as_deref());

    Application::new().run(move |cx: &mut App| {
        let bounds = Bounds::centered(
            None,
            gpui::size(px(layout::WINDOW_WIDTH), px(layout::WINDOW_HEIGHT)),
            cx,
        );
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(gpui::size(
                    px(layout::WINDOW_MIN_WIDTH),
                    px(layout::WINDOW_MIN_HEIGHT),
                )),
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
                let notice = notice.clone();
                move |_window, cx| {
                    cx.new(|_| AppState::new(backend.clone(), notice.clone(), palette))
                }
            },
        )
        .expect("failed to open window");
        cx.activate(true);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn out_of_range_presets_are_dropped() {
        let profiles = vec![
            Profile {
                brightness: 40,
                contrast: 60,
            },
            Profile {
                brightness: 200,
                contrast: 60,
            },
            Profile {
                brightness: 40,
                contrast: 255,
            },
        ];
        let (kept, dropped) = checked_profiles(profiles);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].brightness, 40);
        assert_eq!(dropped, 2);
    }

    #[test]
    fn profile_labels_are_readable() {
        let profile = Profile {
            brightness: 100,
            contrast: 70,
        };
        assert_eq!(profile.label(), "100/70");
    }
}
