import { useCallback, useEffect, useRef, useState } from "react";
import { errorText, fetchTelemetry, setPointer } from "./api";
import { ControlBar } from "./components/ControlBar";
import { DisplaySelector } from "./components/DisplaySelector";
import { ImportLink } from "./components/ImportLink";
import { LibraryView } from "./components/LibraryView";
import { PreviewCard } from "./components/PreviewCard";
import { SettingsPanel } from "./components/SettingsPanel";
import { useMonitors } from "./hooks/useMonitors";
import { useWallpaper } from "./hooks/useWallpaper";
import { PlaylistPanel } from "./components/PlaylistPanel";
import {
  loadSettings,
  nextPlaylistIndex,
  rememberWallpaper,
  saveSettings,
  toggleSaved,
  type ManagerSettings,
  type SavedWallpaper,
} from "./settings";
import type { Inspection, MonitorTarget, WallpaperConfig } from "./types";
import { sourceFor, targetLabel } from "./types";

type Page = "library" | "playlist" | "displays" | "import" | "settings";

function parseOptionalFps(value: string, label: string): number | null {
  const trimmed = value.trim();
  if (trimmed === "") return null;
  const parsed = Number(trimmed);
  if (!Number.isInteger(parsed) || parsed < 1 || parsed > 1000) {
    throw new Error(`${label} must be an integer from 1 to 1000, or empty.`);
  }
  return parsed;
}

const pages: { id: Page; label: string }[] = [
  { id: "library", label: "Library" },
  { id: "playlist", label: "Playlist" },
  { id: "displays", label: "Displays" },
  { id: "import", label: "Import" },
  { id: "settings", label: "Settings" },
];

function App() {
  const monitors = useMonitors();
  const wallpaper = useWallpaper();
  const [page, setPage] = useState<Page>("library");
  const [settings, setSettings] = useState<ManagerSettings>(() => loadSettings());
  const [scope, setScope] = useState<MonitorTarget>({ kind: "all" });
  const [inspection, setInspection] = useState<Inspection | null>(null);
  const [formError, setFormError] = useState<string | null>(null);
  const pointerAt = useRef(0);
  const [telemetry, setTelemetry] = useState("");
  const playlistIndex = useRef(0);
  const settingsRef = useRef(settings);
  settingsRef.current = settings;

  useEffect(() => {
    saveSettings(settings);
  }, [settings]);

  useEffect(() => {
    const root = document.documentElement;
    if (settings.theme === "dark") {
      root.classList.add("dark");
      return;
    }
    if (settings.theme === "light") {
      root.classList.remove("dark");
      return;
    }
    const query = window.matchMedia("(prefers-color-scheme: dark)");
    const apply = () => root.classList.toggle("dark", query.matches);
    apply();
    query.addEventListener("change", apply);
    return () => query.removeEventListener("change", apply);
  }, [settings.theme]);

  useEffect(() => {
    let stop = false;
    const tick = () => {
      fetchTelemetry()
        .then((row) => {
          if (stop) return;
          const cpu = Math.round(row.cpu * 100);
          const ram = Math.round(row.ram_fraction * 100);
          const power = row.on_battery ? "Battery" : row.charging ? "Charging" : "AC";
          const focus = row.fullscreen ? " · Fullscreen" : "";
          setTelemetry(`CPU ${cpu}% · RAM ${ram}% · ${power}${focus}`);
        })
        .catch(() => {
          if (!stop) setTelemetry("");
        });
    };
    tick();
    const id = window.setInterval(tick, 1000);
    return () => {
      stop = true;
      window.clearInterval(id);
    };
  }, []);

  const onInspect = useCallback((next: Inspection) => {
    setInspection(next);
    setFormError(null);
  }, []);

  const screenCount = monitors.monitors.length;
  const status = monitors.error
    ? monitors.error
    : monitors.running
      ? screenCount === 1
        ? "Daemon running · 1 display"
        : `Daemon running · ${screenCount} displays`
      : "Daemon offline";

  async function runControl(action: () => Promise<void>) {
    try {
      await action();
      await monitors.refresh();
    } catch {
      // useWallpaper keeps the message.
    }
  }

  function savedFromInspection(item: { path: string; kind: string; title: string }): SavedWallpaper {
    return { path: item.path, kind: item.kind, title: item.title || item.path };
  }

  async function applySaved(item: SavedWallpaper, active = settingsRef.current) {
    const source = sourceFor(item.kind, item.path);
    if (!source) {
      setFormError("That wallpaper type cannot be applied.");
      return;
    }
    let fps: number | null;
    let battery: number | null;
    try {
      fps = parseOptionalFps(active.fpsCap, "FPS cap");
      battery = parseOptionalFps(active.batteryFps, "Battery FPS");
    } catch (err) {
      setFormError(errorText(err));
      return;
    }
    setFormError(null);
    const config: WallpaperConfig = {
      id: `local:${item.path}`,
      source,
      target: scope,
      playback: { rate: active.rate, looping: active.looping, muted: active.muted },
      audio_reactive: active.audioReactive,
      volume: active.volume,
      fps_cap: fps,
      scale: active.scale,
      pause_on_fullscreen: active.pauseOnFullscreen,
      pause_on_game: active.pauseOnGame,
      battery_fps: battery,
    };
    setSettings((current) => ({
      ...current,
      recent: rememberWallpaper(current.recent, item, 12),
    }));
    await runControl(() => wallpaper.apply(scope, config));
  }

  async function onApply() {
    if (!inspection) {
      setFormError("Choose a wallpaper first.");
      return;
    }
    if (inspection.kind === "directory") {
      setFormError("Choose a package, scene folder, image, or video.");
      return;
    }
    await applyOne(savedFromInspection(inspection));
  }

  async function applyOne(item: SavedWallpaper) {
    setSettings((current) => ({ ...current, playlistEnabled: false }));
    await applySaved(item);
  }

  function playNext() {
    const active = settingsRef.current;
    if (active.playlist.length === 0) return;
    const index = nextPlaylistIndex(
      active.playlist.length,
      playlistIndex.current,
      active.shufflePlaylist,
    );
    playlistIndex.current = index;
    const item = active.playlist[index];
    if (item) void applySaved(item, active);
  }

  useEffect(() => {
    if (!settings.playlistEnabled || settings.playlist.length === 0) return;
    const seconds = Math.max(5, settings.playlistSeconds || settings.playlistMinutes * 60);
    const timer = window.setInterval(() => playNext(), seconds * 1000);
    return () => window.clearInterval(timer);
  }, [settings.playlistEnabled, settings.playlistSeconds, settings.playlistMinutes, settings.playlist.length, settings.shufflePlaylist]);

  useEffect(() => {
    function onKey(event: KeyboardEvent) {
      const target = event.target;
      if (target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement || target instanceof HTMLSelectElement) {
        return;
      }
      if ((event.metaKey || event.ctrlKey) && event.key === "Enter") {
        event.preventDefault();
        void onApply();
      }
      if ((event.metaKey || event.ctrlKey) && event.key === "ArrowRight") {
        event.preventDefault();
        playNext();
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  const applyBlocked = wallpaper.pending || !inspection || inspection.kind === "directory";

  return (
    <div
      className="flex h-full bg-background text-foreground"
      onPointerMove={(event) => {
        const now = performance.now();
        if (now - pointerAt.current < 50) return;
        pointerAt.current = now;
        const bounds = event.currentTarget.getBoundingClientRect();
        void setPointer(event.clientX - bounds.left, event.clientY - bounds.top).catch(() => {
          // The daemon reports the failure on the next status poll.
        });
      }}
    >
      <aside className="flex w-56 shrink-0 flex-col border-r border-sidebar-border bg-sidebar text-sidebar-foreground">
        <div className="flex items-center gap-3 px-5 py-5">
          <img src="/logo.svg" alt="" width={36} height={36} className="size-9 rounded-lg" />
          <div>
            <p className="text-lg font-semibold tracking-tight">Iris</p>
            <p className="text-xs text-muted-foreground">Live wallpaper</p>
          </div>
        </div>
        <nav className="flex flex-col gap-1 px-3">
          {pages.map((item) => (
            <button
              key={item.id}
              type="button"
              className={
                page === item.id
                  ? "rounded-md bg-sidebar-accent px-3 py-2 text-left text-sm font-medium text-sidebar-accent-foreground"
                  : "rounded-md px-3 py-2 text-left text-sm text-sidebar-foreground"
              }
              onClick={() => setPage(item.id)}
            >
              {item.label}
            </button>
          ))}
        </nav>
        <div className="mt-auto space-y-2 px-5 py-4 text-xs text-muted-foreground">
          <p className={monitors.error ? "text-destructive" : ""}>{status}</p>
          <button
            type="button"
            className="text-foreground"
            onClick={() => void monitors.reconnect()}
            disabled={monitors.reconnecting}
          >
            {monitors.reconnecting ? "Connecting…" : "Reconnect daemon"}
          </button>
        </div>
      </aside>

      <div className="flex min-w-0 flex-1 flex-col">
        <header className="flex items-center justify-between border-b border-border px-6 py-3">
          <div>
            <h1 className="text-sm font-medium">{pages.find((item) => item.id === page)?.label}</h1>
            {telemetry ? <p className="text-xs text-muted-foreground">{telemetry}</p> : null}
          </div>
          <button
            type="button"
            disabled={applyBlocked}
            onClick={() => void onApply()}
            className="rounded-md bg-primary px-4 py-2 text-sm font-medium text-primary-foreground disabled:opacity-50"
          >
            {wallpaper.pending ? "Applying…" : `Apply to ${targetLabel(scope)}`}
          </button>
        </header>

        <main className="min-h-0 flex-1 overflow-auto p-6">
          {page === "library" ? (
            <div className="grid items-start gap-6 xl:grid-cols-[minmax(0,1fr)_340px]">
              <LibraryView
                selectedPath={inspection?.path ?? null}
                initialPath={settings.libraryPath}
                favoritePaths={settings.favorites.map((item) => item.path)}
                onInspect={onInspect}
                onApplyEntry={(entry) => {
                  if (entry.kind === "directory") return;
                  void applyOne({ path: entry.path, kind: entry.kind, title: entry.name });
                }}
                onToggleFavorite={(entry) => {
                  setSettings((current) => ({
                    ...current,
                    favorites: toggleSaved(current.favorites, {
                      path: entry.path,
                      kind: entry.kind,
                      title: entry.name,
                    }),
                  }));
                }}
                onStartSlideshow={(items, everySeconds) => {
                  const playlist = items.map((entry) => ({
                    path: entry.path,
                    kind: entry.kind,
                    title: entry.name,
                  }));
                  playlistIndex.current = 0;
                  setSettings((current) => ({
                    ...current,
                    playlist,
                    playlistEnabled: true,
                    playlistSeconds: everySeconds,
                    playlistMinutes: Math.max(1, Math.round(everySeconds / 60)),
                  }));
                  const first = playlist[0];
                  if (first) void applySaved(first);
                  setFormError(null);
                }}
                applyPending={wallpaper.pending}
              />
              <aside className="sticky top-0 space-y-3">
                <PreviewCard inspection={inspection} />
                <div className="rounded-xl border border-border bg-card p-4 text-sm shadow-sm">
                  <p className="font-medium">Apply uses</p>
                  <ul className="mt-2 space-y-1 text-muted-foreground">
                    <li>Target {targetLabel(scope)}</li>
                    <li>Scale {settings.scale}</li>
                    <li>
                      {settings.muted ? "Muted" : `Volume ${Math.round(settings.volume * 100)}%`}
                    </li>
                    <li>{settings.looping ? "Looping" : "Play once"}</li>
                    <li>
                      {settings.pauseOnFullscreen ? "Pauses in fullscreen" : "Keeps playing in fullscreen"}
                    </li>
                  </ul>
                  <button
                    type="button"
                    disabled={applyBlocked}
                    onClick={() => void onApply()}
                    className="mt-4 w-full rounded-md bg-primary py-2 text-sm font-medium text-primary-foreground disabled:opacity-50"
                  >
                    {wallpaper.pending ? "Applying…" : "Set wallpaper"}
                  </button>
                  {formError ? <p className="mt-2 text-sm text-destructive">{formError}</p> : null}
                </div>
              </aside>
            </div>
          ) : null}
          {page === "playlist" ? (
            <PlaylistPanel
              settings={settings}
              current={
                inspection && inspection.kind !== "directory"
                  ? { path: inspection.path, kind: inspection.kind, title: inspection.title || inspection.path }
                  : null
              }
              onChange={setSettings}
              pending={wallpaper.pending}
              onPlay={(item) => void applySaved(item)}
              onNext={playNext}
            />
          ) : null}
          {page === "displays" ? (
            <div className="space-y-4">
              <header>
                <h2 className="text-2xl font-semibold tracking-tight">Displays</h2>
                <p className="mt-1 text-sm text-muted-foreground">
                  Choose where the next wallpaper is applied. All displays is the default.
                </p>
              </header>
              <DisplaySelector
                monitors={monitors.monitors}
                wallpapers={monitors.wallpapers}
                selected={scope}
                onSelect={setScope}
              />
            </div>
          ) : null}
          {page === "import" ? (
            <ImportLink
              onImported={(next) => {
                onInspect(next);
                setPage("library");
              }}
            />
          ) : null}
          {page === "settings" ? (
            <SettingsPanel settings={settings} onChange={setSettings} />
          ) : null}
        </main>

        <ControlBar
          scopeLabel={targetLabel(scope)}
          scopeIsAll={scope.kind === "all"}
          pending={wallpaper.pending}
          error={wallpaper.error}
          onAll={() => setScope({ kind: "all" })}
          onPause={() => void runControl(() => wallpaper.pause(scope, true))}
          onResume={() => void runControl(() => wallpaper.pause(scope, false))}
          onClear={() => void runControl(() => wallpaper.clear(scope))}
        />
      </div>
    </div>
  );
}

export default App;
