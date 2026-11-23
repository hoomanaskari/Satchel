// Pure formatting helpers for cards, status line and labels. Everything
// returned here is plain text: the UI only ever puts it into text nodes.

import type { ClipKind, ClipSummary, SourceApp } from "./types";

const MIN = 60_000;
const HR = 60 * MIN;
const DAY = 24 * HR;

const dateFmt = new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric" });
const dateYearFmt = new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric", year: "numeric" });

function startOfDay(ms: number): number {
  const d = new Date(ms);
  d.setHours(0, 0, 0, 0);
  return d.getTime();
}

/** "Now", "5 min ago", "3 hr ago", "Yesterday", "4 days ago", then a date. */
export function relativeTime(at: number, now: number): string {
  const ago = now - at;
  if (ago < MIN) return "Now";
  if (ago < HR) return `${Math.floor(ago / MIN)} min ago`;
  const days = Math.round((startOfDay(now) - startOfDay(at)) / DAY);
  if (days <= 0) return `${Math.floor(ago / HR)} hr ago`;
  if (days === 1) return "Yesterday";
  if (days < 7) return `${days} days ago`;
  const sameYear = new Date(at).getFullYear() === new Date(now).getFullYear();
  return (sameYear ? dateFmt : dateYearFmt).format(at);
}

/** Decimal units, like Finder: "412 KB", "2.1 MB". */
export function formatBytes(n: number): string {
  const units = ["B", "KB", "MB", "GB", "TB"];
  let v = n;
  let u = 0;
  while (v >= 1000 && u < units.length - 1) {
    v /= 1000;
    u++;
  }
  if (u === 0) return `${n} B`;
  const s = v < 10 ? (Math.round(v * 10) / 10).toString() : Math.round(v).toString();
  return `${s} ${units[u]}`;
}

const HEX = /^#(?:[0-9a-fA-F]{3,4}|[0-9a-fA-F]{6}|[0-9a-fA-F]{8})$/;

/** The value if it is a plain hex color, else null. Guards every value put into a style. */
export function safeHexColor(value: string | null | undefined): string | null {
  return value && HEX.test(value) ? value : null;
}

const KIND_LABELS: Record<ClipKind, string> = {
  text: "Text",
  link: "Link",
  code: "Code",
  color: "Color",
  image: "Image",
  file: "File",
};

export function kindLabel(kind: ClipKind): string {
  return KIND_LABELS[kind];
}

export interface CardPreview {
  head: string | null;
  body: string;
  mono: boolean;
  /** Validated hex for the color swatch. */
  swatch: string | null;
}

function cleanBody(s: string): string {
  return s.replace(/^(?:[ \t]*\r?\n)+/, "").trimEnd();
}

function basename(path: string): string {
  const parts = path.split(/[/\\]/).filter(Boolean);
  return parts.length ? parts[parts.length - 1] : path;
}

function linkParts(raw: string): { host: string; rest: string } | null {
  try {
    const u = new URL(raw.trim());
    if (u.protocol !== "http:" && u.protocol !== "https:") return null;
    const host = u.hostname.replace(/^www\./, "");
    const path = u.pathname === "/" ? "" : u.pathname;
    return { host, rest: host + path + u.search + u.hash };
  } catch {
    return null;
  }
}

const FILES_SHOWN = 5;

export function cardPreview(c: ClipSummary): CardPreview {
  const base: CardPreview = { head: c.title, body: cleanBody(c.preview), mono: false, swatch: null };
  switch (c.kind) {
    case "link": {
      const p = linkParts(c.preview);
      return p ? { ...base, head: c.title ?? p.host, body: p.rest } : base;
    }
    case "code":
      return { ...base, mono: true };
    case "color":
      return { ...base, swatch: safeHexColor(c.preview.trim()) };
    case "image":
      return { ...base, body: `Image · ${formatBytes(c.byteSize)}` };
    case "file": {
      const names = c.preview.split(/\r?\n/).map((s) => s.trim()).filter(Boolean).map(basename);
      if (names.length <= 1) {
        const name = names[0] ?? "";
        const dot = name.lastIndexOf(".");
        const ext = dot > 0 ? name.slice(dot + 1).toUpperCase() : "";
        return { ...base, head: c.title ?? name, body: ext ? `${ext} file` : "File" };
      }
      if (names.length <= FILES_SHOWN + 1) {
        return { ...base, head: c.title ?? names[0], body: names.slice(1).join("\n") };
      }
      const shown = names.slice(0, FILES_SHOWN);
      shown.push(`+${names.length - FILES_SHOWN} more`);
      return { ...base, head: c.title ?? `${names.length} files`, body: shown.join("\n") };
    }
    default:
      return base;
  }
}

const LABEL_CHARS = 80;

/** Accessible name for a card. */
export function cardLabel(c: ClipSummary, now: number): string {
  const p = cardPreview(c);
  let text = (p.head ?? p.body).replace(/\s+/g, " ").trim();
  if (text.length > LABEL_CHARS) text = text.slice(0, LABEL_CHARS) + "…";
  const parts = [kindLabel(c.kind), text];
  if (c.sourceApp) parts.push(`from ${c.sourceApp.name}`);
  parts.push(relativeTime(c.lastUsedAt, now));
  return parts.filter(Boolean).join(", ");
}

export function appInitial(app: SourceApp | null): string {
  const ch = app ? Array.from(app.name.trim())[0] : undefined;
  return ch ? ch.toLocaleUpperCase() : "?";
}

// Dark enough for white text in both themes.
const TILE_COLORS = ["#A86B00", "#0A66C2", "#1558B0", "#7A3EC8", "#4A4A4F", "#0B6FD3", "#B4235A", "#2B7A3D", "#8F4A1D", "#2B2B2E"];

export function appTileColor(app: SourceApp | null): string {
  if (!app) return "#6E6E73";
  // FNV-1a, enough to spread ids over a small palette.
  let h = 0x811c9dc5;
  for (let i = 0; i < app.id.length; i++) {
    h ^= app.id.charCodeAt(i);
    h = Math.imul(h, 0x01000193);
  }
  return TILE_COLORS[(h >>> 0) % TILE_COLORS.length];
}

/** Footer status for the current selection. */
export function statusLine(selected: ClipSummary[], now: number): string {
  if (selected.length === 0) return "No Selection";
  if (selected.length > 1) return `${selected.length} clips selected`;
  const c = selected[0];
  const when = relativeTime(c.createdAt, now);
  return c.sourceApp ? `Copied from ${c.sourceApp.name} · ${when}` : `Copied · ${when}`;
}

export function resultLabel(count: number, more: boolean): string {
  return `${count}${more ? "+" : ""} ${count === 1 && !more ? "item" : "items"}`;
}

export function emptyState(
  f: { text: string; kind: string; pinboardId: number | null },
  pinboardName: string | null,
): { title: string; detail: string } {
  if (f.text.trim() || f.kind !== "all") {
    return { title: "No Results", detail: "Try a different search, or choose All." };
  }
  if (f.pinboardId != null) {
    return { title: `Nothing in ${pinboardName ?? "this pinboard"}`, detail: "Pin clips here from History." };
  }
  return { title: "No Clips Yet", detail: "Copy something and it appears here." };
}

/** Characters shown in the large preview; full text can be up to 2 MB. */
export const PREVIEW_TEXT_CHARS = 20_000;

export function previewText(full: string, max = PREVIEW_TEXT_CHARS): { text: string; truncated: boolean } {
  return full.length > max ? { text: full.slice(0, max), truncated: true } : { text: full, truncated: false };
}
