import { describe, expect, it } from "vitest";
import { failureText, percent, shelfUpdate, updateLine } from "./updates";
import type { UpdatePhase, UpdateStatus } from "./types";

const status = (phase: UpdatePhase, more: Partial<UpdateStatus> = {}): UpdateStatus => ({
  supported: true,
  currentVersion: "0.1.1",
  checking: false,
  phase,
  lastCheck: null,
  ...more,
});
const now = new Date(2026, 10, 24, 12, 0).getTime();
const MIN = 60_000;

describe("percent", () => {
  it("is whole, capped and unknown without a size", () => {
    expect(percent({ state: "downloading", version: "1", downloaded: 421, total: 1000 })).toBe(42);
    expect(percent({ state: "downloading", version: "1", downloaded: 2000, total: 1000 })).toBe(100);
    expect(percent({ state: "downloading", version: "1", downloaded: 5, total: null })).toBeNull();
    expect(percent({ state: "available", version: "1" })).toBeNull();
  });
});

describe("shelfUpdate", () => {
  it("shows nothing without an offer, or in Satchel Dev", () => {
    expect(shelfUpdate(null)).toBeNull();
    expect(shelfUpdate(status({ state: "idle" }, { checking: true }))).toBeNull();
    expect(shelfUpdate(status({ state: "available", version: "0.2.0" }, { supported: false }))).toBeNull();
  });

  it("offers the update, shows progress, and lets a failure be retried", () => {
    const offer = shelfUpdate(status({ state: "available", version: "0.2.0" }), "Satchel")!;
    expect(offer).toMatchObject({ label: "Update to 0.2.0", aria: "Update to Satchel 0.2.0", actionable: true, tone: "offer" });
    const busy = shelfUpdate(status({ state: "downloading", version: "0.2.0", downloaded: 50, total: 200 }), "Satchel")!;
    expect(busy).toMatchObject({ label: "Updating… 25%", actionable: false, tone: "busy", progress: 25 });
    expect(shelfUpdate(status({ state: "installing", version: "0.2.0" }))).toMatchObject({ label: "Installing…", actionable: false });
    const failed = shelfUpdate(status({ state: "failed", version: "0.2.0", during: "verify" }), "Satchel")!;
    expect(failed).toMatchObject({ label: "Update Failed", actionable: true, tone: "failed" });
    expect(failed.title).toContain("couldn’t be verified");
  });
});

describe("updateLine", () => {
  it("says where things stand, phase first", () => {
    expect(updateLine(status({ state: "idle" }), now, "Satchel")).toBe("Not checked yet.");
    expect(updateLine(status({ state: "idle" }, { checking: true }), now)).toBe("Checking for updates…");
    expect(updateLine(status({ state: "idle" }, { lastCheck: { at: now - 5 * MIN, outcome: "upToDate" } }), now, "Satchel")).toBe(
      "Satchel is up to date (checked 5 min ago).",
    );
    expect(updateLine(status({ state: "idle" }, { lastCheck: { at: now - 10, outcome: "upToDate" } }), now, "Satchel")).toBe(
      "Satchel is up to date (checked just now).",
    );
    expect(updateLine(status({ state: "idle" }, { lastCheck: { at: now - 30 * MIN, outcome: "failed" } }), now)).toBe(
      "Couldn’t check for updates (30 min ago). You may be offline.",
    );
    expect(
      updateLine(status({ state: "available", version: "0.2.0" }, { checking: true }), now, "Satchel"),
    ).toBe("Satchel 0.2.0 is available.");
    expect(updateLine(status({ state: "downloading", version: "0.2.0", downloaded: 1, total: 4 }), now, "Satchel")).toBe(
      "Downloading Satchel 0.2.0… 25%",
    );
    expect(updateLine(status({ state: "failed", version: "0.2.0", during: "download" }), now, "Satchel")).toBe(
      failureText("download", "0.2.0", "Satchel"),
    );
  });

  it("explains that Satchel Dev does not update", () => {
    expect(updateLine(status({ state: "idle" }, { supported: false }), now, "Satchel Dev")).toBe(
      "Satchel Dev doesn’t update itself. The installed Satchel does.",
    );
  });

  it("uses a date for older checks", () => {
    const old = new Date(2026, 10, 2, 9, 0).getTime();
    expect(updateLine(status({ state: "idle" }, { lastCheck: { at: old, outcome: "upToDate" } }), now, "Satchel")).toMatch(
      /^Satchel is up to date \(checked on .+\)\.$/,
    );
  });
});
