import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { debounce } from "./debounce";

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

describe("debounce", () => {
  it("runs once with the latest arguments after the quiet period", () => {
    const fn = vi.fn();
    const d = debounce(fn, 40);
    d("a");
    vi.advanceTimersByTime(20);
    d("ab");
    vi.advanceTimersByTime(39);
    expect(fn).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(fn).toHaveBeenCalledExactlyOnceWith("ab");
  });

  it("cancel drops the pending call and leaves no timer behind", () => {
    const fn = vi.fn();
    const d = debounce(fn, 40);
    d("a");
    d.cancel();
    expect(vi.getTimerCount()).toBe(0);
    vi.advanceTimersByTime(100);
    expect(fn).not.toHaveBeenCalled();
  });

  it("flush runs the pending call now", () => {
    const fn = vi.fn();
    const d = debounce(fn, 40);
    d("x");
    d.flush();
    expect(fn).toHaveBeenCalledExactlyOnceWith("x");
    expect(vi.getTimerCount()).toBe(0);
    d.flush();
    expect(fn).toHaveBeenCalledTimes(1);
  });

  it("reports whether a call is pending", () => {
    const d = debounce(() => {}, 40);
    expect(d.pending()).toBe(false);
    d();
    expect(d.pending()).toBe(true);
    vi.runAllTimers();
    expect(d.pending()).toBe(false);
  });
});
