import type { Inspection } from "../types";

interface PreviewCardProps {
  inspection: Inspection | null;
}

export function PreviewCard({ inspection }: PreviewCardProps) {
  if (!inspection) {
    return (
      <div className="rounded-xl border border-dashed border-border bg-card p-6 text-sm text-muted-foreground">
        Select a wallpaper to see its title, description, and preview.
      </div>
    );
  }

  return (
    <article className="overflow-hidden rounded-xl border border-border bg-card shadow-sm">
      <div className="flex aspect-video items-center justify-center bg-muted">
        {inspection.preview_data_url ? (
          <img
            src={inspection.preview_data_url}
            alt={inspection.title}
            className="h-full w-full object-cover"
          />
        ) : (
          <span className="text-sm text-muted-foreground">No preview</span>
        )}
      </div>
      <div className="space-y-2 p-4">
        <p className="text-xs font-medium uppercase tracking-wide text-muted-foreground">
          {inspection.kind}
        </p>
        <h2 className="text-lg font-semibold">{inspection.title}</h2>
        {inspection.description ? (
          <p className="text-sm text-muted-foreground">{inspection.description}</p>
        ) : null}
        <p className="break-all font-mono text-xs text-muted-foreground">{inspection.path}</p>
      </div>
    </article>
  );
}
