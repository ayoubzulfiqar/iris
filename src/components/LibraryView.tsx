import { useCallback, useState } from "react";
import { errorText, inspectPackage, listLibrary } from "../api";
import type { Inspection, LibraryListing } from "../types";
import { WallpaperDropzone } from "./WallpaperDropzone";

interface LibraryViewProps {
  selectedPath: string | null;
  onInspect: (inspection: Inspection) => void;
}

export function LibraryView({ selectedPath, onInspect }: LibraryViewProps) {
  const [listing, setListing] = useState<LibraryListing | null>(null);
  const [path, setPath] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

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

  const onPaths = useCallback(
    (paths: string[]) => {
      const first = paths[0];
      if (first) void openPath(first);
    },
    [openPath],
  );

  return (
    <div className="library">
      <WallpaperDropzone onPaths={onPaths} />
      <form
        className="path-form"
        onSubmit={(event) => {
          event.preventDefault();
          void openPath(path);
        }}
      >
        <input
          value={path}
          onChange={(event) => setPath(event.currentTarget.value)}
          placeholder="/path/to/wallpapers"
          aria-label="Wallpaper folder or file"
        />
        <button type="submit" disabled={busy}>
          Open
        </button>
      </form>
      {error ? <p className="inline-error">{error}</p> : null}
      {listing ? (
        <div className="listing">
          <div className="listing-head">
            <span className="path">{listing.path}</span>
            {listing.parent ? (
              <button type="button" onClick={() => void openPath(listing.parent ?? "")}>
                Up
              </button>
            ) : null}
          </div>
          {listing.entries.length === 0 ? (
            <p className="empty">This folder has no wallpapers.</p>
          ) : (
            <ul>
              {listing.entries.map((entry) => (
                <li key={entry.path}>
                  <button
                    type="button"
                    className={entry.path === selectedPath ? "row active" : "row"}
                    onClick={() => void openPath(entry.path)}
                  >
                    <span className="kind">{entry.kind}</span>
                    <span>{entry.name}</span>
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>
      ) : (
        <p className="empty">Open a folder, or drop a file anywhere in the window.</p>
      )}
    </div>
  );
}
