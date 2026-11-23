import { describe, expect, it } from "vitest";
import * as sel from "./selection";

const order = [10, 20, 30, 40, 50];

describe("selection", () => {
  it("starts empty", () => {
    expect(sel.empty()).toEqual({ ids: [], anchor: null, focus: null });
  });

  it("single replaces everything", () => {
    expect(sel.single(30)).toEqual({ ids: [30], anchor: 30, focus: 30 });
  });

  it("toggle adds and removes one clip, keeping the rest", () => {
    let s = sel.single(10);
    s = sel.toggle(s, 30);
    expect(s).toEqual({ ids: [10, 30], anchor: 30, focus: 30 });
    s = sel.toggle(s, 10);
    expect(s).toEqual({ ids: [30], anchor: 30, focus: 30 });
    s = sel.toggle(s, 30);
    expect(s.ids).toEqual([]);
    expect(s.focus).toBeNull();
  });

  it("range selects from the anchor to the clicked clip, either direction", () => {
    let s = sel.single(20);
    s = sel.range(s, 40, order);
    expect(s).toEqual({ ids: [20, 30, 40], anchor: 20, focus: 40 });
    s = sel.range(s, 10, order);
    expect(s.ids).toEqual([20, 10]);
    expect(s.anchor).toBe(20);
  });

  it("range without an anchor acts like single", () => {
    expect(sel.range(sel.empty(), 30, order)).toEqual(sel.single(30));
  });

  it("move steps the focus and clamps at the ends", () => {
    let s = sel.single(20);
    s = sel.move(s, 1, order);
    expect(s).toEqual(sel.single(30));
    s = sel.move(s, 10, order);
    expect(s).toEqual(sel.single(50));
    s = sel.move(s, -99, order);
    expect(s).toEqual(sel.single(10));
  });

  it("move from nothing selects the first clip", () => {
    expect(sel.move(sel.empty(), 1, order)).toEqual(sel.single(10));
    expect(sel.move(sel.empty(), 1, [])).toEqual(sel.empty());
  });

  it("move with extend grows a range from the anchor", () => {
    let s = sel.single(20);
    s = sel.move(s, 1, order, true);
    s = sel.move(s, 1, order, true);
    expect(s).toEqual({ ids: [20, 30, 40], anchor: 20, focus: 40 });
  });

  it("move collapses a multi-selection", () => {
    const s = sel.move({ ids: [10, 30], anchor: 30, focus: 30 }, -1, order);
    expect(s).toEqual(sel.single(20));
  });

  it("all selects every clip", () => {
    expect(sel.all(order, 30)).toEqual({ ids: order, anchor: 10, focus: 30 });
    expect(sel.all([], null)).toEqual(sel.empty());
  });

  it("ordered returns selected ids in list order", () => {
    expect(sel.ordered({ ids: [40, 10, 30], anchor: 40, focus: 30 }, order)).toEqual([10, 30, 40]);
  });

  it("prune drops vanished ids and falls back to the first clip", () => {
    expect(sel.prune({ ids: [20, 60], anchor: 60, focus: 60 }, order)).toEqual({ ids: [20], anchor: 20, focus: 20 });
    expect(sel.prune(sel.single(99), order)).toEqual(sel.single(10));
    expect(sel.prune(sel.single(99), [])).toEqual(sel.empty());
    const kept = { ids: [20, 30], anchor: 20, focus: 30 };
    expect(sel.prune(kept, order)).toBe(kept);
  });

  it("afterRemoval selects the clip that takes the first removed clip's place", () => {
    const s = { ids: [30, 40], anchor: 30, focus: 40 };
    expect(sel.afterRemoval(s, [30, 40], order)).toEqual(sel.single(50));
    expect(sel.afterRemoval(sel.single(50), [50], order)).toEqual(sel.single(40));
    expect(sel.afterRemoval(sel.single(10), [10], [10])).toEqual(sel.empty());
  });

  it("isSelected", () => {
    const s = { ids: [10, 30], anchor: 10, focus: 30 };
    expect(sel.isSelected(s, 30)).toBe(true);
    expect(sel.isSelected(s, 20)).toBe(false);
  });
});
