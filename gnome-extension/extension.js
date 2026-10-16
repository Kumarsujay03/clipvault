// ClipVault GNOME Shell companion (GNOME 45+).
//
// Wayland does not let ordinary apps grab global keys, place their windows, take focus or
// type into other windows. This extension does exactly those three things for ClipVault:
//   * Super+V (or the shortcut set in ClipVault) toggles the panel,
//   * the panel opens next to the mouse pointer and receives focus,
//   * D-Bus `Paste()` re-focuses the previous window and sends Ctrl+V (Ctrl+Shift+V in
//     terminals).
// It never reads clipboard contents. Disabling it restores any keybinding it changed.

import GLib from 'gi://GLib';
import Gio from 'gi://Gio';
import Meta from 'gi://Meta';
import Shell from 'gi://Shell';
import Clutter from 'gi://Clutter';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

const DBUS_PATH = '/dev/clipvault/Shell';
const DBUS_IFACE = `
<node>
  <interface name="dev.clipvault.Shell">
    <method name="Ping"><arg type="s" direction="out" name="version"/></method>
    <method name="Toggle"/>
    <method name="Paste"/>
  </interface>
</node>`;

const CUSTOM_PATH = '/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/clipvault/';
const TERMINAL_CLASSES = [
    'gnome-terminal', 'org.gnome.terminal', 'kgx', 'org.gnome.console', 'ptyxis', 'org.gnome.ptyxis',
    'kitty', 'alacritty', 'foot', 'tilix', 'terminator', 'xterm', 'konsole', 'wezterm',
    'com.mitchellh.ghostty', 'ghostty', 'blackbox', 'com.raggesilver.blackbox', 'xfce4-terminal',
];

function toAccelerator(shortcut) {
    const parts = String(shortcut || 'Super+V').split('+').map(p => p.trim()).filter(Boolean);
    const key = parts.pop();
    const mods = {super: '<Super>', win: '<Super>', meta: '<Super>', ctrl: '<Primary>', control: '<Primary>', shift: '<Shift>', alt: '<Alt>'};
    let out = '';
    for (const m of parts) {
        const g = mods[m.toLowerCase()];
        if (!g)
            return '<Super>v';
        out += g;
    }
    if (!out || !key)
        return '<Super>v';
    return out + (key.length === 1 ? key.toLowerCase() : key);
}

function readJson(path) {
    try {
        const [ok, bytes] = GLib.file_get_contents(path);
        if (ok)
            return JSON.parse(new TextDecoder().decode(bytes));
    } catch (_) {}
    return null;
}

export default class ClipVaultExtension extends Extension {
    enable() {
        this._configDir = GLib.build_filenamev([GLib.get_user_config_dir(), 'clipvault']);
        this._previous = null;
        this._pointer = null;
        this._placePending = false;
        this._timeouts = new Set();
        this._shellKeys = new Gio.Settings({schema_id: 'org.gnome.shell.keybindings'});
        this._mediaKeys = new Gio.Settings({schema_id: 'org.gnome.settings-daemon.plugins.media-keys'});

        this._dbus = Gio.DBusExportedObject.wrapJSObject(DBUS_IFACE, this);
        this._dbus.export(Gio.DBus.session, DBUS_PATH);

        this._windowCreatedId = global.display.connect('window-created', (_d, win) => this._onWindowCreated(win));
        this._acceleratorId = global.display.connect('accelerator-activated', (_d, action) => {
            if (this._action && action === this._action)
                this.Toggle();
        });
        this._grab(0);
    }

    disable() {
        for (const id of this._timeouts)
            GLib.source_remove(id);
        this._timeouts.clear();
        if (this._action) {
            Main.wm.allowKeybinding(Meta.external_binding_name_for_action(this._action), Shell.ActionMode.NONE);
            global.display.ungrab_accelerator(this._action);
            this._action = 0;
        }
        global.display.disconnect(this._windowCreatedId);
        global.display.disconnect(this._acceleratorId);
        this._dbus.unexport();
        this._dbus = null;

        // Give back what we took.
        if (this._freedSuperV) {
            const tray = this._shellKeys.get_strv('toggle-message-tray');
            if (!tray.some(a => a.toLowerCase() === '<super>v'))
                this._shellKeys.set_strv('toggle-message-tray', [...tray, '<Super>v']);
        }
        if (this._removedCustom) {
            const list = this._mediaKeys.get_strv('custom-keybindings');
            if (!list.includes(CUSTOM_PATH))
                this._mediaKeys.set_strv('custom-keybindings', [...list, CUSTOM_PATH]);
        }
        this._freedSuperV = false;
        this._removedCustom = false;
        this._vk = null;
        this._previous = null;
        this._shellKeys = null;
        this._mediaKeys = null;
    }

    _later(ms, fn) {
        const id = GLib.timeout_add(GLib.PRIORITY_DEFAULT, ms, () => {
            this._timeouts.delete(id);
            fn();
            return GLib.SOURCE_REMOVE;
        });
        this._timeouts.add(id);
    }

    _grab(attempt) {
        const settings = readJson(GLib.build_filenamev([this._configDir, 'settings.json'])) || {};
        const accel = toAccelerator(settings.shortcut);

        if (accel === '<Super>v') {
            const tray = this._shellKeys.get_strv('toggle-message-tray');
            if (tray.some(a => a.toLowerCase() === '<super>v')) {
                this._shellKeys.set_strv('toggle-message-tray', tray.filter(a => a.toLowerCase() !== '<super>v'));
                this._freedSuperV = true;
            }
        }
        // ClipVault's fallback GNOME shortcut would grab the same keys; the extension takes over.
        const list = this._mediaKeys.get_strv('custom-keybindings');
        if (list.includes(CUSTOM_PATH)) {
            this._mediaKeys.set_strv('custom-keybindings', list.filter(p => p !== CUSTOM_PATH));
            this._removedCustom = true;
        }

        const action = global.display.grab_accelerator(accel, Meta.KeyBindingFlags.NONE);
        if (action === Meta.KeyBindingAction.NONE) {
            // The settings daemon may still hold the old grab for a moment.
            if (attempt < 10)
                this._later(500, () => this._grab(attempt + 1));
            else
                console.warn(`[ClipVault] could not grab ${accel}`);
            return;
        }
        this._action = action;
        Main.wm.allowKeybinding(Meta.external_binding_name_for_action(action),
            Shell.ActionMode.NORMAL | Shell.ActionMode.OVERVIEW | Shell.ActionMode.POPUP);
    }

    _isPanel(win) {
        if (!win)
            return false;
        const cls = (win.get_wm_class() || '').toLowerCase();
        const id = (win.get_gtk_application_id?.() || '').toLowerCase();
        return cls === 'clipvault' || id === 'clipvault' || (win.get_title() === 'ClipVault' && cls.includes('clipvault'));
    }

    _isTerminal(win) {
        const cls = (win.get_wm_class() || '').toLowerCase();
        return TERMINAL_CLASSES.some(t => cls === t || cls.startsWith(`${t}-`)) || cls.includes('terminal');
    }

    _findPanel() {
        return global.display.list_all_windows().find(w => this._isPanel(w)) ?? null;
    }

    _launch(arg) {
        const info = readJson(GLib.build_filenamev([this._configDir, 'launcher.json']));
        let argv = Array.isArray(info?.ui) && info.ui.length ? [...info.ui] : null;
        if (!argv) {
            const found = GLib.find_program_in_path('clipvault');
            if (!found) {
                Main.notify('ClipVault', 'ClipVault is not installed or has never been started.');
                return;
            }
            argv = [found];
        }
        try {
            Gio.Subprocess.new([...argv, arg], Gio.SubprocessFlags.NONE);
        } catch (e) {
            console.warn(`[ClipVault] failed to launch: ${e.message}`);
        }
    }

    // ---- D-Bus methods

    Ping() {
        return String(this.metadata.version ?? '1');
    }

    Toggle() {
        const panel = this._findPanel();
        if (panel && panel.has_focus()) {
            this._launch('--hide');
            return;
        }
        const focus = global.display.get_focus_window();
        if (focus && !this._isPanel(focus))
            this._previous = focus;
        const [x, y] = global.get_pointer();
        this._pointer = {x, y};
        this._placePending = true;
        if (panel && panel.showing_on_its_workspace?.())
            this._place(panel);
        this._launch('--show');
    }

    Paste() {
        const target = this._previous;
        const alive = target && global.display.list_all_windows().includes(target);
        if (alive && !target.has_focus()) {
            Main.activateWindow(target);
            this._later(120, () => this._sendPaste(target));
        } else {
            this._later(60, () => this._sendPaste(alive ? target : global.display.get_focus_window()));
        }
    }

    // ---- helpers

    _onWindowCreated(win) {
        if (!this._isPanel(win) || !this._placePending)
            return;
        const id = win.connect('shown', () => {
            win.disconnect(id);
            this._place(win);
        });
    }

    _place(win) {
        this._placePending = false;
        const p = this._pointer ?? (() => {
            const [x, y] = global.get_pointer();
            return {x, y};
        })();
        const frame = win.get_frame_rect();
        const monitor = global.display.get_current_monitor();
        const area = Main.layoutManager.getWorkAreaForMonitor(monitor);
        const margin = 8;
        let x = Math.round(p.x - frame.width / 2);
        let y = p.y + 12;
        if (y + frame.height > area.y + area.height - margin)
            y = p.y - frame.height - 12;
        x = Math.max(area.x + margin, Math.min(x, area.x + area.width - frame.width - margin));
        y = Math.max(area.y + margin, Math.min(y, area.y + area.height - frame.height - margin));
        win.move_frame(true, x, y);
        Main.activateWindow(win);
    }

    _sendPaste(target) {
        if (!this._vk) {
            const seat = Clutter.get_default_backend().get_default_seat();
            this._vk = seat.create_virtual_device(Clutter.InputDeviceType.KEYBOARD_DEVICE);
        }
        const terminal = target && this._isTerminal(target);
        const t = GLib.get_monotonic_time();
        const key = (sym, state) => this._vk.notify_keyval(t, sym, state);
        key(Clutter.KEY_Control_L, Clutter.KeyState.PRESSED);
        if (terminal)
            key(Clutter.KEY_Shift_L, Clutter.KeyState.PRESSED);
        key(Clutter.KEY_v, Clutter.KeyState.PRESSED);
        key(Clutter.KEY_v, Clutter.KeyState.RELEASED);
        if (terminal)
            key(Clutter.KEY_Shift_L, Clutter.KeyState.RELEASED);
        key(Clutter.KEY_Control_L, Clutter.KeyState.RELEASED);
    }
}
