// Reference-counted object URLs for card thumbnails. A card acquires its
// clip's URL when it enters the rendered window and releases it when it
// leaves; the URL is revoked as soon as nobody holds it.

import type { ClipId } from "./types";

export interface ThumbDeps {
  fetch: (id: ClipId) => Promise<Uint8Array>;
  createUrl: (blob: Blob) => string;
  revoke: (url: string) => void;
}

interface Entry {
  refs: number;
  url: string | null;
  pending: Promise<string | null>;
}

export class ThumbnailCache {
  private entries = new Map<ClipId, Entry>();

  constructor(private readonly deps: ThumbDeps) {}

  get size(): number {
    return this.entries.size;
  }

  acquire(id: ClipId): Promise<string | null> {
    const existing = this.entries.get(id);
    if (existing) {
      existing.refs++;
      return existing.pending;
    }
    const entry: Entry = { refs: 1, url: null, pending: Promise.resolve(null) };
    this.entries.set(id, entry);
    entry.pending = this.deps.fetch(id).then(
      (bytes) => {
        const url = this.deps.createUrl(new Blob([bytes as BlobPart], { type: "image/png" }));
        if (this.entries.get(id) !== entry) {
          // Everyone let go while it was loading.
          this.deps.revoke(url);
          return null;
        }
        entry.url = url;
        return url;
      },
      () => {
        if (this.entries.get(id) === entry) this.entries.delete(id);
        return null;
      },
    );
    return entry.pending;
  }

  release(id: ClipId): void {
    const entry = this.entries.get(id);
    if (!entry) return;
    if (--entry.refs > 0) return;
    this.entries.delete(id);
    if (entry.url) this.deps.revoke(entry.url);
  }

  clear(): void {
    for (const e of this.entries.values()) if (e.url) this.deps.revoke(e.url);
    this.entries.clear();
  }
}
