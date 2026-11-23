import { describe, expect, it, vi } from "vitest";
import { ThumbnailCache } from "./thumbs";

function setup() {
  let n = 0;
  const resolvers = new Map<number, (b: Uint8Array) => void>();
  const rejecters = new Map<number, (e: Error) => void>();
  const fetch = vi.fn(
    (id: number) =>
      new Promise<Uint8Array>((res, rej) => {
        resolvers.set(id, res);
        rejecters.set(id, rej);
      }),
  );
  const createUrl = vi.fn((_b: Blob) => `blob:${++n}`);
  const revoke = vi.fn();
  const cache = new ThumbnailCache({ fetch, createUrl, revoke });
  const flush = () => new Promise((r) => setTimeout(r, 0));
  return { cache, fetch, createUrl, revoke, resolvers, rejecters, flush };
}

describe("ThumbnailCache", () => {
  it("fetches once per clip and shares the object URL between holders", async () => {
    const { cache, fetch, resolvers, createUrl } = setup();
    const a = cache.acquire(1);
    const b = cache.acquire(1);
    resolvers.get(1)!(new Uint8Array([1]));
    expect(await a).toBe("blob:1");
    expect(await b).toBe("blob:1");
    expect(fetch).toHaveBeenCalledTimes(1);
    expect(createUrl).toHaveBeenCalledTimes(1);
    expect(createUrl.mock.calls[0][0].type).toBe("image/png");
  });

  it("revokes the URL when the last holder releases it", async () => {
    const { cache, resolvers, revoke } = setup();
    const a = cache.acquire(1);
    cache.acquire(1);
    resolvers.get(1)!(new Uint8Array([1]));
    await a;
    cache.release(1);
    expect(revoke).not.toHaveBeenCalled();
    cache.release(1);
    expect(revoke).toHaveBeenCalledWith("blob:1");
    expect(cache.size).toBe(0);
  });

  it("revokes a URL that arrives after everyone let go", async () => {
    const { cache, resolvers, revoke, flush } = setup();
    const a = cache.acquire(1);
    cache.release(1);
    resolvers.get(1)!(new Uint8Array([1]));
    expect(await a).toBeNull();
    await flush();
    expect(revoke).toHaveBeenCalledWith("blob:1");
    expect(cache.size).toBe(0);
  });

  it("refetches after a full release", async () => {
    const { cache, fetch, resolvers } = setup();
    const a = cache.acquire(1);
    resolvers.get(1)!(new Uint8Array([1]));
    await a;
    cache.release(1);
    const b = cache.acquire(1);
    resolvers.get(1)!(new Uint8Array([1]));
    expect(await b).toBe("blob:2");
    expect(fetch).toHaveBeenCalledTimes(2);
  });

  it("resolves null on fetch errors and forgets the entry", async () => {
    const { cache, rejecters, revoke } = setup();
    const a = cache.acquire(1);
    rejecters.get(1)!(new Error("gone"));
    expect(await a).toBeNull();
    cache.release(1);
    expect(revoke).not.toHaveBeenCalled();
    expect(cache.size).toBe(0);
  });

  it("ignores releases of unknown ids", () => {
    const { cache } = setup();
    expect(() => cache.release(42)).not.toThrow();
  });

  it("clear revokes everything", async () => {
    const { cache, resolvers, revoke } = setup();
    const a = cache.acquire(1);
    const b = cache.acquire(2);
    resolvers.get(1)!(new Uint8Array([1]));
    resolvers.get(2)!(new Uint8Array([2]));
    await Promise.all([a, b]);
    cache.clear();
    expect(revoke).toHaveBeenCalledTimes(2);
    expect(cache.size).toBe(0);
  });
});
