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
monitor discovery, brightness/contrast control and saved presets and is built in
CI. See `docs/` and the tracking issues for the parity roadmap.

## Features

* Discovers every DDC/CI monitor the active backend can see.
* One row per supported continuous VCP feature, with a **draggable level bar**
  (click anywhere to jump to that value, or press and drag to sweep), `−`/`+`
  stepper buttons in 5% steps, and `0/25/50/75/100` quick-set buttons.
* **Presets**: capture the current brightness/contrast pair and reapply it to
  every monitor with one click. Stored in
  `$XDG_CONFIG_HOME/ddcui-rs/profiles.json`.
* A draggable title bar with working minimise/close buttons, a refresh button,
  and a status bar that reports the result of every action.

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

## Command line

The same binary also works headlessly, which is what the desktop *Quick Actions*
use:

```bash
ddcui-rs --list                     # list monitors
ddcui-rs --get-brightness           # print current brightness
ddcui-rs --set-brightness 50        # set brightness to 50%
ddcui-rs --set-brightness 20 --display 2
ddcui-rs --help
```

Unlike the GUI, the command-line mode never falls back to the mock backend — if
`ddcutil` is missing it reports the real error and exits non-zero.

## Install

Prebuilt packages are published on the [Releases](https://github.com/jerinshop7-maker/ddcui-rs/releases)
page:

**AppImage (no install, one file)** — the easiest option:

```bash
chmod +x ddcui-rs-0.2.0-x86_64.AppImage
./ddcui-rs-0.2.0-x86_64.AppImage            # just run it
```

**Debian / Ubuntu** — install the `.deb` (it pulls `ddcutil` and the desktop runtime libs):

```bash
sudo apt install ./ddcui-rs_0.2.0_amd64.deb
```

**Any Linux (tarball):**

```bash
tar xzf ddcui-rs-0.2.0-x86_64-linux.tar.gz
cd ddcui-rs-0.2.0-x86_64-linux
sudo ./install.sh            # PREFIX=/usr sudo ./install.sh to match distro paths
```

Every package installs the `ddcui-rs` binary, a `.desktop` entry, an AppStream
`metainfo.xml`, and hicolor icons in 16–256 px. The `.desktop` entry appears in
the Activities/app-grid search and carries brightness **Quick Actions** (25/50/75/100 %),
so you can right-click the launcher icon to set brightness without opening the
window. The icon set is the upstream `ddcui` artwork from KDE Oxygen (LGPL-3.0;
see `packaging/icons/COPYING`).

Releases are built by `.github/workflows/release.yml` whenever a `v*` tag is
pushed, or manually via *Actions → Release → Run workflow*. The release profile is
tuned for size (`opt-level = "s"`, fat LTO, `codegen-units = 1`, `panic = "abort"`,
symbols stripped).

## Dependency patch

`xattr 0.2.3` (pulled in transitively by the GPUI dependency tree) references
`libc::ENOATTR`, which libc removed on Linux/Android after `0.2.126` (it was a
deprecated alias for `ENODATA`). The crate is vendored under `vendor/xattr` with
a one-line platform fix and wired in via `[patch.crates-io]` in the workspace
`Cargo.toml`.

## License

GPL-2.0-or-later, matching the upstream `ddcui` project.
