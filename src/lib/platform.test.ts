import { afterEach, describe, expect, it } from "vitest";
import { shelfBackground } from "./platform";

const g = globalThis as { __SATCHEL_SHELF_BACKGROUND__?: unknown };

describe("shelfBackground", () => {
  afterEach(() => {
    delete g.__SATCHEL_SHELF_BACKGROUND__;
  });

  it("is none without the native hint (plain browser, mock mode)", () => {
    expect(shelfBackground()).toBe("none");
  });

  it("reads the material Rust announced", () => {
    g.__SATCHEL_SHELF_BACKGROUND__ = "glass";
    expect(shelfBackground()).toBe("glass");
    g.__SATCHEL_SHELF_BACKGROUND__ = "blur";
    expect(shelfBackground()).toBe("blur");
  });

  it("treats unknown values as none", () => {
    g.__SATCHEL_SHELF_BACKGROUND__ = "neon";
    expect(shelfBackground()).toBe("none");
    g.__SATCHEL_SHELF_BACKGROUND__ = 1;
    expect(shelfBackground()).toBe("none");
  });
});
