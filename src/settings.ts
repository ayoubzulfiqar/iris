import type { ScaleMode } from "./types";

export type ThemeMode = "light" | "dark" | "system";

export interface ManagerSettings {
  theme: ThemeMode;
  libraryPath: string;
  scale: ScaleMode;
  volume: number;
  muted: boolean;
  looping: boolean;
  rate: number;
  fpsCap: string;
  batteryFps: string;
  pauseOnFullscreen: boolean;
  pauseOnGame: boolean;
  audioReactive: boolean;
}

const KEY = "iris.settings.v1";

export const defaultSettings: ManagerSettings = {
  theme: "system",
  libraryPath: "",
  scale: "fill",
  volume: 1,
  muted: false,
  looping: true,
  rate: 1,
  fpsCap: "",
  batteryFps: "15",
  pauseOnFullscreen: true,
  pauseOnGame: true,
  audioReactive: false,
};

export function loadSettings(): ManagerSettings {
  try {
    const raw = localStorage.getItem(KEY);
    if (!raw) return defaultSettings;
    const parsed = JSON.parse(raw) as Partial<ManagerSettings>;
    return { ...defaultSettings, ...parsed };
  } catch {
    return defaultSettings;
  }
}

export function saveSettings(settings: ManagerSettings) {
  localStorage.setItem(KEY, JSON.stringify(settings));
}
