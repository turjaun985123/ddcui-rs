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
const MAX_PROFILE_BYTES = 64 * 1024;
const MAX_PROFILES = 64;

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
        const info = file.query_info('standard::size', Gio.FileQueryInfoFlags.NONE, null);
        if (info.get_size() > MAX_PROFILE_BYTES)
            return null;
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
        .filter(profile => Number.isInteger(profile?.brightness) &&
            profile.brightness >= 0 && profile.brightness <= 100 &&
            (profile.contrast === null || profile.contrast === undefined ||
                (Number.isInteger(profile.contrast) && profile.contrast >= 0 &&
                    profile.contrast <= 100)))
        .slice(0, MAX_PROFILES)
        .map(profile => ({
            brightness: profile.brightness,
            contrast: Number.isInteger(profile.contrast) ? profile.contrast : null,
        }));
}

/** Run a ddcui-rs command and report its exit status and captured output. */
function runCommand(command, args) {
    return new Promise((resolve, reject) => {
        let proc;
        try {
            proc = new Gio.Subprocess({
                argv: [command, ...args],
                flags: Gio.SubprocessFlags.STDOUT_PIPE | Gio.SubprocessFlags.STDERR_PIPE,
            });
            proc.init(null);
        } catch (e) {
            reject(e);
            return;
        }

        proc.communicate_utf8_async(null, null, (source, result) => {
            try {
                const [ok, stdout, stderr] = source.communicate_utf8_finish(result);
                if (!ok) {
                    reject(new Error(firstLine(stderr) ?? 'could not read command output'));
                } else if (!source.get_successful()) {
                    reject(new Error(firstLine(stderr) ?? 'ddcui-rs command failed'));
                } else {
                    resolve({ stdout: stdout ?? '', stderr: stderr ?? '' });
                }
            } catch (e) {
                reject(e);
            }
        });
    });
}

/** Read the first monitor's brightness.
 *
 * Return a percentage or null, plus a short hint to show when the read failed.
 */
async function readBrightness(command) {
    try {
        const { stdout, stderr } = await runCommand(command, ['--get-brightness']);
        const match = stdout.match(/(\d+)\s*%/);
        if (match) {
            const percent = Number.parseInt(match[1], 10);
            if (percent >= 0 && percent <= 100)
                return { percent, hint: null };
        }
        return { percent: null, hint: firstLine(stderr) ?? 'ddcutil could not read the monitor' };
    } catch (e) {
        logError(e, 'ddcui-rs: could not read monitor brightness');
        return { percent: null, hint: firstLine(e.message) ?? 'ddcui-rs could not be started' };
    }
}

/** The first non-empty line of some command output, shortened to fit a menu. */
function firstLine(text) {
    const line = (text ?? '').split('\n').map(part => part.trim()).find(Boolean);
    if (!line)
        return null;
    return line.length > 72 ? `${line.slice(0, 69)}…` : line;
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
        this._refreshGeneration = 0;
        this._refreshTask = null;
        this._writeQueue = Promise.resolve();
        /** The last thing we learned about the monitor, shown in the menu. */
        this._hint = null;

        this._setHeader();

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
        this._setHeader();

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
                this._refreshWhenIdle();
                return GLib.SOURCE_REMOVE;
            });
        });
        this.menu.addMenuItem(refreshItem);
    }

    /** Apply a preset saved in the window (brightness, and contrast if it has one). */
    _applyProfile(profile) {
        if (!this._command)
            return;
        this._queueWrite(async () => {
            await runCommand(this._command, ['--set-brightness', `${profile.brightness}`]);
            if (profile.contrast !== null)
                await runCommand(this._command, ['--set-contrast', `${profile.contrast}`]);
        }, 'could not apply preset');
    }

    _setBrightness(percent) {
        if (!this._command)
            return;
        this._queueWrite(async () => {
            await runCommand(this._command, ['--set-brightness', `${percent}`]);
        }, 'could not set brightness');
    }

    /** Serialize writes, report errors, and refresh only after the write ends. */
    _queueWrite(write, failureTitle) {
        this._refreshGeneration++;
        this.subtitle = 'setting…';
        this._hint = null;
        this._setHeader();

        this._writeQueue = this._writeQueue
            .catch(() => {})
            .then(async () => {
                if (this._destroyed)
                    return;
                const previousRefresh = this._refreshTask;
                if (previousRefresh)
                    await previousRefresh;
                if (this._refreshTask === previousRefresh)
                    this._refreshTask = null;
                this.subtitle = 'setting…';
                this._hint = null;
                this._setHeader();
                await write();
                await this._refresh();
            })
            .catch(async e => {
                if (this._destroyed)
                    return;
                const detail = firstLine(e.message) ?? 'ddcui-rs could not apply the change';
                await this._refresh();
                if (this._destroyed)
                    return;
                this.subtitle = `${this.subtitle} · write failed`;
                this._hint = `${failureTitle}: ${detail}`;
                this._setHeader();
            });
    }

    /** Let an explicit refresh wait behind any queued hardware changes. */
    _refreshWhenIdle() {
        this._writeQueue.then(() => this._refresh());
    }

    /** Open the ddcui-rs window. */
    _openWindow() {
        if (!this._command)
            return;
        try {
            Util.spawn([this._command]);
        } catch (e) {
            logError(e, 'ddcui-rs: could not open the window');
        }
    }

    /** Update the subtitle and the list of saved presets. */
    _refresh() {
        if (this._refreshTask)
            return this._refreshTask;

        const task = this._refreshNow();
        this._refreshTask = task;
        task.then(
            () => {
                if (this._refreshTask === task)
                    this._refreshTask = null;
            },
            () => {
                if (this._refreshTask === task)
                    this._refreshTask = null;
            });
        return task;
    }

    async _refreshNow() {
        const generation = ++this._refreshGeneration;
        this._buildMenu();

        if (!this._command) {
            this.subtitle = 'ddcui-rs not found';
            return;
        }

        const { percent, hint } = await readBrightness(this._command);
        if (this._destroyed || generation !== this._refreshGeneration)
            return;
        if (percent === null) {
            this.subtitle = 'brightness unknown';
            this._hint = hint;
        } else {
            this.subtitle = `brightness ${percent}%`;
            this._hint = null;
        }
        this._setHeader();
    }

    /** The menu header, with whatever we last learned about the monitor. */
    _setHeader() {
        if (!this._command) {
            this.menu.setHeader('video-display-symbolic', 'Monitor brightness',
                'install ddcui-rs to use it');
            return;
        }
        this.menu.setHeader('video-display-symbolic', 'Monitor brightness',
            this._hint ?? 'presets are brightness / contrast');
    }

    destroy() {
        this._destroyed = true;
        this._refreshGeneration++;
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
