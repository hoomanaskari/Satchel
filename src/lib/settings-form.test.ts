import { describe, expect, it } from "vitest";
import {
  acceleratorFromEvent,
  formatAccelerator,
  isValidAccelerator,
  normalizeExcluded,
  retentionOptions,
  validateSettings,
} from "./settings-form";
import type { Settings } from "./types";

const base: Settings = {
  maxItems: 2000,
  retentionDays: 90,
  excludedAppIds: ["com.1password.1password"],
  capturePaused: false,
  captureImages: true,
  shelfShortcut: "CommandOrControl+Shift+V",
  pasteOnSelect: true,
};

type Ev = Parameters<typeof acceleratorFromEvent>[0];
const ev = (code: string, key: string, m: Partial<Ev> = {}): Ev => ({
  code,
  key,
  metaKey: false,
  ctrlKey: false,
  altKey: false,
  shiftKey: false,
  ...m,
});

describe("acceleratorFromEvent", () => {
  it("builds Tauri accelerators, Command mapped to CommandOrControl on macOS", () => {
    expect(acceleratorFromEvent(ev("KeyV", "V", { metaKey: true, shiftKey: true }), true)).toBe(
      "CommandOrControl+Shift+V",
    );
    expect(acceleratorFromEvent(ev("KeyV", "v", { ctrlKey: true, altKey: true }), true)).toBe("Control+Alt+V");
  });

  it("maps Ctrl to CommandOrControl and the Windows key to Super elsewhere", () => {
    expect(acceleratorFromEvent(ev("Digit2", "2", { ctrlKey: true }), false)).toBe("CommandOrControl+2");
    expect(acceleratorFromEvent(ev("Space", " ", { metaKey: true }), false)).toBe("Super+Space");
  });

  it("names special keys", () => {
    expect(acceleratorFromEvent(ev("ArrowUp", "ArrowUp", { metaKey: true }), true)).toBe("CommandOrControl+Up");
    expect(acceleratorFromEvent(ev("Comma", ",", { metaKey: true }), true)).toBe("CommandOrControl+Comma");
    expect(acceleratorFromEvent(ev("F5", "F5"), true)).toBe("F5");
  });

  it("returns null until a key with a modifier is pressed", () => {
    expect(acceleratorFromEvent(ev("ShiftLeft", "Shift", { shiftKey: true }), true)).toBeNull();
    expect(acceleratorFromEvent(ev("KeyV", "v"), true)).toBeNull();
    expect(acceleratorFromEvent(ev("KeyV", "V", { shiftKey: true }), true)).toBeNull();
    expect(acceleratorFromEvent(ev("IntlRo", "ろ", { metaKey: true }), true)).toBeNull();
  });
});

describe("formatAccelerator", () => {
  it("uses macOS symbols in the standard order", () => {
    expect(formatAccelerator("CommandOrControl+Shift+V", true)).toBe("⇧⌘V");
    expect(formatAccelerator("Control+Alt+Shift+Super+K", true)).toBe("⌃⌥⇧⌘K");
    expect(formatAccelerator("CommandOrControl+Space", true)).toBe("⌘Space");
  });

  it("spells modifiers out elsewhere", () => {
    expect(formatAccelerator("CommandOrControl+Shift+V", false)).toBe("Ctrl+Shift+V");
    expect(formatAccelerator("Super+Alt+Up", false)).toBe("Win+Alt+Up");
  });
});

describe("isValidAccelerator", () => {
  it.each(["CommandOrControl+Shift+V", "Alt+Space", "F13", "Control+F2"])("accepts %s", (a) =>
    expect(isValidAccelerator(a)).toBe(true),
  );
  it.each(["", "V", "Shift+V", "CommandOrControl+", "CommandOrControl+Shift", "Foo+V", "CommandOrControl+V+X"])(
    "rejects %s",
    (a) => expect(isValidAccelerator(a)).toBe(false),
  );
});

describe("validateSettings", () => {
  it("accepts the defaults", () => {
    expect(validateSettings(base)).toEqual({});
  });

  it("flags bad numbers and shortcuts", () => {
    expect(validateSettings({ ...base, maxItems: 0 })).toHaveProperty("maxItems");
    expect(validateSettings({ ...base, maxItems: 2.5 })).toHaveProperty("maxItems");
    expect(validateSettings({ ...base, maxItems: 1_000_000 })).toHaveProperty("maxItems");
    expect(validateSettings({ ...base, retentionDays: 0 })).toHaveProperty("retentionDays");
    expect(validateSettings({ ...base, retentionDays: null })).toEqual({});
    expect(validateSettings({ ...base, shelfShortcut: "V" })).toHaveProperty("shelfShortcut");
  });
});

describe("normalizeExcluded", () => {
  it("trims, drops blanks and duplicates, keeps order", () => {
    expect(normalizeExcluded([" a ", "", "b", "a", "  "])).toEqual(["a", "b"]);
  });
});

describe("retentionOptions", () => {
  it("offers presets plus Forever, and keeps a custom current value", () => {
    const opts = retentionOptions(90);
    expect(opts.map((o) => o.value)).toEqual([1, 7, 30, 90, 365, null]);
    expect(opts.at(-1)!.label).toBe("Forever");
    expect(retentionOptions(45).map((o) => o.value)).toEqual([1, 7, 30, 45, 90, 365, null]);
    expect(retentionOptions(45).find((o) => o.value === 45)!.label).toBe("45 days");
  });
});
