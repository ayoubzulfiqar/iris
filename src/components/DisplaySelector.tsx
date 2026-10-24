import type { MonitorInfo, MonitorTarget, WallpaperStatus } from "../types";
import { wallpaperFor } from "../types";

interface DisplaySelectorProps {
  monitors: MonitorInfo[];
  wallpapers: WallpaperStatus[];
  selected: MonitorTarget;
  onSelect: (target: MonitorTarget) => void;
}

export function DisplaySelector({
  monitors,
  wallpapers,
  selected,
  onSelect,
}: DisplaySelectorProps) {
  if (monitors.length === 0) {
    return (
      <p className="empty">
        No displays yet. Outputs show up after the daemon connects to the compositor.
      </p>
    );
  }

  return (
    <div className="display-grid">
      {monitors.map((monitor) => {
        const assigned = wallpaperFor(monitor.connector, wallpapers);
        const active =
          selected.kind === "connector" && selected.name === monitor.connector;
        const refresh =
          monitor.refresh_millihz == null
            ? null
            : `${Math.round(monitor.refresh_millihz / 1000)} Hz`;
        return (
          <button
            key={monitor.connector}
            type="button"
            className={active ? "display-card active" : "display-card"}
            onClick={() =>
              onSelect({ kind: "connector", name: monitor.connector })
            }
          >
            <span className="display-top">
              <span className="connector">{monitor.connector}</span>
              {monitor.primary ? <span className="badge">Primary</span> : null}
            </span>
            <span className="display-name">{monitor.name}</span>
            <span className="meta">
              {monitor.width_px} × {monitor.height_px}
              {monitor.scale !== 1 ? ` · ${monitor.scale}×` : ""}
              {refresh ? ` · ${refresh}` : ""}
            </span>
            <span className={assigned ? `assign ${assigned.state}` : "assign"}>
              {assigned
                ? `${assigned.state} · ${assigned.id}`
                : "No wallpaper"}
            </span>
            {assigned?.message ? (
              <span className="assign-message">{assigned.message}</span>
            ) : null}
          </button>
        );
      })}
    </div>
  );
}
