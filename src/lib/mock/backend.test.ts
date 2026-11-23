import { describe, expect, it, vi } from "vitest";
import { createMockBackend } from "./backend";
import type { ClipSummary, Query } from "../types";

const NOW = 1_800_000_000_000;

function q(over: Partial<Query> = {}): Query {
  return { text: null, kind: null, pinboardId: null, limit: 0, before: null, ...over };
}

function setup() {
  const m = createMockBackend({ now: () => NOW });
  const inv = <T>(cmd: string, args?: Record<string, unknown>) => m.transport.invoke(cmd, args) as Promise<T>;
  return { m, inv };
}

describe("mock backend: list_clips", () => {
  it("returns the default page of 50, newest first by (lastUsedAt, id)", async () => {
    const { inv } = setup();
    const rows = await inv<ClipSummary[]>("list_clips", { query: q() });
    expect(rows).toHaveLength(50);
    for (let i = 1; i < rows.length; i++) {
      const a = rows[i - 1];
      const b = rows[i];
      expect(a.lastUsedAt > b.lastUsedAt || (a.lastUsedAt === b.lastUsedAt && a.id > b.id)).toBe(true);
    }
  });

  it("clamps the limit to 200", async () => {
    const { inv } = setup();
    const rows = await inv<ClipSummary[]>("list_clips", { query: q({ limit: 5000 }) });
    expect(rows.length).toBeLessThanOrEqual(200);
  });

  it("pages with the keyset cursor without gaps or overlap", async () => {
    const { inv, m } = setup();
    const seen: number[] = [];
    let before: [number, number] | null = null;
    for (;;) {
      const page: ClipSummary[] = await inv("list_clips", { query: q({ limit: 17, before }) });
      seen.push(...page.map((c) => c.id));
      if (page.length < 17) break;
      const last = page[page.length - 1];
      before = [last.lastUsedAt, last.id];
    }
    expect(new Set(seen).size).toBe(seen.length);
    expect(seen.length).toBe(m.count());
  });

  it("filters by text (case-insensitive over preview, title and app name), kind and pinboard", async () => {
    const { inv } = setup();
    const byText = await inv<ClipSummary[]>("list_clips", { query: q({ text: "nspasteboard", limit: 200 }) });
    expect(byText.length).toBeGreaterThan(0);
    for (const c of byText) {
      const hay = `${c.preview} ${c.title ?? ""} ${c.sourceApp?.name ?? ""}`.toLowerCase();
      expect(hay).toContain("nspasteboard");
    }
    const byApp = await inv<ClipSummary[]>("list_clips", { query: q({ text: "terminal", limit: 200 }) });
    expect(byApp.some((c) => c.sourceApp?.name === "Terminal")).toBe(true);

    const colors = await inv<ClipSummary[]>("list_clips", { query: q({ kind: "color", limit: 200 }) });
    expect(colors.length).toBeGreaterThan(0);
    expect(colors.every((c) => c.kind === "color")).toBe(true);

    const pinned = await inv<ClipSummary[]>("list_clips", { query: q({ pinboardId: 1, limit: 200 }) });
    expect(pinned.length).toBeGreaterThan(0);
    expect(pinned.every((c) => c.pinboardIds.includes(1))).toBe(true);
  });

  it("covers every kind and has thumbnails only for images", async () => {
    const { inv } = setup();
    const rows = await inv<ClipSummary[]>("list_clips", { query: q({ limit: 200 }) });
    const kinds = new Set(rows.map((r) => r.kind));
    expect([...kinds].sort()).toEqual(["code", "color", "file", "image", "link", "text"]);
    for (const r of rows) expect(r.hasThumbnail).toBe(r.kind === "image");
    for (const r of rows) expect(r.preview.length).toBeLessThanOrEqual(300);
  });
});

describe("mock backend: clip commands", () => {
  it("get_clip_text returns the full text, null for images", async () => {
    const { inv } = setup();
    const rows = await inv<ClipSummary[]>("list_clips", { query: q({ limit: 200 }) });
    const text = rows.find((r) => r.kind === "code")!;
    expect(await inv("get_clip_text", { id: text.id })).toEqual(expect.any(String));
    const img = rows.find((r) => r.kind === "image")!;
    expect(await inv("get_clip_text", { id: img.id })).toBeNull();
  });

  it("get_clip_thumbnail returns PNG bytes for images and rejects otherwise", async () => {
    const { inv } = setup();
    const rows = await inv<ClipSummary[]>("list_clips", { query: q({ limit: 200 }) });
    const img = rows.find((r) => r.kind === "image")!;
    const bytes = new Uint8Array((await inv("get_clip_thumbnail", { id: img.id })) as ArrayBuffer);
    expect(Array.from(bytes.slice(0, 4))).toEqual([0x89, 0x50, 0x4e, 0x47]);
    const txt = rows.find((r) => r.kind === "text")!;
    await expect(inv("get_clip_thumbnail", { id: txt.id })).rejects.toBeTruthy();
  });

  it("paste_clip bumps the clip to the top and emits clips-changed", async () => {
    const { m, inv } = setup();
    const events: unknown[] = [];
    await m.transport.listen("clips-changed", (p) => events.push(p));
    const rows = await inv<ClipSummary[]>("list_clips", { query: q({ limit: 10 }) });
    const target = rows[5];
    await inv("paste_clip", { id: target.id, plainText: false });
    expect(m.log.at(-1)).toEqual({ cmd: "paste_clip", args: { id: target.id, plainText: false } });
    const after = await inv<ClipSummary[]>("list_clips", { query: q({ limit: 1 }) });
    expect(after[0].id).toBe(target.id);
    expect(events).toEqual([{ reason: "bumped", id: target.id }]);
  });

  it("delete_clip removes the clip once", async () => {
    const { m, inv } = setup();
    const events: unknown[] = [];
    await m.transport.listen("clips-changed", (p) => events.push(p));
    const before = m.count();
    expect(await inv("delete_clip", { id: 1 })).toBe(true);
    expect(await inv("delete_clip", { id: 1 })).toBe(false);
    expect(m.count()).toBe(before - 1);
    expect(events).toEqual([{ reason: "deleted", id: 1 }]);
  });

  it("set_clip_title sets and clears the title", async () => {
    const { inv } = setup();
    await inv("set_clip_title", { id: 2, title: "Hello" });
    let rows = await inv<ClipSummary[]>("list_clips", { query: q({ text: "hello", limit: 200 }) });
    expect(rows.find((r) => r.id === 2)?.title).toBe("Hello");
    await inv("set_clip_title", { id: 2, title: null });
    rows = await inv<ClipSummary[]>("list_clips", { query: q({ limit: 200 }) });
    expect(rows.find((r) => r.id === 2)?.title).toBeNull();
  });

  it("clear_history removes unpinned clips and returns the count", async () => {
    const { m, inv } = setup();
    const total = m.count();
    const removed = await inv<number>("clear_history");
    expect(removed).toBeGreaterThan(0);
    expect(m.count()).toBe(total - removed);
    const rows = await inv<ClipSummary[]>("list_clips", { query: q({ limit: 200 }) });
    expect(rows.every((r) => r.pinboardIds.length > 0)).toBe(true);
  });

  it("rejects unknown commands with a string message", async () => {
    const { inv } = setup();
    await expect(inv("greet", {})).rejects.toBe("unknown command: greet");
  });
});

describe("mock backend: pinboards", () => {
  it("creates, renames and deletes pinboards and emits pinboards-changed", async () => {
    const { m, inv } = setup();
    const changed = vi.fn();
    await m.transport.listen("pinboards-changed", changed);
    const boards = await inv<{ id: number; position: number }[]>("list_pinboards");
    const created = await inv<{ id: number; name: string; color: string; position: number }>("create_pinboard", {
      name: "Recipes",
      color: "#FF9F0A",
    });
    expect(created).toMatchObject({ name: "Recipes", color: "#FF9F0A" });
    expect(created.position).toBeGreaterThan(Math.max(...boards.map((b) => b.position)));
    await inv("rename_pinboard", { id: created.id, name: "Food" });
    expect((await inv<{ name: string }[]>("list_pinboards")).at(-1)!.name).toBe("Food");
    await inv("delete_pinboard", { id: created.id });
    expect(await inv<unknown[]>("list_pinboards")).toHaveLength(boards.length);
    expect(changed).toHaveBeenCalledTimes(3);
  });

  it("pin_clip and unpin_clip change membership", async () => {
    const { inv } = setup();
    await inv("pin_clip", { clipId: 10, pinboardId: 2 });
    let rows = await inv<ClipSummary[]>("list_clips", { query: q({ pinboardId: 2, limit: 200 }) });
    expect(rows.some((r) => r.id === 10)).toBe(true);
    await inv("unpin_clip", { clipId: 10, pinboardId: 2 });
    rows = await inv<ClipSummary[]>("list_clips", { query: q({ pinboardId: 2, limit: 200 }) });
    expect(rows.some((r) => r.id === 10)).toBe(false);
  });

  it("deleting a pinboard drops it from clips", async () => {
    const { inv } = setup();
    await inv("delete_pinboard", { id: 1 });
    const rows = await inv<ClipSummary[]>("list_clips", { query: q({ limit: 200 }) });
    expect(rows.every((r) => !r.pinboardIds.includes(1))).toBe(true);
  });
});

describe("mock backend: settings and simulation", () => {
  it("saves settings and emits settings-changed", async () => {
    const { m, inv } = setup();
    const seen: unknown[] = [];
    await m.transport.listen("settings-changed", (p) => seen.push(p));
    const s = await inv<Record<string, unknown>>("get_settings");
    await inv("save_settings", { settings: { ...s, maxItems: 10 } });
    expect((await inv<{ maxItems: number }>("get_settings")).maxItems).toBe(10);
    await inv("set_capture_paused", { paused: true });
    expect((await inv<{ capturePaused: boolean }>("get_settings")).capturePaused).toBe(true);
    expect(seen).toHaveLength(2);
  });

  it("simulateCopy inserts a new clip on top and show() emits shelf-shown", async () => {
    const { m, inv } = setup();
    const shown = vi.fn();
    await m.transport.listen("shelf-shown", shown);
    const id = m.simulateCopy("fresh text");
    const top = await inv<ClipSummary[]>("list_clips", { query: q({ limit: 1 }) });
    expect(top[0]).toMatchObject({ id, kind: "text", preview: "fresh text" });
    m.show();
    expect(shown).toHaveBeenCalledWith(null);
  });

  it("unlisten stops delivery", async () => {
    const { m } = setup();
    const h = vi.fn();
    const stop = await m.transport.listen("shelf-shown", h);
    stop();
    m.show();
    expect(h).not.toHaveBeenCalled();
  });
});
