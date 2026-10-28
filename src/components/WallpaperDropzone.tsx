import { useEffect, useState } from "react";

interface WallpaperDropzoneProps {
  onPaths: (paths: string[]) => void;
}

export function WallpaperDropzone({ onPaths }: WallpaperDropzoneProps) {
  const [hovering, setHovering] = useState(false);

  useEffect(() => {
    let stopped = false;
    let unlisten: (() => void) | undefined;
    void (async () => {
      try {
        const { getCurrentWebview } = await import("@tauri-apps/api/webview");
        const stop = await getCurrentWebview().onDragDropEvent((event) => {
          if (event.payload.type === "enter") setHovering(true);
          if (event.payload.type === "leave") setHovering(false);
          if (event.payload.type === "drop") {
            setHovering(false);
            onPaths(event.payload.paths);
          }
        });
        if (stopped) stop();
        else unlisten = stop;
      } catch {
        // The Vite browser preview has no Tauri drag-drop channel.
      }
    })();
    return () => {
      stopped = true;
      unlisten?.();
    };
  }, [onPaths]);

  return (
    <div
      className={
        hovering
          ? "rounded-xl border border-primary bg-accent px-4 py-6 text-center shadow-sm"
          : "rounded-xl border border-dashed border-border bg-card px-4 py-6 text-center"
      }
    >
      <p className="text-sm font-medium">Drop a wallpaper</p>
      <p className="mt-1 text-sm text-muted-foreground">
        Package, scene folder, image, or video. It is added to the library.
      </p>
    </div>
  );
}
