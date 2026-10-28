interface ControlBarProps {
  scopeLabel: string;
  scopeIsAll: boolean;
  pending: boolean;
  error: string | null;
  onPause: () => void;
  onResume: () => void;
  onClear: () => void;
  onAll: () => void;
}

export function ControlBar({
  scopeLabel,
  scopeIsAll,
  pending,
  error,
  onPause,
  onResume,
  onClear,
  onAll,
}: ControlBarProps) {
  return (
    <footer className="flex flex-wrap items-center justify-between gap-3 border-t border-border bg-card px-5 py-3">
      <div className="flex items-center gap-3 text-sm">
        <span className="text-muted-foreground">Target</span>
        <strong>{scopeLabel}</strong>
        <button
          type="button"
          className={
            scopeIsAll
              ? "rounded-md bg-primary px-2.5 py-1 text-xs text-primary-foreground"
              : "rounded-md bg-secondary px-2.5 py-1 text-xs text-secondary-foreground"
          }
          onClick={onAll}
        >
          All displays
        </button>
      </div>
      <div className="flex items-center gap-2">
        <button type="button" className="rounded-md border border-border px-3 py-1.5 text-sm" onClick={onPause} disabled={pending}>
          Pause
        </button>
        <button type="button" className="rounded-md border border-border px-3 py-1.5 text-sm" onClick={onResume} disabled={pending}>
          Resume
        </button>
        <button type="button" className="rounded-md bg-destructive px-3 py-1.5 text-sm text-destructive-foreground" onClick={onClear} disabled={pending}>
          Clear
        </button>
      </div>
      {error ? <p className="w-full text-sm text-destructive">{error}</p> : null}
    </footer>
  );
}
