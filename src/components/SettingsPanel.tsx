import type { ManagerSettings, ThemeMode } from "../settings";
import type { ScaleMode } from "../types";

interface SettingsPanelProps {
  settings: ManagerSettings;
  onChange: (next: ManagerSettings) => void;
}

export function SettingsPanel({ settings, onChange }: SettingsPanelProps) {
  function patch(partial: Partial<ManagerSettings>) {
    onChange({ ...settings, ...partial });
  }

  return (
    <div className="mx-auto max-w-3xl space-y-6">
      <header>
        <h1 className="text-2xl font-semibold tracking-tight">Settings</h1>
        <p className="mt-1 text-sm text-muted-foreground">
          These options are stored on this computer and sent with every Apply.
        </p>
      </header>

      <section className="space-y-4 rounded-xl border border-border bg-card p-5 shadow-sm">
        <h2 className="text-sm font-semibold">Appearance</h2>
        <label className="block text-sm">
          Theme
          <select
            className="mt-1 w-full rounded-md border border-border bg-input px-3 py-2"
            value={settings.theme}
            onChange={(event) => patch({ theme: event.currentTarget.value as ThemeMode })}
          >
            <option value="system">Match system</option>
            <option value="light">Light</option>
            <option value="dark">Dark</option>
          </select>
        </label>
        <label className="block text-sm">
          Library folder
          <input
            className="mt-1 w-full rounded-md border border-border bg-input px-3 py-2 font-mono text-xs"
            value={settings.libraryPath}
            placeholder="/home/you/Wallpaper"
            onChange={(event) => patch({ libraryPath: event.currentTarget.value })}
          />
        </label>
      </section>

      <section className="space-y-4 rounded-xl border border-border bg-card p-5 shadow-sm">
        <h2 className="text-sm font-semibold">Playback</h2>
        <label className="block text-sm">
          Scale
          <select
            className="mt-1 w-full rounded-md border border-border bg-input px-3 py-2"
            value={settings.scale}
            onChange={(event) => patch({ scale: event.currentTarget.value as ScaleMode })}
          >
            <option value="fill">Fill</option>
            <option value="fit">Fit</option>
            <option value="stretch">Stretch</option>
            <option value="center">Center</option>
          </select>
        </label>
        <label className="block text-sm">
          Playback rate ({settings.rate.toFixed(2)}×)
          <input
            className="mt-2 w-full"
            type="range"
            min={0.25}
            max={2}
            step={0.05}
            value={settings.rate}
            onChange={(event) => patch({ rate: Number(event.currentTarget.value) })}
          />
        </label>
        <label className="block text-sm">
          Volume ({Math.round(settings.volume * 100)}%)
          <input
            className="mt-2 w-full"
            type="range"
            min={0}
            max={1}
            step={0.01}
            value={settings.volume}
            onChange={(event) => patch({ volume: Number(event.currentTarget.value) })}
          />
        </label>
        <label className="flex items-center gap-2 text-sm">
          <input
            type="checkbox"
            checked={settings.looping}
            onChange={(event) => patch({ looping: event.currentTarget.checked })}
          />
          Loop video and scenes
        </label>
        <label className="flex items-center gap-2 text-sm">
          <input
            type="checkbox"
            checked={settings.muted}
            onChange={(event) => patch({ muted: event.currentTarget.checked })}
          />
          Mute audio
        </label>
        <label className="flex items-center gap-2 text-sm">
          <input
            type="checkbox"
            checked={settings.audioReactive}
            onChange={(event) => patch({ audioReactive: event.currentTarget.checked })}
          />
          Audio-reactive scenes and web wallpapers
        </label>
      </section>

      <section className="space-y-4 rounded-xl border border-border bg-card p-5 shadow-sm">
        <h2 className="text-sm font-semibold">Power and focus</h2>
        <label className="block text-sm">
          FPS cap
          <input
            className="mt-1 w-full rounded-md border border-border bg-input px-3 py-2"
            inputMode="numeric"
            placeholder="Display rate"
            value={settings.fpsCap}
            onChange={(event) => patch({ fpsCap: event.currentTarget.value })}
          />
        </label>
        <label className="block text-sm">
          Battery FPS
          <input
            className="mt-1 w-full rounded-md border border-border bg-input px-3 py-2"
            inputMode="numeric"
            value={settings.batteryFps}
            onChange={(event) => patch({ batteryFps: event.currentTarget.value })}
          />
        </label>
        <label className="flex items-center gap-2 text-sm">
          <input
            type="checkbox"
            checked={settings.pauseOnFullscreen}
            onChange={(event) => patch({ pauseOnFullscreen: event.currentTarget.checked })}
          />
          Pause while another window is fullscreen
        </label>
        <label className="flex items-center gap-2 text-sm">
          <input
            type="checkbox"
            checked={settings.pauseOnGame}
            onChange={(event) => patch({ pauseOnGame: event.currentTarget.checked })}
          />
          Pause while a game is in front
        </label>
      </section>
    </div>
  );
}
