import { describe, expect, it, vi } from "vitest";
import { Pager } from "./paging";
import type { Filters } from "./query";
import type { ClipSummary, Query } from "./types";

const filters: Filters = { text: "", kind: "all", pinboardId: null };

function clip(id: number): ClipSummary {
  return {
    id,
    kind: "text",
    title: null,
    preview: `clip ${id}`,
    sourceApp: null,
    createdAt: id * 10,
    lastUsedAt: id * 10,
    byteSize: 1,
    hasThumbnail: false,
    linkPreview: null,
    pinboardIds: [],
  };
}

/** A store of ids n..1, newest first, honoring limit and cursor. */
function store(n: number) {
  const all = Array.from({ length: n }, (_, i) => clip(n - i));
  return vi.fn(async (q: Query) =>
    all.filter((c) => !q.before || c.lastUsedAt < q.before[0] || (c.lastUsedAt === q.before[0] && c.id < q.before[1])).slice(0, q.limit),
  );
}

function deferred<T>() {
  let resolve!: (v: T) => void;
  const promise = new Promise<T>((r) => (resolve = r));
  return { promise, resolve };
}

describe("Pager", () => {
  it("loads the first page and reports whether more exist", async () => {
    const fetch = store(30);
    const p = new Pager(fetch, 10);
    await p.reset(filters);
    expect(p.items.map((c) => c.id)).toEqual([30, 29, 28, 27, 26, 25, 24, 23, 22, 21]);
    expect(p.hasMore).toBe(true);
    expect(fetch).toHaveBeenLastCalledWith({ text: null, kind: null, pinboardId: null, limit: 10, before: null });
  });

  it("pages with the cursor of the last item until the store runs dry", async () => {
    const fetch = store(25);
    const p = new Pager(fetch, 10);
    await p.reset(filters);
    await p.loadMore();
    expect(fetch).toHaveBeenLastCalledWith(expect.objectContaining({ before: [160, 16], limit: 10 }));
    await p.loadMore();
    expect(p.items).toHaveLength(25);
    expect(p.hasMore).toBe(false);
    await p.loadMore();
    expect(fetch).toHaveBeenCalledTimes(3);
  });

  it("does not issue two loadMore requests at once", async () => {
    const fetch = store(100);
    const p = new Pager(fetch, 10);
    await p.reset(filters);
    await Promise.all([p.loadMore(), p.loadMore()]);
    expect(fetch).toHaveBeenCalledTimes(2);
    expect(p.items).toHaveLength(20);
  });

  it("drops responses that a newer reset superseded", async () => {
    const slow = deferred<ClipSummary[]>();
    const fetch = vi
      .fn<(q: Query) => Promise<ClipSummary[]>>()
      .mockReturnValueOnce(slow.promise)
      .mockResolvedValueOnce([clip(2)]);
    const p = new Pager(fetch, 10);
    const first = p.reset({ ...filters, text: "a" });
    const second = p.reset({ ...filters, text: "ab" });
    await second;
    slow.resolve([clip(1)]);
    expect(await first).toBe(false);
    expect(p.items.map((c) => c.id)).toEqual([2]);
    expect(p.filters.text).toBe("ab");
  });

  it("discards a loadMore that finishes after a reset", async () => {
    const slow = deferred<ClipSummary[]>();
    const fetch = vi
      .fn<(q: Query) => Promise<ClipSummary[]>>()
      .mockResolvedValueOnce(Array.from({ length: 10 }, (_, i) => clip(100 - i)))
      .mockReturnValueOnce(slow.promise)
      .mockResolvedValueOnce([clip(5)]);
    const p = new Pager(fetch, 10);
    await p.reset(filters);
    const more = p.loadMore();
    await p.reset({ ...filters, kind: "code" });
    slow.resolve([clip(50)]);
    await more;
    expect(p.items.map((c) => c.id)).toEqual([5]);
  });

  it("skips duplicates when appending", async () => {
    const fetch = vi
      .fn<(q: Query) => Promise<ClipSummary[]>>()
      .mockResolvedValueOnce([clip(3), clip(2)])
      .mockResolvedValueOnce([clip(2), clip(1)]);
    const p = new Pager(fetch, 2);
    await p.reset(filters);
    await p.loadMore();
    expect(p.items.map((c) => c.id)).toEqual([3, 2, 1]);
  });

  it("refresh keeps as many rows as are loaded (up to 200) for the same filters", async () => {
    const fetch = store(500);
    const p = new Pager(fetch, 50);
    await p.reset(filters);
    await p.loadMore();
    await p.loadMore();
    await p.refresh();
    expect(fetch).toHaveBeenLastCalledWith(expect.objectContaining({ limit: 150, before: null }));
    expect(p.items).toHaveLength(150);
    for (let i = 0; i < 3; i++) await p.loadMore();
    await p.refresh();
    expect(fetch).toHaveBeenLastCalledWith(expect.objectContaining({ limit: 200 }));
  });

  it("records errors and stops loading", async () => {
    const fetch = vi.fn<(q: Query) => Promise<ClipSummary[]>>().mockRejectedValue(new Error("db busy"));
    const p = new Pager(fetch, 10);
    await p.reset(filters);
    expect(p.error).toBe("db busy");
    expect(p.loading).toBe(false);
    expect(p.items).toEqual([]);
  });

  it("removes and patches rows locally", async () => {
    const p = new Pager(store(5), 10);
    await p.reset(filters);
    p.remove([4, 2]);
    expect(p.items.map((c) => c.id)).toEqual([5, 3, 1]);
    p.patch(3, { title: "T" });
    expect(p.items.find((c) => c.id === 3)?.title).toBe("T");
  });

  it("notifies on every state change", async () => {
    const onChange = vi.fn();
    const p = new Pager(store(5), 10, onChange);
    await p.reset(filters);
    // loading=true, then results
    expect(onChange.mock.calls.length).toBeGreaterThanOrEqual(2);
    onChange.mockClear();
    p.remove([1]);
    expect(onChange).toHaveBeenCalledTimes(1);
  });

  it("wants more when the window end nears the loaded tail", async () => {
    const p = new Pager(store(100), 20);
    await p.reset(filters);
    expect(p.wantsMore(5)).toBe(false);
    expect(p.wantsMore(15)).toBe(true);
  });
});
