// What the shelf is filtering by, and how that becomes a contract `Query`.

import type { ClipKind, Cursor, PinboardId, Query } from "./types";

export type KindScope = ClipKind | "all";

export interface Filters {
  /** Raw search field text. */
  text: string;
  kind: KindScope;
  /** `null` is the History tab. */
  pinboardId: PinboardId | null;
}

export const MAX_QUERY_CHARS = 256;
export const MAX_LIMIT = 200;

export const KIND_SCOPES: readonly { id: KindScope; label: string }[] = [
  { id: "all", label: "All" },
  { id: "text", label: "Text" },
  { id: "link", label: "Links" },
  { id: "code", label: "Code" },
  { id: "image", label: "Images" },
  { id: "color", label: "Colors" },
  { id: "file", label: "Files" },
];

export function buildQuery(f: Filters, limit: number, before: Cursor | null = null): Query {
  const text = f.text.trim().slice(0, MAX_QUERY_CHARS);
  return {
    text: text || null,
    kind: f.kind === "all" ? null : f.kind,
    pinboardId: f.pinboardId,
    limit: Math.min(MAX_LIMIT, Math.max(1, Math.floor(limit))),
    before,
  };
}

export function cursorOf(c: { lastUsedAt: number; id: number }): Cursor {
  return [c.lastUsedAt, c.id];
}
