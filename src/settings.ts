import type { ScaleMode } from "./types";

export type ThemeMode = "light" | "dark" | "system";

export interface SavedWallpaper {
  path: string;
  kind: string;
  title: string;
}

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
  favorites: SavedWallpaper[];
  recent: SavedWallpaper[];
  playlist: SavedWallpaper[];
  playlistEnabled: boolean;
  playlistMinutes: number;
  playlistSeconds: number;
  shufflePlaylist: boolean;
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
  favorites: [],
  recent: [],
  playlist: [],
  playlistEnabled: false,
  playlistMinutes: 30,
  playlistSeconds: 1800,
  shufflePlaylist: false,
};

export function rememberWallpaper(list: SavedWallpaper[], item: SavedWallpaper, limit: number): SavedWallpaper[] {
  const next = [item, ...list.filter((entry) => entry.path !== item.path)];
  return next.slice(0, limit);
}

export function toggleSaved(list: SavedWallpaper[], item: SavedWallpaper): SavedWallpaper[] {
  if (list.some((entry) => entry.path === item.path)) {
    return list.filter((entry) => entry.path !== item.path);
  }
  return [...list, item];
}

export function nextPlaylistIndex(length: number, current: number, shuffle: boolean): number {
  if (length <= 1) return 0;
  if (!shuffle) return (current + 1) % length;
  let pick = current;
  while (pick === current) {
    pick = Math.floor(Math.random() * length);
  }
  return pick;
}

export function loadSettings(): ManagerSettings {
  try {
    const raw = localStorage.getItem(KEY);
    if (!raw) return defaultSettings;
    const parsed = JSON.parse(raw) as Partial<ManagerSettings>;
    const next = { ...defaultSettings, ...parsed };
    next.favorites = Array.isArray(parsed.favorites) ? parsed.favorites : [];
    next.recent = Array.isArray(parsed.recent) ? parsed.recent : [];
    next.playlist = Array.isArray(parsed.playlist) ? parsed.playlist : [];
    if (!Number.isFinite(next.playlistMinutes) || next.playlistMinutes < 1) {
      next.playlistMinutes = defaultSettings.playlistMinutes;
    }
    if (!Number.isFinite(next.playlistSeconds) || next.playlistSeconds < 5) {
      next.playlistSeconds = next.playlistMinutes * 60;
    }
    return next;
  } catch {
    return defaultSettings;
  }
}

export function saveSettings(settings: ManagerSettings) {
  localStorage.setItem(KEY, JSON.stringify(settings));
}
