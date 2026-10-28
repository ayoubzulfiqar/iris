import { useState, type FormEvent } from "react";
import { errorText, importMediaUrl, inspectPackage } from "../api";
import type { Inspection } from "../types";

interface ImportLinkProps {
  onImported: (inspection: Inspection) => void;
}

export function ImportLink({ onImported }: ImportLinkProps) {
  const [url, setUrl] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);

  async function onSubmit(event: FormEvent) {
    event.preventDefault();
    const trimmed = url.trim();
    if (!trimmed) {
      setError("Paste a direct image or video link.");
      return;
    }
    setBusy(true);
    setError(null);
    setNote(null);
    try {
      const saved = await importMediaUrl(trimmed);
      const inspection = await inspectPackage(saved.path);
      const megabytes = (saved.bytes / (1024 * 1024)).toFixed(1);
      setNote(`Saved ${megabytes} MB to ${saved.path}`);
      onImported(inspection);
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="mx-auto max-w-3xl space-y-6">
      <header>
        <h1 className="text-2xl font-semibold tracking-tight">Import from a link</h1>
        <p className="mt-1 text-sm text-muted-foreground">
          Paste a direct file URL. Iris downloads mp4, webm, mov, mkv, png, jpeg, gif, webp, and
          bmp into Wallpaper/Imports, then you can apply it like any local file.
        </p>
      </header>
      <form onSubmit={(event) => void onSubmit(event)} className="space-y-3 rounded-xl border border-border bg-card p-5 shadow-sm">
        <label className="block text-sm">
          Media URL
          <input
            className="mt-1 w-full rounded-md border border-border bg-input px-3 py-2 font-mono text-xs"
            value={url}
            placeholder="https://example.com/loop.mp4"
            onChange={(event) => setUrl(event.currentTarget.value)}
            spellCheck={false}
          />
        </label>
        <button
          type="submit"
          disabled={busy}
          className="rounded-md bg-primary px-4 py-2 text-sm font-medium text-primary-foreground disabled:opacity-50"
        >
          {busy ? "Downloading…" : "Download"}
        </button>
        {error ? <p className="text-sm text-destructive">{error}</p> : null}
        {note ? <p className="break-all text-sm text-muted-foreground">{note}</p> : null}
      </form>
      <section className="rounded-xl border border-border bg-muted/40 p-5 text-sm text-muted-foreground">
        <p className="font-medium text-foreground">What this accepts</p>
        <ul className="mt-2 list-disc space-y-1 pl-5">
          <li>A direct https link to a video or image file, up to 512 MB.</li>
          <li>Gallery pages and workshop pages are not downloaded. Copy the file address.</li>
          <li>Local addresses and links that embed a password are rejected.</li>
        </ul>
      </section>
    </div>
  );
}
