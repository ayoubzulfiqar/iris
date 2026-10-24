import { useCallback, useState } from "react";
import { applyWallpaper, clearWallpaper, errorText, setPaused } from "../api";
import type { MonitorTarget, WallpaperConfig } from "../types";

export function useWallpaper() {
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const run = useCallback(async (action: () => Promise<void>) => {
    setPending(true);
    setError(null);
    try {
      await action();
    } catch (err) {
      const message = errorText(err);
      setError(message);
      throw new Error(message);
    } finally {
      setPending(false);
    }
  }, []);

  const apply = useCallback(
    (target: MonitorTarget, config: WallpaperConfig) =>
      run(() => applyWallpaper(target, config)),
    [run],
  );

  const clear = useCallback(
    (target: MonitorTarget) => run(() => clearWallpaper(target)),
    [run],
  );

  const pause = useCallback(
    (target: MonitorTarget, paused: boolean) => run(() => setPaused(target, paused)),
    [run],
  );

  return { pending, error, apply, clear, pause };
}
