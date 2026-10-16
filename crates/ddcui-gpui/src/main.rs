//! GPUI front-end for `ddcui-rs`.
//!
//! This is the Rust/GPUI replacement for the Qt UI in the original `ddcui`.
//! The window lists every DDC/CI monitor discovered by the active backend and
//! lets you nudge each monitor's brightness up or down.

use std::sync::Arc;

use gpui::{
    div, prelude::*, px, rgb, size, App, Application, Bounds, Context, MouseButton, MouseUpEvent,
    Window, WindowBounds, WindowOptions,
};

use ddc_core::mock::MockBackend;
use ddc_core::{DdcBackend, DdcutilCli, MonitorId, VcpCode};

const BG: u32 = 0x1e1e1e;
const CARD: u32 = 0x2a2a2a;
const BUTTON: u32 = 0x3a3a3a;
const BUTTON_HOVER: u32 = 0x4a4a4a;
const TRACK: u32 = 0x111111;
const ACCENT: u32 = 0x3b82f6;
const TEXT_DIM: u32 = 0x9ca3af;

/// One row in the UI: a monitor plus the brightness value we are showing.
struct MonitorRow {
    id: MonitorId,
    brightness: u16,
    max: u16,
}

/// Root application state and view.
struct AppState {
    backend: Arc<dyn DdcBackend>,
    monitors: Vec<MonitorRow>,
    status: String,
}

impl AppState {
    fn new(backend: Arc<dyn DdcBackend>) -> Self {
        let mut state = Self {
            backend,
            monitors: Vec::new(),
            status: String::new(),
        };
        state.refresh();
        state
    }

    fn refresh(&mut self) {
        self.monitors.clear();
        match self.backend.detect() {
            Ok(ids) if ids.is_empty() => {
                self.status = format!("{} backend: no DDC/CI monitors found.", self.backend.name());
            }
            Ok(ids) => {
                for id in ids {
                    let (brightness, max) = match self.backend.get_vcp(&id, VcpCode::BRIGHTNESS) {
                        Ok(value) => (value.current, value.max.max(1)),
                        Err(_) => (0, 100),
                    };
                    self.monitors.push(MonitorRow {
                        id,
                        brightness,
                        max,
                    });
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

    fn adjust(&mut self, index: usize, direction: i32) {
        let Some(row) = self.monitors.get(index) else {
            return;
        };
        let step = (row.max / 20).max(1) as i32;
        let target = (row.brightness as i32 + direction * step).clamp(0, row.max as i32) as u16;
        let id = row.id.clone();

        match self.backend.set_vcp(&id, VcpCode::BRIGHTNESS, target) {
            Ok(()) => {
                if let Some(row) = self.monitors.get_mut(index) {
                    row.brightness = target;
                }
                self.status = format!("{}: brightness {}", id.label(), target);
            }
            Err(err) => self.status = format!("Failed to set brightness: {err}"),
        }
    }
}

impl Render for AppState {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let indices: Vec<usize> = (0..self.monitors.len()).collect();

        div()
            .flex()
            .flex_col()
            .gap_3()
            .p_5()
            .size_full()
            .bg(rgb(BG))
            .text_color(gpui::white())
            .child(div().text_xl().child("ddcui-rs"))
            .child(
                div()
                    .text_size(px(13.0))
                    .text_color(rgb(TEXT_DIM))
                    .child(self.status.clone()),
            )
            .child(
                div()
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .bg(rgb(BUTTON))
                    .cursor_pointer()
                    .hover(|style| style.bg(rgb(BUTTON_HOVER)))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(
                            |this: &mut Self,
                             _: &MouseUpEvent,
                             _: &mut Window,
                             cx: &mut Context<Self>| {
                                this.refresh();
                                cx.notify();
                            },
                        ),
                    )
                    .child("Refresh"),
            )
            .children(
                indices
                    .into_iter()
                    .map(|index| self.render_monitor(index, cx)),
            )
    }
}

impl AppState {
    fn render_monitor(&self, index: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let row = &self.monitors[index];
        let percent = if row.max == 0 {
            0
        } else {
            u32::from(row.brightness) * 100 / u32::from(row.max)
        };
        let label = format!("{}  —  {}%", row.id.label(), percent);
        let fill_width = (percent as f32 / 100.0) * 240.0;

        let decrease = cx.listener(
            move |this: &mut Self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>| {
                this.adjust(index, -1);
                cx.notify();
            },
        );
        let increase = cx.listener(
            move |this: &mut Self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>| {
                this.adjust(index, 1);
                cx.notify();
            },
        );

        div()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .bg(rgb(CARD))
            .rounded_md()
            .child(div().child(label))
            .child(
                div()
                    .w(px(240.0))
                    .h(px(10.0))
                    .rounded_md()
                    .bg(rgb(TRACK))
                    .child(
                        div()
                            .h(px(10.0))
                            .w(px(fill_width))
                            .rounded_md()
                            .bg(rgb(ACCENT)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .items_center()
                    .child(control_button("-", decrease))
                    .child(control_button("+", increase)),
            )
    }
}

fn control_button(
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

fn make_backend() -> Arc<dyn DdcBackend> {
    let cli = DdcutilCli::new();
    if cli.is_available() {
        Arc::new(cli)
    } else {
        Arc::new(MockBackend::demo())
    }
}

fn main() {
    let backend = make_backend();

    Application::new().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(560.0), px(480.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
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
