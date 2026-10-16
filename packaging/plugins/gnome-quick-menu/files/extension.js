// ddcui-rs – monitor brightness in the GNOME Quick Settings menu.
//
// Part of ddcui-rs (GPL-2.0-or-later). This extension is a thin front end:
// every action runs the `ddcui-rs` command, so the DDC/CI logic lives in one
// place, shared with the window.
//
// Install it with `ddcui-rs --plugin-install gnome-quick-menu`.

import GObject from 'gi://GObject';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';

import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import * as PopupMenu from 'resource:///org/gnome/shell/ui/popupMenu.js';
import * as Util from 'resource:///org/gnome/shell/misc/util.js';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';
import {QuickMenuToggle, SystemIndicator} from 'resource:///org/gnome/shell/ui/quickSettings.js';

const BRIGHTNESS_PRESETS = [0, 25, 50, 75, 100];
const REFRESH_DELAY_SECONDS = 2;

/** The ddcui-rs command: `PATH` first, then the usual install locations. */
function findCommand() {
    const fromPath = GLib.find_program_in_path('ddcui-rs');
    if (fromPath)
        return fromPath;

    const candidates = [
        GLib.build_filenamev([GLib.get_home_dir(), '.local', 'bin', 'ddcui-rs']),
        '/usr/local/bin/ddcui-rs',
        '/usr/bin/ddcui-rs',
    ];
    for (const path of candidates) {
        if (GLib.file_test(path, GLib.FileTest.IS_EXECUTABLE))
            return path;
    }
    return null;
}

/** Read a small text file, or null. */
function readTextFile(path) {
    try {
        const file = Gio.File.new_for_path(path);
        const [ok, contents] = file.load_contents(null);
        return ok ? new TextDecoder().decode(contents) : null;
    } catch {
        return null;
    }
}

/** The presets saved in the ddcui-rs window, if any. */
function loadProfiles() {
    const path = GLib.build_filenamev([
        GLib.get_user_config_dir(), 'ddcui-rs', 'profiles.json',
    ]);
    const text = readTextFile(path);
    if (!text)
        return [];

    let parsed;
    try {
        parsed = JSON.parse(text);
    } catch {
        return [];
    }
    if (!Array.isArray(parsed))
        return [];

    return parsed
        .filter(profile => Number.isInteger(profile?.brightness))
        .map(profile => ({
            brightness: profile.brightness,
            contrast: Number.isInteger(profile.contrast) ? profile.contrast : null,
        }));
}

/** Run ddcui-rs in the background and ignore the result. */
function spawnCommand(command, args) {
    try {
        Util.spawn([command, ...args]);
    } catch (e) {
        logError(e, 'ddcui-rs: could not run the command');
    }
}

/** Read the first monitor's brightness; the callback gets a percentage or null. */
function readBrightness(command, callback) {
    let proc;
    try {
        proc = new Gio.Subprocess({
            argv: [command, '--get-brightness'],
            flags: Gio.SubprocessFlags.STDOUT_PIPE | Gio.SubprocessFlags.STDERR_SILENCE,
        });
        proc.init(null);
    } catch (e) {
        logError(e, 'ddcui-rs: could not start the command');
        callback(null);
        return;
    }

    proc.communicate_utf8_async(null, null, (source, result) => {
        let percent = null;
        try {
            const [, stdout] = source.communicate_utf8_finish(result);
            const match = (stdout ?? '').match(/(\d+)\s*%/);
            if (match)
                percent = Number.parseInt(match[1], 10);
        } catch (e) {
            logError(e, 'ddcui-rs: could not read the brightness');
        }
        callback(percent);
    });
}

const DdcToggle = GObject.registerClass(
class DdcToggle extends QuickMenuToggle {
    _init(command) {
        super._init({
            title: 'ddcui',
            subtitle: command ? 'brightness …' : 'ddcui-rs not found',
            iconName: 'video-display-symbolic',
        });

        this._command = command;
        this._destroyed = false;
        this._refreshSource = 0;

        this.menu.setHeader('video-display-symbolic', 'Monitor brightness',
            command ? 'presets are brightness / contrast' : 'install ddcui-rs to use it');

        // The cell behaves like a button: open the window and drop the
        // "pressed" look again. The arrow on the right opens the menu.
        this.connect('clicked', () => {
            this.checked = false;
            this._openWindow();
        });

        this._refresh();
    }

    /** (Re)build the menu: saved presets, brightness presets, actions. */
    _buildMenu() {
        this.menu.removeAll();

        if (!this._command) {
            this.menu.addMenuItem(new PopupMenu.PopupMenuItem('ddcui-rs was not found'));
        } else {
            const profiles = loadProfiles();
            for (const profile of profiles) {
                const label = `Preset ${profile.brightness}/${profile.contrast ?? '–'}`;
                const item = new PopupMenu.PopupMenuItem(label);
                item.connect('activate', () => this._applyProfile(profile));
                this.menu.addMenuItem(item);
            }
            if (profiles.length > 0)
                this.menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem());

            for (const percent of BRIGHTNESS_PRESETS) {
                const item = new PopupMenu.PopupMenuItem(`Brightness ${percent}%`);
                item.connect('activate', () => this._setBrightness(percent));
                this.menu.addMenuItem(item);
            }

            this.menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem());

            const openItem = new PopupMenu.PopupMenuItem('Open ddcui-rs');
            openItem.connect('activate', () => this._openWindow());
            this.menu.addMenuItem(openItem);
        }

        const refreshItem = new PopupMenu.PopupMenuItem('Refresh');
        refreshItem.connect('activate', () => {
            // Rebuild after the menu has closed and this item is gone.
            GLib.idle_add(GLib.PRIORITY_DEFAULT_IDLE, () => {
                this._refresh();
                return GLib.SOURCE_REMOVE;
            });
        });
        this.menu.addMenuItem(refreshItem);
    }

    /** Apply a preset saved in the window (brightness, and contrast if it has one). */
    _applyProfile(profile) {
        if (!this._command)
            return;
        spawnCommand(this._command, ['--set-brightness', `${profile.brightness}`]);
        if (profile.contrast !== null)
            spawnCommand(this._command, ['--set-contrast', `${profile.contrast}`]);
        this.subtitle = `brightness ${profile.brightness}%`;
        this._scheduleRefresh();
    }

    _setBrightness(percent) {
        if (!this._command)
            return;
        spawnCommand(this._command, ['--set-brightness', `${percent}`]);
        this.subtitle = `brightness ${percent}%`;
        this._scheduleRefresh();
    }

    /** Open the ddcui-rs window. */
    _openWindow() {
        if (!this._command)
            return;
        spawnCommand(this._command, []);
    }

    /** Update the subtitle and the list of saved presets. */
    _refresh() {
        this._buildMenu();

        if (!this._command) {
            this.subtitle = 'ddcui-rs not found';
            return;
        }

        readBrightness(this._command, percent => {
            if (this._destroyed)
                return;
            this.subtitle = percent === null
                ? 'brightness unknown'
                : `brightness ${percent}%`;
        });
    }

    /** Read the monitor again once ddcutil had time to apply a change. */
    _scheduleRefresh() {
        if (this._refreshSource !== 0)
            GLib.source_remove(this._refreshSource);

        this._refreshSource = GLib.timeout_add_seconds(
            GLib.PRIORITY_DEFAULT, REFRESH_DELAY_SECONDS, () => {
                this._refreshSource = 0;
                this._refresh();
                return GLib.SOURCE_REMOVE;
            });
    }

    destroy() {
        this._destroyed = true;
        if (this._refreshSource !== 0) {
            GLib.source_remove(this._refreshSource);
            this._refreshSource = 0;
        }
        super.destroy();
    }
});

const DdcIndicator = GObject.registerClass(
class DdcIndicator extends SystemIndicator {
    _init() {
        super._init();
        // No panel icon: the button only shows up inside Quick Settings.
        this._toggle = new DdcToggle(findCommand());
        this.quickSettingsItems.push(this._toggle);
    }
});

export default class DdcuiRsExtension extends Extension {
    enable() {
        this._indicator = new DdcIndicator();
        Main.panel.statusArea.quickSettings.addExternalIndicator(this._indicator);
    }

    disable() {
        this._indicator?.quickSettingsItems.forEach(item => item.destroy());
        this._indicator?.destroy();
        this._indicator = null;
    }
}
