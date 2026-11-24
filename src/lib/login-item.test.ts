import { describe, expect, it } from "vitest";
import { loginItemHint } from "./login-item";
import type { LoginItem } from "./types";

const item = (more: Partial<LoginItem> = {}): LoginItem => ({ enabled: true, requiresApproval: false, unavailable: null, ...more });

describe("loginItemHint", () => {
  it("says where to find it when offered", () => {
    expect(loginItemHint(item(), "Satchel")).toContain("Login Items");
    expect(loginItemHint(item({ enabled: false }), "Satchel")).toContain("Starts quietly");
  });
  it("asks for approval when macOS waits for it", () => {
    expect(loginItemHint(item({ requiresApproval: true }), "Satchel")).toMatch(/^Waiting for you to allow Satchel/);
  });
  it("explains why it is not offered", () => {
    expect(loginItemHint(item({ unavailable: "dev" }), "Satchel Dev")).toContain("Satchel Dev never does");
    expect(loginItemHint(item({ unavailable: "macos" }), "Satchel")).toContain("macOS 13");
    expect(loginItemHint(item({ unavailable: "location" }), "Satchel")).toContain("Applications folder");
  });
});
