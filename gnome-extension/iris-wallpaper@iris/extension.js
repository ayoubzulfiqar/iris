import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';

const TITLE = 'IrisWallpaper';

function trace(line) {
    try {
        const file = Gio.File.new_for_path('/tmp/iris-extension.log');
        const stream = file.append_to(Gio.FileCreateFlags.NONE, null);
        stream.write_all(`${new Date().toISOString()} ${line}\n`, null);
        stream.close(null);
    } catch (error) {
        log(`Iris wallpaper log failed: ${error}`);
    }
}

export default class IrisWallpaperExtension extends Extension {
    enable() {
        this._enabled = true;
        this._pinned = [];
        try {
            const dir = GLib.build_filenamev([GLib.get_user_state_dir(), 'iris']);
            GLib.mkdir_with_parents(dir, 0o755);
            GLib.file_set_contents(
                GLib.build_filenamev([dir, 'shell-extension']),
                'background-group-1\n');
        } catch (error) {
            log(`Iris wallpaper: ${error}`);
        }
        this._created = global.display.connect('window-created', (_display, win) => {
            this._watch(win);
        });
        this._restacked = global.display.connect('restacked', () => {
            this._schedule();
        });
        trace('enabled');
        for (const actor of global.get_window_actors()) {
            const win = actor.meta_window ?? actor.get_meta_window?.();
            trace(`existing ${win?.get_title?.() ?? ''}`);
            this._watch(win);
        }
    }

    disable() {
        this._enabled = false;
        if (this._created) {
            global.display.disconnect(this._created);
            this._created = 0;
        }
        if (this._restacked) {
            global.display.disconnect(this._restacked);
            this._restacked = 0;
        }
        if (this._pinIdle) {
            GLib.source_remove(this._pinIdle);
            this._pinIdle = 0;
        }
        const background = Main.layoutManager?._backgroundGroup;
        for (const win of this._pinned ?? []) {
            try {
                const actor = win.get_compositor_private();
                const home = win._irisHome;
                if (actor && home && background && actor.get_parent() === background) {
                    background.remove_child(actor);
                    home.add_child(actor);
                }
            } catch (error) {
                console.error(`Iris wallpaper: ${error}`);
            }
        }
        this._pinned = [];
    }

    _watch(win) {
        if (!win || win._irisWatch)
            return;
        win._irisWatch = true;
        win.connect('notify::title', () => this._queue(win));
        win.connect('shown', () => this._queue(win));
        this._queue(win);
    }

    _queue(win) {
        if (!this._enabled || !isWallpaper(win))
            return;
        this._schedule();
    }

    _schedule() {
        if (!this._enabled || this._pinIdle)
            return;
        this._pinIdle = GLib.idle_add(GLib.PRIORITY_DEFAULT_IDLE, () => {
            this._pinIdle = 0;
            if (this._enabled)
                this._pinAll();
            return GLib.SOURCE_REMOVE;
        });
    }

    _pinAll() {
        if (!this._enabled)
            return;
        for (const actor of global.get_window_actors()) {
            const win = actor.meta_window ?? actor.get_meta_window?.();
            if (isWallpaper(win))
                this._pin(win);
        }
    }

    _pin(win) {
        try {
            const actor = win.get_compositor_private();
            const background = Main.layoutManager?._backgroundGroup;
            if (!actor || !background) {
                trace(`no actor for ${win.get_title?.() ?? ''}`);
                return;
            }
            const parent = actor.get_parent();
            // A normal window stays in the window stack, so Mutter draws it
            // above the desktop. Move the actor into the background group,
            // which is already behind every window.
            if (parent && parent !== background) {
                if (!win._irisHome)
                    win._irisHome = parent;
                parent.remove_child(actor);
                background.add_child(actor);
                if (!this._pinned.includes(win))
                    this._pinned.push(win);
            }
            actor.reactive = false;
            actor.opacity = 255;
            if (typeof actor.show === 'function')
                actor.show();
            const box = monitorBox(win) ?? geometry(win.get_title());
            if (box) {
                actor.set_position(box.x, box.y);
                actor.set_size(box.width, box.height);
            }
            if (!win._irisStuck && typeof win.stick === 'function') {
                win._irisStuck = true;
                if (typeof win.is_on_all_workspaces !== 'function' || !win.is_on_all_workspaces())
                    win.stick();
            }
            trace(`pinned ${win.get_title?.() ?? ''} at ${box ? `${box.x},${box.y} ${box.width}x${box.height}` : 'no-box'}`);
        } catch (error) {
            trace(`pin failed: ${error}`);
        }
    }
}

function monitorBox(win) {
    try {
        const index = typeof win.get_monitor === 'function' ? win.get_monitor() : -1;
        const monitor = Main.layoutManager?.monitors?.[index];
        if (!monitor)
            return null;
        return {
            x: monitor.x,
            y: monitor.y,
            width: monitor.width,
            height: monitor.height,
        };
    } catch (error) {
        return null;
    }
}

function isWallpaper(win) {
    if (!win)
        return false;
    const title = win.get_title?.() ?? '';
    return String(title).startsWith(TITLE);
}

function geometry(title) {
    const raw = String(title ?? '').split(':')[1];
    if (!raw)
        return null;
    const parts = raw.split(',').map(Number);
    if (parts.length !== 4 || parts.some(value => !Number.isFinite(value)))
        return null;
    return {x: parts[0], y: parts[1], width: parts[2], height: parts[3]};
}
