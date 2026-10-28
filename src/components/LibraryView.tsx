import { useCallback, useEffect, useMemo, useState } from "react";
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
  onInspect: (inspection: Inspection) => void;
  onApply: () => void;
  applyPending: boolean;
}

const filters = ["all", "image", "video", "scene", "package", "directory"] as const;

export function LibraryView({
  selectedPath,
  initialPath = "",
  onInspect,
  onApply,
  applyPending,
}: LibraryViewProps) {
  const [listing, setListing] = useState<LibraryListing | null>(null);
  const [path, setPath] = useState(initialPath);
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<(typeof filters)[number]>("all");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [thumbs, setThumbs] = useState<Record<string, string>>({});

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
      image: entries.filter((entry) => entry.kind === "image").length,
      video: entries.filter((entry) => entry.kind === "video").length,
      scene: entries.filter((entry) => entry.kind === "scene").length,
      package: entries.filter((entry) => entry.kind === "package").length,
      directory: entries.filter((entry) => entry.kind === "directory").length,
    };
  }, [listing]);

  const visible = (listing?.entries ?? []).filter((entry) => {
    if (filter !== "all" && entry.kind !== filter) return false;
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
                  onOpen={() => void openPath(entry.path)}
                  onApply={onApply}
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
  onOpen,
  onApply,
  applyPending,
}: {
  entry: LibraryEntry;
  preview: string | null;
  selected: boolean;
  onOpen: () => void;
  onApply: () => void;
  applyPending: boolean;
}) {
  return (
    <li>
      <article
        className={
          selected
            ? "overflow-hidden rounded-xl border-2 border-primary bg-card shadow-md"
            : "overflow-hidden rounded-xl border border-border bg-card shadow-sm"
        }
      >
        <button type="button" onClick={onOpen} className="block w-full text-left">
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
        {selected && entry.kind !== "directory" ? (
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
