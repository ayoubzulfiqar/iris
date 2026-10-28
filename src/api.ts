import { invoke } from "@tauri-apps/api/core";
import type {
  DaemonStatus,
  Inspection,
  LibraryListing,
  MonitorInfo,
  MonitorTarget,
  TelemetryReport,
  WallpaperConfig,
  ImportedMedia,
} from "./types";

export function errorText(err: unknown): string {
  if (typeof err === "string") return err;
  if (err instanceof Error && err.message) return err.message;
  return "Request failed";
}

export function fetchMonitors(): Promise<MonitorInfo[]> {
  return invoke<MonitorInfo[]>("get_system_monitors");
}

export function fetchTelemetry(): Promise<TelemetryReport> {
  return invoke<TelemetryReport>("get_telemetry_command");
}

export function fetchStatus(): Promise<DaemonStatus> {
  return invoke<DaemonStatus>("get_wallpaper_status");
}

export function applyWallpaper(
  target: MonitorTarget,
  config: WallpaperConfig,
): Promise<void> {
  return invoke("apply_wallpaper_command", { target, config });
}

export function clearWallpaper(target: MonitorTarget): Promise<void> {
  return invoke("clear_wallpaper_command", { target });
}

export function setPaused(target: MonitorTarget, paused: boolean): Promise<void> {
  return invoke("set_paused_command", { target, paused });
}

export function setPointer(x: number, y: number): Promise<void> {
  return invoke("set_pointer_command", { x, y });
}

export function inspectPackage(path: string): Promise<Inspection> {
  return invoke<Inspection>("inspect_wallpaper_package", { path });
}

export function listLibrary(path: string): Promise<LibraryListing> {
  return invoke<LibraryListing>("list_library", { path });
}

export function defaultLibraryPath(): Promise<string> {
  return invoke<string>("default_library_path");
}

export interface LibraryThumb {
  path: string;
  preview_data_url: string | null;
}

export function libraryThumbnails(paths: string[]): Promise<LibraryThumb[]> {
  return invoke<LibraryThumb[]>("library_thumbnails", { paths });
}

export function ensureDaemon(): Promise<void> {
  return invoke("ensure_daemon");
}

export function importMediaUrl(url: string): Promise<ImportedMedia> {
  return invoke<ImportedMedia>("import_media_url", { url });
}
