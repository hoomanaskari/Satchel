import { describe, expect, it, vi } from "vitest";
import { AppIconCache, iconSlot } from "./app-icons";

function setup(fetchImpl: (id: string) => Promise<Uint8Array>) {
  let n = 0;
  const fetch = vi.fn(fetchImpl);
  const createUrl = vi.fn((_b: Blob) => `blob:${++n}`);
  const revoke = vi.fn();
  return { cache: new AppIconCache({ fetch, createUrl, revoke }), fetch, createUrl, revoke };
}

describe("AppIconCache", () => {
  it("fetches each app once, whatever the case of its id", async () => {
    const { cache, fetch } = setup(async () => new Uint8Array([137, 80]));
    const [a, b] = await Promise.all([cache.get("com.apple.Notes"), cache.get("COM.APPLE.NOTES")]);
    expect(a).toBe("blob:1");
    expect(b).toBe("blob:1");
    expect(fetch).toHaveBeenCalledTimes(1);
  });

  it("gives null for apps without an icon and does not ask again", async () => {
    const { cache, fetch, createUrl } = setup(async () => {
      throw new Error("not found");
    });
    expect(await cache.get("x.y")).toBeNull();
    expect(await cache.get("x.y")).toBeNull();
    expect(fetch).toHaveBeenCalledTimes(1);
    expect(createUrl).not.toHaveBeenCalled();
  });

  it("treats empty bytes as no icon", async () => {
    const { cache } = setup(async () => new Uint8Array());
    expect(await cache.get("x.y")).toBeNull();
  });

  it("revokes every URL on clear and fetches again afterwards", async () => {
    const { cache, fetch, revoke } = setup(async () => new Uint8Array([1]));
    await cache.get("a.b");
    await cache.get("c.d");
    cache.clear();
    expect(revoke.mock.calls.map((c) => c[0])).toEqual(["blob:1", "blob:2"]);
    await cache.get("a.b");
    expect(fetch).toHaveBeenCalledTimes(3);
  });
});

describe("AppIconCache.peek", () => {
  it("is unknown until the fetch settles, then answers synchronously", async () => {
    const { cache } = setup(async (id) => (id === "x.y" ? new Uint8Array() : new Uint8Array([1])));
    expect(cache.peek("a.b")).toBeUndefined();
    const p = cache.get("a.b");
    expect(cache.peek("A.B")).toBeUndefined();
    await p;
    expect(cache.peek("A.B")).toBe("blob:1");
    await cache.get("x.y");
    expect(cache.peek("x.y")).toBeNull();
  });

  it("does not keep an icon that arrives after clear", async () => {
    let done!: (b: Uint8Array) => void;
    const { cache, revoke } = setup(() => new Promise((r) => (done = r)));
    const p = cache.get("a.b");
    cache.clear();
    done(new Uint8Array([1]));
    expect(await p).toBe("blob:1");
    expect(cache.peek("a.b")).toBeUndefined();
    expect(revoke).toHaveBeenCalledWith("blob:1");
  });
});

describe("iconSlot", () => {
  it("falls back to the lettered tile without an app, a cache or an icon", async () => {
    const { cache } = setup(async () => {
      throw new Error("not found");
    });
    expect(iconSlot(cache, null, null)).toBeNull();
    expect(iconSlot(cache, "  ", null)).toBeNull();
    expect(iconSlot(null, "com.apple.Notes", null)).toBeNull();
    await cache.get("gone.app");
    expect(iconSlot(cache, "gone.app", null)).toBeNull();
  });

  it("is blank while loading and shows a cached icon at once", async () => {
    const { cache } = setup(async () => new Uint8Array([1]));
    expect(iconSlot(cache, "com.apple.Notes", null)).toBeUndefined();
    await cache.get("com.apple.Notes");
    // A card mounted later (scrolled back, list re-rendered, shelf reopened) starts with it.
    expect(iconSlot(cache, "com.apple.notes", null)).toBe("blob:1");
  });

  it("uses the slot's own load only for the id it was started for", () => {
    const { cache } = setup(async () => new Uint8Array([1]));
    const loaded = { id: "a.b", url: "blob:9" };
    expect(iconSlot(cache, "A.B", loaded)).toBe("blob:9");
    expect(iconSlot(cache, "c.d", loaded)).toBeUndefined();
    expect(iconSlot(cache, "c.d", { id: "c.d", url: null })).toBeNull();
  });
});
