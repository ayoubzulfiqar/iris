import type { ManagerSettings, SavedWallpaper } from "../settings";

interface PlaylistPanelProps {
  settings: ManagerSettings;
  current: SavedWallpaper | null;
  onChange: (next: ManagerSettings) => void;
  onPlay: (item: SavedWallpaper) => void;
  onNext: () => void;
  pending: boolean;
}

export function PlaylistPanel({
  settings,
  current,
  onChange,
  onPlay,
  onNext,
  pending,
}: PlaylistPanelProps) {
  function patch(partial: Partial<ManagerSettings>) {
    onChange({ ...settings, ...partial });
  }

  function addCurrent() {
    if (!current || current.kind === "directory") return;
    if (settings.playlist.some((item) => item.path === current.path)) return;
    patch({ playlist: [...settings.playlist, current] });
  }

  function removeAt(index: number) {
    patch({ playlist: settings.playlist.filter((_, itemIndex) => itemIndex !== index) });
  }

  function move(index: number, direction: -1 | 1) {
    const next = settings.playlist.slice();
    const target = index + direction;
    const item = next[index];
    const swap = next[target];
    if (!item || !swap) return;
    next[index] = swap;
    next[target] = item;
    patch({ playlist: next });
  }

  return (
    <div className="mx-auto max-w-3xl space-y-6">
      <header>
        <h1 className="text-2xl font-semibold tracking-tight">Playlist</h1>
        <p className="mt-1 text-sm text-muted-foreground">
          Iris applies the next wallpaper on a timer. The list stays on this computer.
        </p>
      </header>

      <section className="space-y-4 rounded-xl border border-border bg-card p-5 shadow-sm">
        <label className="flex items-center gap-2 text-sm">
          <input
            type="checkbox"
            checked={settings.playlistEnabled}
            onChange={(event) => patch({ playlistEnabled: event.currentTarget.checked })}
          />
          Rotate automatically
        </label>
        <label className="flex items-center gap-2 text-sm">
          <input
            type="checkbox"
            checked={settings.shufflePlaylist}
            onChange={(event) => patch({ shufflePlaylist: event.currentTarget.checked })}
          />
          Shuffle
        </label>
        <label className="block text-sm">
          Seconds between wallpapers
          <input
            className="mt-1 w-full rounded-md border border-border bg-input px-3 py-2"
            inputMode="numeric"
            min={5}
            value={settings.playlistSeconds}
            onChange={(event) => {
              const parsed = Number(event.currentTarget.value);
              const seconds = Number.isFinite(parsed) && parsed >= 5 ? Math.floor(parsed) : 5;
              patch({ playlistSeconds: seconds, playlistMinutes: Math.max(1, Math.round(seconds / 60)) });
            }}
          />
        </label>
        <div className="flex flex-wrap gap-2">
          <button
            type="button"
            className="rounded-md bg-secondary px-3 py-2 text-sm"
            onClick={addCurrent}
            disabled={!current || current.kind === "directory"}
          >
            Add current
          </button>
          <button
            type="button"
            className="rounded-md bg-primary px-3 py-2 text-sm text-primary-foreground disabled:opacity-50"
            onClick={onNext}
            disabled={pending || settings.playlist.length === 0}
          >
            Next wallpaper
          </button>
        </div>
      </section>

      {settings.playlist.length === 0 ? (
        <p className="rounded-xl border border-dashed border-border p-8 text-center text-sm text-muted-foreground">
          Apply a wallpaper or star one, then add it here.
        </p>
      ) : (
        <ol className="space-y-2">
          {settings.playlist.map((item, index) => (
            <li
              key={item.path}
              className="flex items-center gap-3 rounded-xl border border-border bg-card px-4 py-3"
            >
              <div className="min-w-0 flex-1">
                <p className="truncate text-sm font-medium">{item.title}</p>
                <p className="truncate font-mono text-xs text-muted-foreground">{item.path}</p>
              </div>
              <button type="button" className="text-xs" onClick={() => onPlay(item)} disabled={pending}>
                Play
              </button>
              <button type="button" className="text-xs" onClick={() => move(index, -1)} disabled={index === 0}>
                Up
              </button>
              <button
                type="button"
                className="text-xs"
                onClick={() => move(index, 1)}
                disabled={index === settings.playlist.length - 1}
              >
                Down
              </button>
              <button type="button" className="text-xs text-destructive" onClick={() => removeAt(index)}>
                Remove
              </button>
            </li>
          ))}
        </ol>
      )}

      {settings.recent.length > 0 ? (
        <section className="space-y-2">
          <h2 className="text-sm font-semibold">Recent</h2>
          <ul className="space-y-2">
            {settings.recent.map((item) => (
              <li key={item.path}>
                <button
                  type="button"
                  className="w-full truncate rounded-lg border border-border px-3 py-2 text-left text-sm"
                  onClick={() => onPlay(item)}
                  disabled={pending}
                >
                  {item.title}
                </button>
              </li>
            ))}
          </ul>
        </section>
      ) : null}
    </div>
  );
}
