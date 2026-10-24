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
    <div className={hovering ? "dropzone hovering" : "dropzone"}>
      <strong>Drop a wallpaper</strong>
      <p>A .pkg archive, an unpacked scene folder, or a still image.</p>
    </div>
  );
}
