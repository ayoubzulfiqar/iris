import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  defaultLibraryPath,
  errorText,
  inspectPackage,
  libraryThumbnails,
  listLibrary,
} from "../api";
import type { Inspection, LibraryEntry, LibraryListing } from "../types";
import { WallpaperDropzone } from "./WallpaperDropzone";

interface LibraryViewProps {
  selectedPath: string | null;
  initialPath?: string;
  favoritePaths: string[];
  onInspect: (inspection: Inspection) => void;
  onApplyEntry: (entry: LibraryEntry) => void;
  onToggleFavorite: (entry: LibraryEntry) => void;
  onStartSlideshow: (items: LibraryEntry[], everySeconds: number) => void;
  applyPending: boolean;
}

const filters = ["all", "favorites", "image", "video", "scene", "package", "directory"] as const;

export function LibraryView({
  selectedPath,
  initialPath = "",
  favoritePaths,
  onInspect,
  onApplyEntry,
  onToggleFavorite,
  onStartSlideshow,
  applyPending,
}: LibraryViewProps) {
  const [listing, setListing] = useState<LibraryListing | null>(null);
  const [path, setPath] = useState(initialPath);
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<(typeof filters)[number]>("all");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [thumbs, setThumbs] = useState<Record<string, string>>({});
  const [picked, setPicked] = useState<string[]>([]);
  const [everyAmount, setEveryAmount] = useState(30);
  const [everyUnit, setEveryUnit] = useState<"minutes" | "seconds">("minutes");
  const lastPicked = useRef<string | null>(null);

  const openPath = useCallback(
    async (next: string) => {
      const trimmed = next.trim();
      if (!trimmed) {
        setError("Enter a path.");
        return;
      }
      setBusy(true);
      setError(null);
      try {
        const inspection = await inspectPackage(trimmed);
        if (inspection.kind === "directory") {
          const nextListing = await listLibrary(trimmed);
          setListing(nextListing);
          setPath(nextListing.path);
          return;
        }
        onInspect(inspection);
      } catch (err) {
        setError(errorText(err));
      } finally {
        setBusy(false);
      }
    },
    [onInspect],
  );

  useEffect(() => {
    let stopped = false;
    const open = openPath;
    void (async () => {
      const start = initialPath.trim() || (await defaultLibraryPath().catch(() => ""));
      if (!stopped && start) await open(start);
    })();
    return () => {
      stopped = true;
    };
  }, [initialPath, openPath]);

  useEffect(() => {
    if (!listing) return;
    const paths = listing.entries
      .filter((entry) => entry.kind === "image")
      .slice(0, 48)
      .map((entry) => entry.path);
    if (paths.length === 0) {
      setThumbs({});
      return;
    }
    let stopped = false;
    void libraryThumbnails(paths)
      .then((rows) => {
        if (stopped) return;
        const next: Record<string, string> = {};
        for (const row of rows) {
          if (row.preview_data_url) next[row.path] = row.preview_data_url;
        }
        setThumbs(next);
      })
      .catch(() => {
        if (!stopped) setThumbs({});
      });
    return () => {
      stopped = true;
    };
  }, [listing]);

  const onPaths = useCallback(
    (paths: string[]) => {
      const first = paths[0];
      if (first) void openPath(first);
    },
    [openPath],
  );

  const counts = useMemo(() => {
    const entries = listing?.entries ?? [];
    return {
      all: entries.length,
      favorites: entries.filter((entry) => favoritePaths.includes(entry.path)).length,
      image: entries.filter((entry) => entry.kind === "image").length,
      video: entries.filter((entry) => entry.kind === "video").length,
      scene: entries.filter((entry) => entry.kind === "scene").length,
      package: entries.filter((entry) => entry.kind === "package").length,
      directory: entries.filter((entry) => entry.kind === "directory").length,
    };
  }, [listing, favoritePaths]);

  const images = (listing?.entries ?? []).filter((entry) => entry.kind === "image");
  const allImagesPicked = images.length > 0 && images.every((entry) => picked.includes(entry.path));

  function togglePick(path: string) {
    setPicked((current) =>
      current.includes(path) ? current.filter((item) => item !== path) : [...current, path],
    );
    lastPicked.current = path;
  }

  function pickImage(path: string, shiftKey: boolean) {
    const order = visible.filter((entry) => entry.kind === "image").map((entry) => entry.path);
    const anchor = lastPicked.current;
    if (shiftKey && anchor) {
      const start = order.indexOf(anchor);
      const end = order.indexOf(path);
      if (start >= 0 && end >= 0) {
        const [from, to] = start < end ? [start, end] : [end, start];
        const range = order.slice(from, to + 1);
        setPicked((current) => Array.from(new Set([...current, ...range])));
        lastPicked.current = path;
        return;
      }
    }
    togglePick(path);
  }

  function toggleAllImages() {
    setPicked(allImagesPicked ? [] : images.map((entry) => entry.path));
  }

  function startSlideshow() {
    const chosen = images.filter((entry) => picked.includes(entry.path));
    const amount = Number.isFinite(everyAmount) && everyAmount > 0 ? everyAmount : 1;
    const seconds = Math.max(5, Math.round(everyUnit === "minutes" ? amount * 60 : amount));
    onStartSlideshow(chosen, seconds);
  }

  const visible = (listing?.entries ?? []).filter((entry) => {
    if (filter === "favorites" && !favoritePaths.includes(entry.path)) return false;
    if (filter !== "all" && filter !== "favorites" && entry.kind !== filter) return false;
    const needle = query.trim().toLowerCase();
    if (!needle) return true;
    return entry.name.toLowerCase().includes(needle);
  });

  return (
    <div className="space-y-4">
      <WallpaperDropzone onPaths={onPaths} />
      <form
        className="flex gap-2"
        onSubmit={(event) => {
          event.preventDefault();
          void openPath(path);
        }}
      >
        <input
          value={path}
          onChange={(event) => setPath(event.currentTarget.value)}
          placeholder="/home/you/Wallpaper"
          aria-label="Wallpaper folder or file"
          className="min-w-0 flex-1 rounded-lg border border-border bg-input px-3 py-2 text-sm shadow-sm"
        />
        <button
          type="submit"
          disabled={busy}
          className="rounded-lg bg-secondary px-3 py-2 text-sm font-medium text-secondary-foreground disabled:opacity-50"
        >
          {busy ? "Opening…" : "Open"}
        </button>
      </form>
      {error ? <p className="text-sm text-destructive">{error}</p> : null}
      {listing ? (
        <div className="space-y-3">
          <div className="flex flex-wrap items-center justify-between gap-3">
            <div className="min-w-0">
              <p className="truncate font-mono text-xs text-muted-foreground">{listing.path}</p>
              <p className="text-sm">
                {counts.all} items
                {busy ? " · loading" : ""}
              </p>
            </div>
            <div className="flex flex-wrap items-center gap-2">
              {listing.parent ? (
                <button
                  type="button"
                  className="rounded-md border border-border px-2 py-1 text-xs"
                  onClick={() => void openPath(listing.parent ?? "")}
                >
                  Up
                </button>
              ) : null}
              <input
                value={query}
                onChange={(event) => setQuery(event.currentTarget.value)}
                placeholder="Search"
                aria-label="Filter library"
                className="w-40 rounded-md border border-border bg-input px-2 py-1 text-sm"
              />
            </div>
          </div>
          {images.length > 0 ? (
            <section className="space-y-3 rounded-xl border border-border bg-card p-4 shadow-sm">
              <div className="flex flex-wrap items-center justify-between gap-3">
                <div>
                  <h2 className="text-sm font-semibold">Image slideshow</h2>
                  <p className="text-xs text-muted-foreground">
                    Click images to select them. Shift-click selects the range between two images.
                    Only the ones you pick are used.
                  </p>
                </div>
                <div className="flex flex-wrap gap-2">
                  <button type="button" className="rounded-md border border-border px-3 py-1.5 text-xs" onClick={toggleAllImages}>
                    {allImagesPicked ? "Clear all" : `Select all ${images.length}`}
                  </button>
                  <button
                    type="button"
                    className="rounded-md border border-border px-3 py-1.5 text-xs"
                    onClick={() => setPicked([])}
                    disabled={picked.length === 0}
                  >
                    Clear selection
                  </button>
                </div>
              </div>
              <div className="flex flex-wrap items-end gap-2">
                <label className="text-sm">
                  Change every
                  <input
                    className="mt-1 block w-24 rounded-md border border-border bg-input px-3 py-2"
                    inputMode="numeric"
                    min={1}
                    value={everyAmount}
                    aria-label="Slideshow interval"
                    onChange={(event) => setEveryAmount(Number(event.currentTarget.value))}
                  />
                </label>
                <select
                  className="rounded-md border border-border bg-input px-3 py-2 text-sm"
                  value={everyUnit}
                  aria-label="Slideshow interval unit"
                  onChange={(event) => setEveryUnit(event.currentTarget.value as "minutes" | "seconds")}
                >
                  <option value="minutes">minutes</option>
                  <option value="seconds">seconds</option>
                </select>
                <button
                  type="button"
                  className="rounded-md bg-primary px-3 py-2 text-sm font-medium text-primary-foreground disabled:opacity-50"
                  disabled={picked.length === 0 || applyPending}
                  onClick={startSlideshow}
                >
                  {picked.length === 0 ? "Select images first" : `Rotate ${picked.length} images`}
                </button>
                <span className="text-xs text-muted-foreground">{picked.length} selected</span>
              </div>
            </section>
          ) : null}
          <div className="flex flex-wrap gap-2">
            {filters.map((item) => (
              <button
                key={item}
                type="button"
                onClick={() => setFilter(item)}
                className={
                  filter === item
                    ? "rounded-full bg-primary px-3 py-1 text-xs text-primary-foreground"
                    : "rounded-full bg-secondary px-3 py-1 text-xs text-secondary-foreground"
                }
              >
                {item} {counts[item]}
              </button>
            ))}
          </div>
          {visible.length === 0 ? (
            <p className="rounded-xl border border-dashed border-border p-8 text-center text-sm text-muted-foreground">
              Nothing in this folder matches the filter.
            </p>
          ) : (
            <ul className="grid grid-cols-2 gap-3 xl:grid-cols-3">
              {visible.map((entry) => (
                <GalleryCard
                  key={entry.path}
                  entry={entry}
                  preview={thumbs[entry.path] ?? null}
                  selected={entry.path === selectedPath}
                  favorite={favoritePaths.includes(entry.path)}
                  picked={picked.includes(entry.path)}
                  onTogglePick={(shiftKey) => pickImage(entry.path, shiftKey)}
                  onOpen={() => void openPath(entry.path)}
                  onApply={() => onApplyEntry(entry)}
                  onToggleFavorite={() => onToggleFavorite(entry)}
                  applyPending={applyPending}
                />
              ))}
            </ul>
          )}
        </div>
      ) : (
        <p className="text-sm text-muted-foreground">Looking for a wallpaper folder…</p>
      )}
    </div>
  );
}

function GalleryCard({
  entry,
  preview,
  selected,
  favorite,
  picked,
  onTogglePick,
  onOpen,
  onApply,
  onToggleFavorite,
  applyPending,
}: {
  entry: LibraryEntry;
  preview: string | null;
  selected: boolean;
  favorite: boolean;
  picked: boolean;
  onTogglePick: (shiftKey: boolean) => void;
  onOpen: () => void;
  onApply: () => void;
  onToggleFavorite: () => void;
  applyPending: boolean;
}) {
  return (
    <li>
      <article
        className={
          picked
            ? "overflow-hidden rounded-xl border-2 border-primary bg-card shadow-md"
            : selected
              ? "overflow-hidden rounded-xl border border-primary/50 bg-card shadow-sm"
              : "overflow-hidden rounded-xl border border-border bg-card shadow-sm"
        }
      >
        <div className="relative">
          <button
            type="button"
            onClick={onOpen}
            className="block w-full text-left"
          >
            <div className="relative aspect-video bg-muted">
              {preview ? (
                <img src={preview} alt="" className="h-full w-full object-cover" />
              ) : (
                <KindFill kind={entry.kind} />
              )}
              <span className="absolute left-2 top-2 rounded-md bg-black/65 px-2 py-0.5 text-[10px] font-medium uppercase tracking-wide text-white">
                {entry.kind}
              </span>
            </div>
            <p className="truncate px-3 py-2 text-sm font-medium">{entry.name}</p>
          </button>
          {entry.kind === "image" ? (
            <button
              type="button"
              aria-pressed={picked}
              aria-label={picked ? `Deselect ${entry.name}` : `Select ${entry.name}`}
              className={
                picked
                  ? "absolute bottom-2 left-2 rounded-md bg-primary px-2 py-1 text-[10px] font-medium text-primary-foreground"
                  : "absolute bottom-2 left-2 rounded-md bg-black/75 px-2 py-1 text-[10px] font-medium text-white"
              }
              onClick={(event) => onTogglePick(event.shiftKey)}
            >
              {picked ? "Selected" : "Select"}
            </button>
          ) : null}
          {entry.kind === "image" ? (
            <button
              type="button"
              className="absolute bottom-2 right-2 rounded-md bg-black/75 px-2 py-1 text-[10px] text-white"
              onClick={onOpen}
            >
              Preview
            </button>
          ) : null}
          {entry.kind !== "directory" ? (
            <button
              type="button"
              aria-label={favorite ? "Remove favorite" : "Add favorite"}
              aria-pressed={favorite}
              className="absolute right-2 top-2 rounded-md bg-black/65 px-2 py-0.5 text-xs text-white"
              onClick={onToggleFavorite}
            >
              {favorite ? "Starred" : "Star"}
            </button>
          ) : null}
        </div>
        {entry.kind !== "directory" ? (
          <div className="px-3 pb-3">
            <button
              type="button"
              disabled={applyPending}
              onClick={onApply}
              className="w-full rounded-md bg-primary py-1.5 text-sm font-medium text-primary-foreground disabled:opacity-50"
            >
              {applyPending ? "Applying…" : "Set wallpaper"}
            </button>
          </div>
        ) : null}
      </article>
    </li>
  );
}

function KindFill({ kind }: { kind: string }) {
  const label =
    kind === "video" ? "Video" : kind === "scene" ? "Scene" : kind === "package" ? "Package" : "Folder";
  return (
    <div className="flex h-full items-end bg-gradient-to-br from-primary/80 to-chart-3 p-3">
      <span className="text-sm font-semibold text-white">{label}</span>
    </div>
  );
}
