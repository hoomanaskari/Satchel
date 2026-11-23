import { describe, expect, it } from "vitest";
import {
  appInitial,
  appTileColor,
  cardLabel,
  cardPreview,
  emptyState,
  previewText,
  formatBytes,
  kindLabel,
  relativeTime,
  resultLabel,
  safeHexColor,
  statusLine,
} from "./format";
import type { ClipSummary } from "./types";

const MIN = 60_000;
const HR = 60 * MIN;
// Local-time noon on a Wednesday, so day boundaries are predictable.
const NOW = new Date(2026, 10, 18, 12, 0, 0).getTime();

function clip(over: Partial<ClipSummary> = {}): ClipSummary {
  return {
    id: 1,
    kind: "text",
    title: null,
    preview: "hello",
    sourceApp: { id: "com.apple.Notes", name: "Notes" },
    createdAt: NOW,
    lastUsedAt: NOW,
    byteSize: 5,
    hasThumbnail: false,
    pinboardIds: [],
    ...over,
  };
}

describe("relativeTime", () => {
  it.each([
    [0, "Now"],
    [30_000, "Now"],
    [-5_000, "Now"], // clock skew: never "in the future"
    [MIN, "1 min ago"],
    [2 * MIN + 10_000, "2 min ago"],
    [59 * MIN, "59 min ago"],
    [HR, "1 hr ago"],
    [3 * HR + 20 * MIN, "3 hr ago"],
  ])("%d ms ago is %s", (ago, label) => {
    expect(relativeTime(NOW - ago, NOW)).toBe(label);
  });

  it("uses calendar days past today", () => {
    const yesterdayLate = new Date(2026, 10, 17, 23, 0).getTime();
    expect(relativeTime(yesterdayLate, new Date(2026, 10, 18, 0, 30).getTime())).toBe("Yesterday");
    expect(relativeTime(new Date(2026, 10, 15, 9, 0).getTime(), NOW)).toBe("3 days ago");
    expect(relativeTime(new Date(2026, 10, 12, 9, 0).getTime(), NOW)).toBe("6 days ago");
  });

  it("prints a date a week or more back, with the year only when it differs", () => {
    const d = new Date(2026, 2, 4, 9, 0).getTime();
    expect(relativeTime(d, NOW)).toBe(new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric" }).format(d));
    const old = new Date(2024, 2, 4, 9, 0).getTime();
    expect(relativeTime(old, NOW)).toBe(
      new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric", year: "numeric" }).format(old),
    );
  });
});

describe("formatBytes", () => {
  it.each([
    [0, "0 B"],
    [999, "999 B"],
    [412_000, "412 KB"],
    [2_100_000, "2.1 MB"],
    [20_000_000, "20 MB"],
    [1_500_000_000, "1.5 GB"],
  ])("%d → %s", (n, s) => expect(formatBytes(n)).toBe(s));
});

describe("safeHexColor", () => {
  it.each(["#34C759", "#fff", "#FFFA", "#11223344"])("accepts %s", (c) => expect(safeHexColor(c)).toBe(c));
  it.each(["34C759", "#12", "#GGGGGG", "red", "url(https://x)", "#fff;background:url(x)", " #fff", ""])(
    "rejects %s",
    (c) => expect(safeHexColor(c)).toBeNull(),
  );
});

describe("kindLabel", () => {
  it("names each kind", () => {
    expect(["text", "link", "code", "color", "image", "file"].map((k) => kindLabel(k as never))).toEqual([
      "Text",
      "Link",
      "Code",
      "Color",
      "Image",
      "File",
    ]);
  });
});

describe("cardPreview", () => {
  it("text: body is the preview, title becomes the heading", () => {
    expect(cardPreview(clip({ preview: "a\nb" }))).toEqual({ head: null, body: "a\nb", mono: false, swatch: null });
    expect(cardPreview(clip({ title: "Sig", preview: "x" }))).toMatchObject({ head: "Sig", body: "x" });
  });

  it("strips leading blank lines and trailing whitespace from the body", () => {
    expect(cardPreview(clip({ preview: "\n\n  hi  \n\n" })).body).toBe("  hi");
  });

  it("link: heading is the title or host, body the address without scheme or www", () => {
    const l = clip({ kind: "link", preview: "https://www.sqlite.org/fts5.html" });
    expect(cardPreview(l)).toMatchObject({ head: "sqlite.org", body: "sqlite.org/fts5.html" });
    expect(cardPreview({ ...l, title: "FTS5" })).toMatchObject({ head: "FTS5", body: "sqlite.org/fts5.html" });
    expect(cardPreview(clip({ kind: "link", preview: "not a url" }))).toMatchObject({ head: null, body: "not a url" });
  });

  it("code is monospace", () => {
    expect(cardPreview(clip({ kind: "code", preview: "let x = 1" }))).toMatchObject({ mono: true, body: "let x = 1" });
  });

  it("color: swatch only for a valid hex", () => {
    expect(cardPreview(clip({ kind: "color", preview: "#34C759" }))).toMatchObject({ swatch: "#34C759", body: "#34C759" });
    expect(cardPreview(clip({ kind: "color", preview: "red;x" }))).toMatchObject({ swatch: null, body: "red;x" });
  });

  it("image: body describes size", () => {
    expect(cardPreview(clip({ kind: "image", preview: "", byteSize: 412_000 }))).toMatchObject({
      head: null,
      body: "Image · 412 KB",
    });
  });

  it("file: first name is the heading, the rest listed or counted", () => {
    expect(cardPreview(clip({ kind: "file", preview: "roadmap-q1.pdf" }))).toMatchObject({
      head: "roadmap-q1.pdf",
      body: "PDF file",
    });
    expect(cardPreview(clip({ kind: "file", preview: "/Users/me/a.png\n/Users/me/b.png" }))).toMatchObject({
      head: "a.png",
      body: "b.png",
    });
    const many = Array.from({ length: 8 }, (_, i) => `f${i}.txt`).join("\n");
    expect(cardPreview(clip({ kind: "file", preview: many }))).toMatchObject({
      head: "8 files",
      body: "f0.txt\nf1.txt\nf2.txt\nf3.txt\nf4.txt\n+3 more",
    });
    expect(cardPreview(clip({ kind: "file", preview: "README" })).body).toBe("File");
  });
});

describe("cardLabel", () => {
  it("summarizes a card for assistive tech", () => {
    expect(cardLabel(clip({ preview: "hello world" }), NOW)).toBe("Text, hello world, from Notes, Now");
    expect(cardLabel(clip({ sourceApp: null, title: "T" }), NOW)).toBe("Text, T, Now");
    expect(cardLabel(clip({ preview: "x".repeat(200) }), NOW)).toBe(`Text, ${"x".repeat(80)}…, from Notes, Now`);
  });
});

describe("app tile", () => {
  it("initial is the first letter, ? when unknown", () => {
    expect(appInitial({ id: "a", name: "safari" })).toBe("S");
    expect(appInitial(null)).toBe("?");
    expect(appInitial({ id: "a", name: "  " })).toBe("?");
    expect(appInitial({ id: "a", name: "Élan" })).toBe("É");
  });

  it("tile color is stable per app id and a valid hex", () => {
    const a = appTileColor({ id: "com.apple.Notes", name: "Notes" });
    expect(a).toBe(appTileColor({ id: "com.apple.Notes", name: "Other" }));
    expect(safeHexColor(a)).toBe(a);
    expect(appTileColor(null)).toBe(appTileColor(null));
  });
});

describe("status and result labels", () => {
  it("describes the selection", () => {
    expect(statusLine([], NOW)).toBe("No Selection");
    expect(statusLine([clip()], NOW)).toBe("Copied from Notes · Now");
    expect(statusLine([clip({ sourceApp: null })], NOW)).toBe("Copied · Now");
    expect(statusLine([clip(), clip({ id: 2 })], NOW)).toBe("2 clips selected");
  });

  it("counts results, marking more pages", () => {
    expect(resultLabel(0, false)).toBe("0 items");
    expect(resultLabel(1, false)).toBe("1 item");
    expect(resultLabel(50, true)).toBe("50+ items");
  });
});

describe("emptyState", () => {
  it("explains an empty history, an empty pinboard and an empty search", () => {
    const none = { text: "", kind: "all" as const, pinboardId: null };
    expect(emptyState(none, null)).toEqual({ title: "No Clips Yet", detail: "Copy something and it appears here." });
    expect(emptyState({ ...none, pinboardId: 1 }, "Work")).toEqual({
      title: "Nothing in Work",
      detail: "Pin clips here from History.",
    });
    expect(emptyState({ ...none, text: "zz" }, null)).toEqual({
      title: "No Results",
      detail: "Try a different search, or choose All.",
    });
    expect(emptyState({ ...none, kind: "image" }, null).title).toBe("No Results");
  });
});

describe("previewText", () => {
  it("keeps short text and cuts very long text for the large preview", () => {
    expect(previewText("abc", 10)).toEqual({ text: "abc", truncated: false });
    expect(previewText("abcdefghijkl", 10)).toEqual({ text: "abcdefghij", truncated: true });
  });
});
