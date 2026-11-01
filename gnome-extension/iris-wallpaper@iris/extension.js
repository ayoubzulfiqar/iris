import Clutter from 'gi://Clutter';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import GObject from 'gi://GObject';
import Meta from 'gi://Meta';
import {Extension, InjectionManager} from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Background from 'resource:///org/gnome/shell/ui/background.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';

const TITLE = 'IrisWallpaper';
const STAMP = 'clone-background-3\n';

function windowTitle(win) {
    return String(win?.get_title?.() ?? win?.title ?? '');
}

function isWallpaperWindow(win) {
    return windowTitle(win).startsWith(TITLE);
}

function trace(line) {
    try {
        const file = Gio.File.new_for_path('/tmp/iris-extension.log');
        const stream = file.append_to(Gio.FileCreateFlags.NONE, null);
        stream.write_all(`${new Date().toISOString()} ${line}\n`, null);
        stream.close(null);
    } catch (error) {
        log(`Iris wallpaper: ${error}`);
    }
}

function rendererFor(monitor) {
    return global.get_window_actors().find(actor => {
        const win = actor.meta_window;
        return isWallpaperWindow(win) && (win.get_monitor?.() ?? 0) === monitor;
    }) ?? null;
}

const WallpaperLayer = GObject.registerClass(
class WallpaperLayer extends Clutter.Actor {
    _init(backgroundActor) {
        super._init({
            reactive: false,
            x_expand: true,
            y_expand: true,
            width: backgroundActor.width,
            height: backgroundActor.height,
        });
        this._monitor = backgroundActor.monitor ?? 0;
        this._clone = null;
        if (!backgroundActor.layout_manager)
            backgroundActor.layout_manager = new Clutter.BinLayout();
        backgroundActor.add_child(this);
        this.attach();
    }

    attach() {
        if (this._clone)
            return;
        const source = rendererFor(this._monitor);
        if (!source)
            return;
        this._clone = new Clutter.Clone({source, reactive: false});
        this._clone.source.connect('destroy', () => {
            this._clone?.destroy();
            this._clone = null;
        });
        this.add_child(this._clone);
        trace(`cloned monitor ${this._monitor}`);
    }
});

export default class IrisWallpaperExtension extends Extension {
    enable() {
        this._enabled = true;
        this._layers = [];
        this._injection = new InjectionManager();
        try {
            const dir = GLib.build_filenamev([GLib.get_user_state_dir(), 'iris']);
            GLib.mkdir_with_parents(dir, 0o755);
            GLib.file_set_contents(GLib.build_filenamev([dir, 'shell-extension']), STAMP);
        } catch (error) {
            trace(`stamp failed: ${error}`);
        }
        this._created = global.display.connect('window-created', (_display, win) => {
            GLib.idle_add(GLib.PRIORITY_DEFAULT_IDLE, () => {
                this._watch(win);
                return GLib.SOURCE_REMOVE;
            });
        });
        this._injectBackground();
        this._hideFromSwitcher();
        try {
            Main.layoutManager._updateBackgrounds();
        } catch (error) {
            trace(`background refresh failed: ${error}`);
        }
        for (const actor of global.get_window_actors())
            this._watch(actor.meta_window);
        this._timer = GLib.timeout_add_seconds(GLib.PRIORITY_DEFAULT, 1, () => {
            if (!this._enabled)
                return GLib.SOURCE_REMOVE;
            this._parkAll();
            for (const layer of this._layers)
                layer.attach();
            return GLib.SOURCE_CONTINUE;
        });
        trace('enabled clone background');
    }

    disable() {
        this._enabled = false;
        if (this._created) {
            global.display.disconnect(this._created);
            this._created = 0;
        }
        if (this._timer) {
            GLib.source_remove(this._timer);
            this._timer = 0;
        }
        this._injection.clear();
        for (const layer of this._layers)
            layer.destroy();
        this._layers = [];
        try {
            Main.layoutManager._updateBackgrounds();
        } catch (error) {
            trace(`background restore failed: ${error}`);
        }
    }

    _injectBackground() {
        const layers = this._layers;
        this._injection.overrideMethod(
            Background.BackgroundManager.prototype,
            '_createBackgroundActor',
            original => {
                return function () {
                    const backgroundActor = original.call(this);
                    try {
                        const layer = new WallpaperLayer(backgroundActor);
                        layers.push(layer);
                        layer.connect('destroy', () => {
                            const index = layers.indexOf(layer);
                            if (index >= 0)
                                layers.splice(index, 1);
                        });
                    } catch (error) {
                        log(`Iris wallpaper layer: ${error}`);
                    }
                    return backgroundActor;
                };
            });
    }

    _hideFromSwitcher() {
        this._injection.overrideMethod(Meta.Display.prototype, 'get_tab_list', original => {
            return function (type, workspace) {
                const windows = original.apply(this, [type, workspace]);
                return windows.filter(win => !isWallpaperWindow(win));
            };
        });
    }

    _watch(win) {
        if (!this._enabled || !win || win._irisWatch || !isWallpaperWindow(win))
            return;
        win._irisWatch = true;
        win.connect('shown', () => this._park(win));
        win.connect('raised', () => this._park(win));
        this._park(win);
    }

    _parkAll() {
        for (const actor of global.get_window_actors()) {
            const win = actor.meta_window;
            if (isWallpaperWindow(win))
                this._park(win);
        }
    }

    _park(win) {
        try {
            const index = win.get_monitor?.() ?? 0;
            const monitor = Main.layoutManager.monitors[index] ?? Main.layoutManager.primaryMonitor;
            if (!monitor)
                return;
            win.move_resize_frame(
                false,
                monitor.x,
                monitor.y + monitor.height + 80,
                monitor.width,
                monitor.height);
            if (typeof win.lower === 'function')
                win.lower();
        } catch (error) {
            trace(`park failed: ${error}`);
        }
    }
}
