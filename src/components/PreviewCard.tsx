import type { Inspection } from "../types";

interface PreviewCardProps {
  inspection: Inspection | null;
}

export function PreviewCard({ inspection }: PreviewCardProps) {
  if (!inspection) {
    return (
      <div className="preview empty-preview">
        <p>Select a wallpaper to see its title, description, and preview.</p>
      </div>
    );
  }

  return (
    <article className="preview">
      <div className="preview-frame">
        {inspection.preview_data_url ? (
          <img src={inspection.preview_data_url} alt={inspection.title} />
        ) : (
          <span className="preview-missing">No preview</span>
        )}
      </div>
      <p className="kind">{inspection.kind}</p>
      <h2>{inspection.title}</h2>
      {inspection.description ? <p className="description">{inspection.description}</p> : null}
      <p className="path">{inspection.path}</p>
    </article>
  );
}
