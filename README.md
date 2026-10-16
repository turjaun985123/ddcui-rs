# ddcui-rs

[![CI](https://github.com/jerinshop7-maker/ddcui-rs/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/jerinshop7-maker/ddcui-rs/actions/workflows/ci.yml)

A Rust + [GPUI](https://gpui.rs) port of [`ddcui`](https://github.com/rockowitz/ddcui) —
a GUI for controlling external monitors over **DDC/CI** (brightness, contrast, and
other MCCS VCP features).

`ddcui` is a C++/Qt application that talks to monitors through
[`libddcutil`](https://github.com/rockowitz/ddcutil). This project re-implements the
same idea in Rust: a backend-agnostic core crate plus a GPUI desktop front-end.

## Status

Early port. The DDC/CI core is functional and unit-tested; the GPUI UI covers
monitor discovery and brightness control and is built in CI. See `docs/` and the
tracking issues for the parity roadmap.

## Workspace layout

| Path | What it is |
| --- | --- |
| `crates/ddc-core` | Backend-agnostic DDC/CI logic: monitor model, VCP feature codes, the `DdcBackend` trait, a `ddcutil` CLI backend, and an in-memory mock backend for tests. No GUI, no hardware required to test. |
| `crates/ddcui-gpui` | GPUI desktop application (`ddcui-rs` binary). |

## Mapping from the original `ddcui`

| `ddcui` (C++/Qt) | `ddcui-rs` (Rust/GPUI) |
| --- | --- |
| `src/nongui` (DDC access, no Qt) | `ddc-core` (`DdcBackend` trait, `ddcutil` backend) |
| `src/monitor_desc` (monitor model) | `ddc-core::model::MonitorId` |
| `src/feature_value_widgets` (per-feature controls) | `ddcui-gpui` elements |
| `src/core_widgets`, `src/feature_scrollarea` | `ddcui-gpui` layout |
| `src/action_dialogs`, `src/option_dialogs` | `ddcui-gpui` (partial) |
| `libddcutil` | `ddcutil` CLI (default) or native Rust DDC backend (planned) |

## Building

This project is developed and tested entirely in CI (GitHub Actions). Locally:

```bash
cargo build -p ddcui-gpui
cargo test -p ddc-core
```

On Linux the GPUI front-end needs the usual desktop build dependencies
(Wayland/X11, fontconfig/freetype, Vulkan, clang for bindgen). The CI workflow
installs them; see `.github/workflows/ci.yml`.

## Running

DDC/CI must be enabled in the monitor's on-screen menu, and the user needs access
to the `/dev/i2c-*` devices (see the `ddcutil` documentation). Run:

```bash
cargo run -p ddcui-gpui
```

If the `ddcutil` executable is not found, the app falls back to a mock backend so
the UI is still usable for development.

## Dependency patch

`xattr 0.2.3` (pulled in transitively by the GPUI dependency tree) references
`libc::ENOATTR`, which libc removed on Linux/Android after `0.2.126` (it was a
deprecated alias for `ENODATA`). The crate is vendored under `vendor/xattr` with
a one-line platform fix and wired in via `[patch.crates-io]` in the workspace
`Cargo.toml`.

## License

GPL-2.0-or-later, matching the upstream `ddcui` project.
