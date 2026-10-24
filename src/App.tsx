import { useEffect, useRef, useState } from "react";
import { errorText, fetchTelemetry, setPointer } from "./api";
import { ControlBar } from "./components/ControlBar";
import { DisplaySelector } from "./components/DisplaySelector";
import { LibraryView } from "./components/LibraryView";
import { PreviewCard } from "./components/PreviewCard";
import { useMonitors } from "./hooks/useMonitors";
import { useWallpaper } from "./hooks/useWallpaper";
import type { Inspection, MonitorTarget, ScaleMode, WallpaperConfig } from "./types";
import { sourceFor, targetLabel } from "./types";
import "./App.css";

function parseFps(value: string): number | null {
  const trimmed = value.trim();
  if (trimmed === "") return null;
  const parsed = Number(trimmed);
  if (!Number.isInteger(parsed) || parsed < 1 || parsed > 1000) {
    throw new Error("FPS cap must be an integer from 1 to 1000.");
  }
  return parsed;
}

function App() {
  const monitors = useMonitors();
  const wallpaper = useWallpaper();
  const [scope, setScope] = useState<MonitorTarget>({ kind: "all" });
  const [inspection, setInspection] = useState<Inspection | null>(null);
  const [fpsCap, setFpsCap] = useState("");
  const [volume, setVolume] = useState(1);
  const [scale, setScale] = useState<ScaleMode>("fill");
  const [muted, setMuted] = useState(false);
  const [formError, setFormError] = useState<string | null>(null);
  const pointerAt = useRef(0);
  const [telemetry, setTelemetry] = useState("");

  useEffect(() => {
    let stop = false;
    const tick = () => {
      fetchTelemetry()
        .then((row) => {
          if (stop) return;
          const cpu = Math.round(row.cpu * 100);
          const ram = Math.round(row.ram_fraction * 100);
          const power = row.on_battery ? "battery" : row.charging ? "charging" : "AC";
          const focus = row.fullscreen ? " · fullscreen" : "";
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

  const screenCount = monitors.monitors.length;
  const status = monitors.error
    ? monitors.error
    : monitors.running
      ? screenCount === 1
        ? "Daemon running · 1 screen"
        : `Daemon running · ${screenCount} screens`
      : "Daemon offline";

  async function runControl(action: () => Promise<void>) {
    try {
      await action();
      await monitors.refresh();
    } catch {
      // useWallpaper keeps the message.
    }
  }

  async function onApply() {
    if (!inspection) {
      setFormError("Choose a wallpaper first.");
      return;
    }
    const source = sourceFor(inspection.kind, inspection.path);
    if (!source || inspection.kind === "directory") {
      setFormError("Choose a package, scene folder, image, or video.");
      return;
    }
    let fps: number | null;
    try {
      fps = parseFps(fpsCap);
    } catch (err) {
      setFormError(errorText(err));
      return;
    }
    setFormError(null);
    const config: WallpaperConfig = {
      id: `local:${inspection.path}`,
      source,
      target: scope,
      playback: { rate: 1, looping: true, muted },
      audio_reactive: false,
      volume,
      fps_cap: fps,
      scale,
      pause_on_fullscreen: true,
      pause_on_game: true,
    };
    await runControl(() => wallpaper.apply(scope, config));
  }

  const applyBlocked =
    wallpaper.pending ||
    !inspection ||
    inspection.kind === "directory";

  return (
    <div
      className="app"
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
      <header className="topbar">
        <div>
          <p className="mark">Iris</p>
          <p className={monitors.error ? "status bad" : "status"}>{status}</p>
          {telemetry ? <p className="status telemetry">{telemetry}</p> : null}
        </div>
        <button
          type="button"
          onClick={() => void monitors.reconnect()}
          disabled={monitors.reconnecting}
        >
          {monitors.reconnecting ? "Connecting…" : "Reconnect"}
        </button>
      </header>

      <main className="workspace">
        <section className="panel">
          <h1>Displays</h1>
          <DisplaySelector
            monitors={monitors.monitors}
            wallpapers={monitors.wallpapers}
            selected={scope}
            onSelect={setScope}
          />
        </section>

        <section className="panel">
          <h1>Library</h1>
          <LibraryView
            selectedPath={inspection?.path ?? null}
            onInspect={setInspection}
          />
        </section>

        <section className="panel">
          <h1>Inspector</h1>
          <PreviewCard inspection={inspection} />
          <div className="fields">
            <label>
              FPS cap
              <input
                inputMode="numeric"
                value={fpsCap}
                placeholder="Display rate"
                onChange={(event) => setFpsCap(event.currentTarget.value)}
              />
            </label>
            <label>
              Volume
              <input
                type="range"
                min={0}
                max={1}
                step={0.01}
                value={volume}
                onChange={(event) => setVolume(Number(event.currentTarget.value))}
              />
              <span className="value">{Math.round(volume * 100)}%</span>
            </label>
            <label>
              Scale
              <select
                value={scale}
                onChange={(event) => setScale(event.currentTarget.value as ScaleMode)}
              >
                <option value="fill">Fill</option>
                <option value="fit">Fit</option>
                <option value="stretch">Stretch</option>
                <option value="center">Center</option>
              </select>
            </label>
            <label className="check">
              <input
                type="checkbox"
                checked={muted}
                onChange={(event) => setMuted(event.currentTarget.checked)}
              />
              Muted
            </label>
            <button type="button" className="apply" onClick={() => void onApply()} disabled={applyBlocked}>
              {wallpaper.pending ? "Applying…" : `Apply to ${targetLabel(scope)}`}
            </button>
            {formError ? <p className="inline-error">{formError}</p> : null}
          </div>
        </section>
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
  );
}

export default App;
