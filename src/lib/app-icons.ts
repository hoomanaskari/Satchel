// Object URLs for app icons (the settings window's app list, the shelf's
// cards). Rust renders each icon once (and caches the PNG); here each id is
// fetched once per window and its `blob:` URL (the only image source the CSP
// allows besides 'self') is kept until the window goes away or `clear` is
// called, so every card from the same app shares one entry. Ids compare
// case-insensitively, like the excluded-app list.

export interface IconDeps {
  fetch: (id: string) => Promise<Uint8Array>;
  createUrl: (blob: Blob) => string;
  revoke: (url: string) => void;
}

export class AppIconCache {
  private entries = new Map<string, Promise<string | null>>();
  private urls = new Set<string>();
  /** Settled entries, readable synchronously so a cached icon is in the first frame. */
  private settled = new Map<string, string | null>();

  constructor(private readonly deps: IconDeps) {}

  /** The icon's URL, or null when the app has no icon here (not installed). */
  get(id: string): Promise<string | null> {
    const k = key(id);
    let entry = this.entries.get(k);
    if (!entry) {
      entry = this.deps.fetch(id).then(
        (bytes) => {
          if (bytes.length === 0) return null;
          const url = this.deps.createUrl(new Blob([bytes as BlobPart], { type: "image/png" }));
          this.urls.add(url);
          return url;
        },
        () => null,
      ).then((url) => {
        // A `clear` while this was in flight dropped the entry: do not resurrect it.
        if (this.entries.get(k) === entry) this.settled.set(k, url);
        else if (url) {
          this.urls.delete(url);
          this.deps.revoke(url);
        }
        return url;
      });
      this.entries.set(k, entry);
    }
    return entry;
  }

  /**
   * What `get` would resolve to, if it already has: a URL, or null for an app
   * without an icon. `undefined` while unknown (never asked, or in flight).
   */
  peek(id: string): string | null | undefined {
    return this.settled.get(key(id));
  }

  clear(): void {
    for (const url of this.urls) this.deps.revoke(url);
    this.urls.clear();
    this.entries.clear();
    this.settled.clear();
  }
}

function key(id: string): string {
  return id.trim().toLowerCase();
}

/**
 * What an app-icon slot shows right now: the icon's URL, `null` for the
 * lettered fallback tile (no app id, no cache, or the app has no icon here:
 * uninstalled, another platform, a mock without one), or `undefined` while
 * the icon is still loading (a blank tile, so a quick load does not flash a
 * letter). `loaded` is what a load started by the slot itself returned.
 */
export function iconSlot(
  cache: AppIconCache | null,
  id: string | null | undefined,
  loaded: { id: string; url: string | null } | null,
): string | null | undefined {
  if (!id || !id.trim() || !cache) return null;
  if (loaded && key(loaded.id) === key(id)) return loaded.url;
  return cache.peek(id);
}
