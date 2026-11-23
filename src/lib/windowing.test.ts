import { describe, expect, it } from "vitest";
import { computeWindow, contentWidth, firstVisible, offsetOf, revealScroll, type Layout } from "./windowing";

const L: Layout = { item: 226, gap: 14, pad: 20 };
const stride = 240;

describe("geometry", () => {
  it("offsetOf places items after the padding at a fixed stride", () => {
    expect(offsetOf(0, L)).toBe(20);
    expect(offsetOf(3, L)).toBe(20 + 3 * stride);
  });

  it("contentWidth includes both paddings and gaps between items only", () => {
    expect(contentWidth(0, L)).toBe(40);
    expect(contentWidth(1, L)).toBe(40 + 226);
    expect(contentWidth(3, L)).toBe(40 + 3 * 226 + 2 * 14);
  });
});

describe("computeWindow", () => {
  it("renders only what intersects the viewport plus overscan", () => {
    // Viewport 1000px at scroll 0 shows items 0..4 (item 4 partially).
    expect(computeWindow(0, 1000, 100, L, 2)).toEqual({ start: 0, end: 7 });
  });

  it("follows the scroll position", () => {
    const w = computeWindow(stride * 10, 1000, 100, L, 2);
    // Item 9 (2180..2406) still peeks in at the left edge.
    expect(w.start).toBe(7);
    expect(w.end).toBe(17);
  });

  it("clamps to the item count", () => {
    expect(computeWindow(0, 1000, 3, L, 2)).toEqual({ start: 0, end: 3 });
    expect(computeWindow(0, 1000, 0, L, 2)).toEqual({ start: 0, end: 0 });
    expect(computeWindow(stride * 98, 1000, 100, L, 2).end).toBe(100);
  });

  it("renders a first screenful before the viewport is measured", () => {
    expect(computeWindow(0, 0, 100, L, 2).end).toBeGreaterThan(0);
  });
});

describe("firstVisible", () => {
  it("is the first item fully inside the viewport", () => {
    expect(firstVisible(0, 1000, 100, L)).toBe(0);
    expect(firstVisible(30, 1000, 100, L)).toBe(1);
    expect(firstVisible(stride * 5 + 20, 1000, 100, L)).toBe(5);
    expect(firstVisible(0, 1000, 0, L)).toBe(0);
  });
});

describe("revealScroll", () => {
  it("leaves the scroll alone when the item is fully visible", () => {
    expect(revealScroll(2, 0, 1000, L)).toBe(0);
  });

  it("scrolls right just enough to show an item past the right edge, keeping the padding", () => {
    // item 5 spans 1220..1446; viewport 1000 → scroll 1446 + 20 - 1000
    expect(revealScroll(5, 0, 1000, L)).toBe(466);
  });

  it("scrolls left to show an item before the left edge", () => {
    expect(revealScroll(1, 2000, 1000, L)).toBe(offsetOf(1, L) - 20);
    expect(revealScroll(0, 2000, 1000, L)).toBe(0);
  });
});
