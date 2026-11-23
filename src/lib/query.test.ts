import { describe, expect, it } from "vitest";
import { buildQuery, cursorOf, KIND_SCOPES, MAX_QUERY_CHARS, type Filters } from "./query";

const f = (over: Partial<Filters> = {}): Filters => ({ text: "", kind: "all", pinboardId: null, ...over });

describe("buildQuery", () => {
  it("maps empty filters to a whole-history query", () => {
    expect(buildQuery(f(), 50)).toEqual({ text: null, kind: null, pinboardId: null, limit: 50, before: null });
  });

  it("trims text and treats whitespace-only as no search", () => {
    expect(buildQuery(f({ text: "  foo bar " }), 50).text).toBe("foo bar");
    expect(buildQuery(f({ text: " \t " }), 50).text).toBeNull();
  });

  it("caps very long search text", () => {
    expect(buildQuery(f({ text: "x".repeat(5000) }), 50).text).toHaveLength(MAX_QUERY_CHARS);
  });

  it("passes kind, pinboard and cursor through", () => {
    expect(buildQuery(f({ kind: "image", pinboardId: 3 }), 20, [100, 7])).toEqual({
      text: null,
      kind: "image",
      pinboardId: 3,
      limit: 20,
      before: [100, 7],
    });
  });

  it("clamps the limit into the store's 1..=200", () => {
    expect(buildQuery(f(), 999).limit).toBe(200);
    expect(buildQuery(f(), 0).limit).toBe(1);
  });
});

describe("cursorOf", () => {
  it("is (lastUsedAt, id) of a clip", () => {
    expect(cursorOf({ lastUsedAt: 123, id: 9 })).toEqual([123, 9]);
  });
});

describe("KIND_SCOPES", () => {
  it("lists All first, then each kind once, in the prototype's order", () => {
    expect(KIND_SCOPES.map((k) => k.id)).toEqual(["all", "text", "link", "code", "image", "color", "file"]);
    expect(KIND_SCOPES.map((k) => k.label)).toEqual(["All", "Text", "Links", "Code", "Images", "Colors", "Files"]);
  });
});
