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
      <p className="text-sm text-muted-foreground">
        No displays yet. Outputs show up after the daemon connects to the compositor.
      </p>
    );
  }

  return (
    <div className="grid gap-4 lg:grid-cols-2">
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
            className={
              active
                ? "rounded-2xl border-2 border-primary bg-card p-4 text-left shadow-md"
                : "rounded-2xl border border-border bg-card p-4 text-left shadow-sm"
            }
            onClick={() =>
              onSelect({ kind: "connector", name: monitor.connector })
            }
          >
            <div
              className="mb-4 flex aspect-video items-end rounded-xl bg-gradient-to-br from-primary to-chart-3 p-4 text-white"
              style={{ maxHeight: 220 }}
            >
              <div>
                <p className="text-xs uppercase tracking-wide text-white/80">{monitor.connector}</p>
                <p className="text-lg font-semibold">
                  {assigned ? assigned.state : "Empty desktop"}
                </p>
              </div>
            </div>
            <div className="flex items-start justify-between gap-3">
              <div>
                <p className="font-medium">{monitor.name}</p>
                <p className="text-xs text-muted-foreground">
                  {monitor.width_px} × {monitor.height_px}
                  {monitor.scale !== 1 ? ` · ${monitor.scale}×` : ""}
                  {refresh ? ` · ${refresh}` : ""}
                </p>
              </div>
              {monitor.primary ? (
                <span className="rounded-md bg-primary px-2 py-0.5 text-xs text-primary-foreground">
                  Primary
                </span>
              ) : null}
            </div>
            <p className="mt-3 truncate text-sm">
              {assigned ? assigned.id : "Nothing assigned"}
            </p>
            {assigned?.message ? (
              <p className="mt-1 text-xs text-muted-foreground">{assigned.message}</p>
            ) : null}
          </button>
        );
      })}
    </div>
  );
}
