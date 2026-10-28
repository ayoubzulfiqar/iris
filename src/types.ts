export type MonitorTarget =
  | { kind: "all" }
  | { kind: "primary" }
  | { kind: "index"; index: number }
  | { kind: "connector"; name: string };

export type WallpaperSource =
  | { kind: "package"; path: string }
  | { kind: "video"; path: string }
  | { kind: "image"; path: string }
  | { kind: "scene_directory"; path: string };

export type ScaleMode = "fill" | "fit" | "stretch" | "center";

export type RunState = "playing" | "paused" | "failed";

export interface MonitorInfo {
  index: number;
  connector: string;
  name: string;
  width_px: number;
  height_px: number;
  scale: number;
  primary: boolean;
  refresh_millihz: number | null;
}

export interface WallpaperStatus {
  id: string;
  target: MonitorTarget;
  state: RunState;
  message: string | null;
}

export interface WallpaperConfig {
  id: string;
  source: WallpaperSource;
  target: MonitorTarget;
  playback: {
    rate: number;
    looping: boolean;
    muted: boolean;
  };
  audio_reactive: boolean;
  volume: number;
  fps_cap: number | null;
  scale: ScaleMode;
  pause_on_fullscreen: boolean;
  pause_on_game: boolean;
  battery_fps?: number | null;
}

export interface ImportedMedia {
  path: string;
  kind: string;
  bytes: number;
}

export interface DaemonStatus {
  running: boolean;
  wallpapers: WallpaperStatus[];
}

export interface TelemetryReport {
  cpu: number;
  cores: number[];
  ram_used: number;
  ram_total: number;
  ram_fraction: number;
  net_rx: number;
  net_tx: number;
  battery: number;
  on_battery: boolean;
  charging: boolean;
  fullscreen: boolean;
}

export interface Inspection {
  path: string;
  kind: string;
  title: string;
  description: string;
  preview_data_url: string | null;
}

export interface LibraryEntry {
  path: string;
  name: string;
  kind: string;
}

export interface LibraryListing {
  path: string;
  parent: string | null;
  entries: LibraryEntry[];
}

export function targetLabel(target: MonitorTarget): string {
  switch (target.kind) {
    case "all":
      return "All displays";
    case "primary":
      return "Primary";
    case "index":
      return `Display ${target.index}`;
    case "connector":
      return target.name;
  }
}

export function sourceFor(kind: string, path: string): WallpaperSource | null {
  if (kind === "package") return { kind: "package", path };
  if (kind === "scene") return { kind: "scene_directory", path };
  if (kind === "image") return { kind: "image", path };
  if (kind === "video") return { kind: "video", path };
  return null;
}

export function wallpaperFor(
  connector: string,
  wallpapers: WallpaperStatus[],
): WallpaperStatus | undefined {
  return wallpapers.find(
    (row) => row.target.kind === "connector" && row.target.name === connector,
  );
}
