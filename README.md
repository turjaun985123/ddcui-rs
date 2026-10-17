# ddcui-rs

[![CI](https://github.com/turjaun985123/ddcui-rs/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/turjaun985123/ddcui-rs/actions/workflows/ci.yml)

A Rust + [GPUI](https://gpui.rs) port of [`ddcui`](https://github.com/rockowitz/ddcui) —
a GUI for controlling external monitors over **DDC/CI** (brightness, contrast, and
other MCCS VCP features).

`ddcui` is a C++/Qt application that talks to monitors through
[`libddcutil`](https://github.com/rockowitz/ddcutil). This project re-implements the
same idea in Rust: a backend-agnostic core crate plus a GPUI desktop front-end.

## Status

Early port. The DDC/CI core is functional and unit-tested; the GPUI window covers
monitor discovery, brightness/contrast control, saved presets and light/dark
themes, and is built in CI. See `docs/` and the tracking issues for the parity
roadmap.

## Features

* **One card per monitor**, with the monitor's name, its display number and where
  it is attached (`display 1 · card1-HDMI-A-1`); cards stack and scroll if you
  have more monitors than fit in the window.
* One row per supported continuous VCP feature, with a **draggable level bar**
  (click anywhere to jump to that value, or press and drag to sweep), tick marks
  every 25 %, `−`/`+` stepper buttons in 5 % steps, and `0/25/50/75/100`
  quick-set buttons.
* **Follows the desktop's colour scheme** — light, dark, or `system` to match
  GNOME; pick one with `--theme`, `DDCUI_THEME`, or leave it to the desktop.
* **No waiting on the monitor**: every write runs on a background worker, one
  command at a time, and repeated changes to the same row are coalesced to the
  newest value — so dragging the level bar stays smooth even when the monitor
  answers slowly. The row shows `writing…` while a value is on its way and
  `write failed` when it did not arrive.
* **Writes follow the hardware, not the list position**: each monitor is
  addressed by a key built from its EDID serial (falling back to the connector it
  hangs on), so a refresh can never send a change to the wrong monitor.
* **Honest state**: a value that could not be re-read is marked as the last
  known one instead of being shown as fresh, and the demo backend (used when
  `ddcutil` is missing) says so in a banner rather than showing invented monitors.
* **Presets**: capture the current brightness/contrast pair and reapply it to
  every monitor with one click. Stored in
  `$XDG_CONFIG_HOME/ddcui-rs/profiles.json`, written atomically, and validated
  when read (an unreadable file is kept as a backup instead of being overwritten).
* A draggable title bar with working minimise/close buttons, a refresh button,
  and a status bar that reports the result of every action.
* **Desktop plugins**: the same binary installs its desktop integrations, so
  there is nothing else to download. `ddcui-rs --plugin-install gnome-quick-menu`
  puts a **brightness button in the GNOME Quick Settings menu** (the top-right
  menu with the power off button) with the presets saved in the window.

## Requirements

**To reach the monitors:**

* an external monitor with **DDC/CI switched on in its own on-screen menu** (it
  usually is). A cable from the monitor straight to the graphics card is the
  most reliable path — some KVMs and adapters do not pass DDC/CI through.
* **`ddcutil`**, the program this app drives (`detect`, `capabilities --brief`,
  `getvcp`/`setvcp`). Install it with your package manager
  (`sudo apt install ddcutil`). Developed and tested against version 2.2.5;
  the interface it uses has been stable for years.
* **read/write access to the monitor's `/dev/i2c-*` device.** On most
  distributions ddcutil's udev rule grants the *logged-in* desktop user access
  to the display buses automatically. If ddcutil says "permission denied"
  instead, add yourself to the `i2c` group (`sudo usermod -aG i2c "$USER"`)
  and sign out and back in.

`ddcutil environment` checks all three and names what is missing. Without
`ddcutil` the window still opens, with a banner saying the monitors are
simulated; the command line never pretends: it reports the real error.

**To run the window:** any Wayland or X11 desktop with a working **Vulkan
driver** — on Debian/Ubuntu that is `mesa-vulkan-drivers`. The other libraries
are the usual desktop ones; the `.deb` lists them all as dependencies.

**For the GNOME Quick Settings button** (`--plugin-install gnome-quick-menu`):
GNOME Shell **45–50**, and the extension finds the `ddcui-rs` command in `PATH`
or in `~/.local/bin`, `/usr/local/bin` or `/usr/bin`. GNOME only scans for new
extensions at startup, so sign out and back in once after installing it.

**AppImage:** needs **FUSE 2** (`libfuse2`/`libfuse2t64`). Without it, run
`APPIMAGE_EXTRACT_AND_RUN=1 ./ddcui-rs-*.AppImage`, or use the tarball or the
`.deb`.

Installed already? `ddcui-rs --version` names the version and `ddcui-rs --list`
shows the monitors it can see.

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

Set up per [Requirements](#requirements) first — a monitor with DDC/CI on,
`ddcutil` installed, and access to its `/dev/i2c-*` bus. Then run:

```bash
cargo run -p ddcui-gpui
```

If the `ddcutil` executable is not found, the app falls back to a mock backend so
the UI is still usable for development; a banner at the top of the window says
that the monitors are simulated. Command-line mode never does this: it reports
the real error and exits non-zero.

The window follows the desktop's colour scheme. `--theme light`, `--theme dark`
or `--theme system` overrides it for one run, and `DDCUI_THEME` does the same for
launches from the desktop menu.

## Command line

The same binary also works headlessly, which is what the desktop *Quick Actions*
use:

```bash
ddcui-rs --list                     # list monitors
ddcui-rs --get-brightness           # print current brightness
ddcui-rs --set-brightness 50        # set brightness to 50%
ddcui-rs --set-brightness 20 --display 2
ddcui-rs --theme dark               # open the window in dark colours
ddcui-rs --plugin-list              # list the desktop plugins
ddcui-rs --plugin-install gnome-quick-menu
ddcui-rs --version                  # print the version
ddcui-rs --help
```

Unlike the GUI, the command-line mode never falls back to the mock backend — if
`ddcutil` is missing it reports the real error and exits non-zero.

## Plugins

A plugin is a directory with a `plugin.json` manifest and the files it installs,
so integrations can be added, removed or written by hand without touching the
binary. `ddcui-rs --plugin-list` shows what is available and
`ddcui-rs --plugin-uninstall <id>` takes it away again. Plugins are looked for in

1. `$DDCUI_PLUGIN_PATH` (colon separated),
2. `~/.local/share/ddcui-rs/plugins` (drop your own in here),
3. `plugins/` next to the binary and `../share/ddcui-rs/plugins` beside it
   (where the tarball, AppImage and `/usr/local` layouts put them), and
4. `/usr/share/ddcui-rs/plugins` (the `.deb`).

The bundled plugin is **`gnome-quick-menu`** (GNOME Shell 45–50). It adds a
`ddcui` cell to the Quick Settings menu — the menu that holds the power off
button — with the brightness of the first monitor in its subtitle, brightness
presets from 0 % to 100 %, the presets saved in the window, and an item that
opens the app. The extension is a thin front end: it runs the `ddcui-rs`
command, so the DDC/CI logic stays in one place.

```bash
ddcui-rs --plugin-install gnome-quick-menu
```

GNOME Shell only scans for new extensions when it starts, so sign out and back in
once after the first install; the extension already comes up enabled. The menu
button then appears without any further setup.

### Is that plugin safe to install?

A plugin copies files into your account, and a shell extension runs inside your
desktop session, so a plugin is as sensitive as a program. The bundled ones ship
with the app and install without a warning; plugins that come from
`$DDCUI_PLUGIN_PATH` or from your own plugin directory are announced (with their
path) before anything is copied, because that is your call to make.

Whatever the source, `--plugin-install` validates before it touches the disk:

* the extension uuid has to be a plain identifier (no `/`, no `..`, no leading
  dot, no quotes), so a manifest can never name a directory outside
  `~/.local/share/gnome-shell/extensions/<uuid>`;
* the `files` field has to be a relative directory that really is inside the
  plugin directory, and symlinks are refused rather than followed;
* the extension is staged in a temporary directory and swapped in, so a failed
  copy cannot leave a half-written extension behind, and an existing
  installation is only replaced after the new one is complete;
* the enabled-extensions list is read before it is written: if it cannot be read,
  the write is skipped instead of dropping the extensions you already had.

`--plugin-uninstall <id>` only removes the directory named after the validated
uuid and refuses to guess anywhere else.

## Install

Prebuilt packages are published on the [Releases](https://github.com/turjaun985123/ddcui-rs/releases)
page:

**AppImage (no install, one file)** — the easiest option (it needs FUSE 2; see
[Requirements](#requirements) for the one-line workaround without it):

```bash
chmod +x ddcui-rs-0.5.1-x86_64.AppImage
./ddcui-rs-0.5.1-x86_64.AppImage            # just run it
```

**Debian / Ubuntu** — install the `.deb` (it pulls `ddcutil` and the desktop runtime libs):

```bash
sudo apt install ./ddcui-rs_0.5.1_amd64.deb
```

**Any Linux (tarball):**

```bash
tar xzf ddcui-rs-0.5.1-x86_64-linux.tar.gz
cd ddcui-rs-0.5.1-x86_64-linux
sudo ./install.sh            # PREFIX=/usr sudo ./install.sh to match distro paths
```

Every release also ships a `SHA256SUMS` file, so you can check what you
downloaded before running it:

```bash
sha256sum --check SHA256SUMS --ignore-missing
```

Every package installs the `ddcui-rs` binary, a `.desktop` entry, an AppStream
`metainfo.xml`, hicolor icons in 16–256 px, and the plugin files (so
`--plugin-install` works offline). The `.desktop` entry appears in
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
