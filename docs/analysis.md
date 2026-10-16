# Analysis of upstream `ddcui`

Source: <https://github.com/rockowitz/ddcui> (C++ / Qt, GPL-2.0).

## What it is

`ddcui` is the reference GUI for [ddcutil](https://github.com/rockowitz/ddcutil).
It discovers monitors that speak **DDC/CI** and exposes their **MCCS VCP
features** (brightness, contrast, input source, colour gains, ...) as UI
controls. It does not talk to the monitor directly — it links `libddcutil`, which
in turn drives the monitor over the I2C bus of the graphics adapter.

## Source layout

| Directory | Responsibility |
| --- | --- |
| `src/nongui` | DDC access and feature model, deliberately free of Qt so it can be tested headlessly. |
| `src/base` | Small cross-cutting helpers (formatting, parsing, settings). |
| `src/monitor_desc` | The monitor list shown in the UI. |
| `src/core_widgets` | Reusable Qt widgets and the main feature area. |
| `src/feature_value_widgets` | Per-feature controls: slider, spinbox, combobox, etc. |
| `src/feature_scrollarea` | Scrollable container of feature widgets. |
| `src/action_dialogs` | Actions such as set/read/reset for a feature. |
| `src/option_dialogs` | Application options and diagnostics dialogs. |
| `src/cmdline` | Command line parsing. |
| `src/unit_tests` | Tests for the non-GUI layer. |

## The seam that matters for a port

The important architectural property of `ddcui` is the split between:

1. **DDC access** (`src/nongui`) — talks to monitors through `libddcutil`.
2. **Presentation** (`src/*_widgets`, dialogs) — Qt.

A Rust port should preserve that seam exactly. In `ddcui-rs`:

* (1) becomes the **`ddc-core`** crate — the `DdcBackend` trait plus concrete
  backends. The trait is the Rust equivalent of the `Display_Handle` /
  `DDCA_Feature_Value_Table` surface, expressed in safe Rust and without a
  native dependency.
* (2) becomes the **`ddcui-gpui`** crate — GPUI elements replacing the Qt
  widgets.

## Backend strategy

The upstream build links `libddcutil`. For the port we start with a backend that
shells out to the `ddcutil` executable (same data, no FFI build complexity, and
trivially testable with recorded output). A native backend that speaks DDC/CI
over `/dev/i2c-*` directly (via `ddc-i2c` / `mccs`) is the next step and slots in
behind the same trait. An in-memory backend keeps the UI and tests independent of
hardware.

## Port roadmap

1. **Core** — monitor discovery, VCP read/write, capabilities. *(this crate)*
2. **Brightness/contrast UI** in GPUI. *(this crate)*
3. Native I2C backend behind `DdcBackend`.
4. Remaining feature widgets (input source, colour gains, ...) and dialogs.
5. Packaging (`.desktop`, metainfo, icons) mirrored from upstream.
