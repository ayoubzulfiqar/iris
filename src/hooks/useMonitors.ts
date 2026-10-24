import { useCallback, useEffect, useState } from "react";
import { ensureDaemon, errorText, fetchMonitors, fetchStatus } from "../api";
import type { MonitorInfo, WallpaperStatus } from "../types";

export function useMonitors() {
  const [monitors, setMonitors] = useState<MonitorInfo[]>([]);
  const [wallpapers, setWallpapers] = useState<WallpaperStatus[]>([]);
  const [running, setRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [reconnecting, setReconnecting] = useState(false);

  const refresh = useCallback(async () => {
    try {
      const [nextMonitors, status] = await Promise.all([
        fetchMonitors(),
        fetchStatus(),
      ]);
      setMonitors(nextMonitors);
      setWallpapers(status.wallpapers);
      setRunning(status.running);
      setError(null);
    } catch (err) {
      setRunning(false);
      setError(errorText(err));
    }
  }, []);

  const reconnect = useCallback(async () => {
    setReconnecting(true);
    try {
      await ensureDaemon();
      await refresh();
    } catch (err) {
      setError(errorText(err));
    } finally {
      setReconnecting(false);
    }
  }, [refresh]);

  useEffect(() => {
    let stopped = false;
    let timer = 0;
    void (async () => {
      try {
        await ensureDaemon();
      } catch (err) {
        if (!stopped) setError(errorText(err));
      }
      if (stopped) return;
      await refresh();
      if (stopped) return;
      timer = window.setInterval(() => {
        void refresh();
      }, 2000);
    })();
    return () => {
      stopped = true;
      window.clearInterval(timer);
    };
  }, [refresh]);

  return {
    monitors,
    wallpapers,
    running,
    error,
    reconnecting,
    refresh,
    reconnect,
  };
}
