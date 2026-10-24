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
    <footer className="control-bar">
      <div className="scope">
        <span>Target</span>
        <strong>{scopeLabel}</strong>
        <button
          type="button"
          className={scopeIsAll ? "chip active" : "chip"}
          onClick={onAll}
        >
          All displays
        </button>
      </div>
      <div className="control-actions">
        <button type="button" onClick={onPause} disabled={pending}>
          Pause
        </button>
        <button type="button" onClick={onResume} disabled={pending}>
          Resume
        </button>
        <button type="button" className="danger" onClick={onClear} disabled={pending}>
          Clear
        </button>
      </div>
      {error ? <p className="bar-error">{error}</p> : null}
    </footer>
  );
}
