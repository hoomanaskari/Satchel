import { describe, expect, it } from "vitest";
import { closesSettingsWindow, mapKey, type KeyContext, type KeyInput } from "./keyboard";

function k(key: string, mods: Partial<KeyInput> = {}): KeyInput {
  return { key, code: "", metaKey: false, ctrlKey: false, shiftKey: false, altKey: false, ...mods };
}
const mac: KeyContext = { mac: true, focus: "search", searchEmpty: true, hasTextSelection: false, previewOpen: false };
const win: KeyContext = { ...mac, mac: false };

describe("navigation", () => {
  it("Left/Right move the selection", () => {
    expect(mapKey(k("ArrowLeft"), mac)).toEqual({ type: "move", delta: -1, extend: false });
    expect(mapKey(k("ArrowRight"), mac)).toEqual({ type: "move", delta: 1, extend: false });
  });

  it("Shift+Left/Right extend the selection, unless editing a non-empty search", () => {
    expect(mapKey(k("ArrowRight", { shiftKey: true }), mac)).toEqual({ type: "move", delta: 1, extend: true });
    expect(mapKey(k("ArrowRight", { shiftKey: true }), { ...mac, searchEmpty: false })).toBeNull();
  });

  it("arrows still move cards while a search is typed", () => {
    expect(mapKey(k("ArrowLeft"), { ...mac, searchEmpty: false })).toEqual({ type: "move", delta: -1, extend: false });
  });

  it("Home/End jump when the search field is not focused", () => {
    expect(mapKey(k("Home"), { ...mac, focus: "other" })).toEqual({ type: "edge", end: false });
    expect(mapKey(k("End"), { ...mac, focus: "other" })).toEqual({ type: "edge", end: true });
    expect(mapKey(k("End"), mac)).toBeNull();
  });
});

describe("pasting and copying", () => {
  it("Return pastes, Shift+Return pastes as plain text", () => {
    expect(mapKey(k("Enter"), mac)).toEqual({ type: "paste", plain: false });
    expect(mapKey(k("Enter", { shiftKey: true }), mac)).toEqual({ type: "paste", plain: true });
  });

  it("Cmd+1…9 (Ctrl elsewhere) pastes the nth visible clip", () => {
    expect(mapKey(k("1", { metaKey: true, code: "Digit1" }), mac)).toEqual({ type: "pasteNth", n: 1, plain: false });
    expect(mapKey(k("9", { ctrlKey: true, code: "Digit9" }), win)).toEqual({ type: "pasteNth", n: 9, plain: false });
    // Shift changes the key on most layouts; the code still identifies the digit.
    expect(mapKey(k("!", { metaKey: true, shiftKey: true, code: "Digit1" }), mac)).toEqual({
      type: "pasteNth",
      n: 1,
      plain: true,
    });
    expect(mapKey(k("0", { metaKey: true, code: "Digit0" }), mac)).toBeNull();
    expect(mapKey(k("1", { ctrlKey: true, code: "Digit1" }), mac)).toBeNull();
    expect(mapKey(k("1", { metaKey: true, code: "Digit1" }), win)).toBeNull();
  });

  it("Cmd+C copies the clip unless text is selected in a field", () => {
    expect(mapKey(k("c", { metaKey: true, code: "KeyC" }), mac)).toEqual({ type: "copy", plain: false });
    expect(mapKey(k("C", { metaKey: true, shiftKey: true, code: "KeyC" }), mac)).toEqual({ type: "copy", plain: true });
    expect(mapKey(k("c", { metaKey: true, code: "KeyC" }), { ...mac, hasTextSelection: true })).toBeNull();
    expect(mapKey(k("c", { ctrlKey: true, code: "KeyC" }), win)).toEqual({ type: "copy", plain: false });
  });
});

describe("editing clips", () => {
  it("Cmd+P pins", () => {
    expect(mapKey(k("p", { metaKey: true, code: "KeyP" }), mac)).toEqual({ type: "pin" });
    expect(mapKey(k("p", { ctrlKey: true, code: "KeyP" }), win)).toEqual({ type: "pin" });
  });

  it("Cmd+Backspace deletes, but edits a non-empty search as usual", () => {
    expect(mapKey(k("Backspace", { metaKey: true }), mac)).toEqual({ type: "delete" });
    expect(mapKey(k("Delete", { ctrlKey: true }), win)).toEqual({ type: "delete" });
    expect(mapKey(k("Backspace", { metaKey: true }), { ...mac, searchEmpty: false })).toBeNull();
    expect(mapKey(k("Backspace", { metaKey: true }), { ...mac, focus: "other", searchEmpty: false })).toEqual({
      type: "delete",
    });
  });

  it("Cmd+A selects all clips unless there is search text to select", () => {
    expect(mapKey(k("a", { metaKey: true, code: "KeyA" }), mac)).toEqual({ type: "selectAll" });
    expect(mapKey(k("a", { metaKey: true, code: "KeyA" }), { ...mac, searchEmpty: false })).toBeNull();
  });
});

describe("preview and closing", () => {
  it("Space toggles the preview when it would not type into the search", () => {
    expect(mapKey(k(" "), mac)).toEqual({ type: "preview" });
    expect(mapKey(k(" "), { ...mac, focus: "other", searchEmpty: false })).toEqual({ type: "preview" });
    expect(mapKey(k(" "), { ...mac, searchEmpty: false })).toBeNull();
    expect(mapKey(k(" "), { ...mac, searchEmpty: false, previewOpen: true })).toEqual({ type: "preview" });
  });

  it("Escape closes the preview first, then the shelf", () => {
    expect(mapKey(k("Escape"), { ...mac, previewOpen: true })).toEqual({ type: "closePreview" });
    expect(mapKey(k("Escape"), mac)).toEqual({ type: "close" });
  });

  it("Cmd+F focuses the search", () => {
    expect(mapKey(k("f", { metaKey: true, code: "KeyF" }), { ...mac, focus: "other" })).toEqual({ type: "focusSearch" });
  });
});

describe("type to search", () => {
  it("printable keys outside a text field go to the search", () => {
    expect(mapKey(k("a"), { ...mac, focus: "other" })).toEqual({ type: "typeToSearch" });
    expect(mapKey(k("A", { shiftKey: true }), { ...mac, focus: "other" })).toEqual({ type: "typeToSearch" });
    expect(mapKey(k("a"), mac)).toBeNull();
    expect(mapKey(k("j", { metaKey: true }), { ...mac, focus: "other" })).toBeNull();
  });
});

describe("ignored input", () => {
  it("leaves other text fields (rename, settings) alone entirely", () => {
    const ctx: KeyContext = { ...mac, focus: "text" };
    for (const key of ["Enter", "Escape", " ", "ArrowLeft", "a"]) expect(mapKey(k(key), ctx)).toBeNull();
    expect(mapKey(k("Backspace", { metaKey: true }), ctx)).toBeNull();
  });

  it("ignores IME composition", () => {
    expect(mapKey(k("Enter", { isComposing: true }), mac)).toBeNull();
  });

  it("ignores Alt combinations and unknown keys", () => {
    expect(mapKey(k("Enter", { altKey: true }), mac)).toBeNull();
    expect(mapKey(k("Tab"), mac)).toBeNull();
    expect(mapKey(k("F5"), mac)).toBeNull();
  });
});

describe("focused controls", () => {
  it("lets a focused button (tab, scope, footer) handle Return and Space itself", () => {
    const ctx: KeyContext = { ...mac, focus: "other", onControl: true };
    expect(mapKey(k("Enter"), ctx)).toBeNull();
    expect(mapKey(k(" "), ctx)).toBeNull();
    expect(mapKey(k("ArrowRight"), ctx)).toEqual({ type: "move", delta: 1, extend: false });
    expect(mapKey(k("Escape"), ctx)).toEqual({ type: "close" });
  });
});

describe("settings", () => {
  it("Cmd+, opens settings", () => {
    expect(mapKey(k(",", { metaKey: true, code: "Comma" }), mac)).toEqual({ type: "settings" });
    expect(mapKey(k(",", { ctrlKey: true, code: "Comma" }), win)).toEqual({ type: "settings" });
  });
});

describe("onControl only applies outside text fields", () => {
  it("still pastes and previews from the search field", () => {
    const ctx: KeyContext = { ...mac, focus: "search", onControl: true };
    expect(mapKey(k("Enter"), ctx)).toEqual({ type: "paste", plain: false });
    expect(mapKey(k(" "), ctx)).toEqual({ type: "preview" });
  });
});

describe("settings window", () => {
  it("Escape and Cmd+W close it on macOS", () => {
    expect(closesSettingsWindow(k("Escape"), true)).toBe(true);
    expect(closesSettingsWindow(k("w", { metaKey: true, code: "KeyW" }), true)).toBe(true);
    expect(closesSettingsWindow(k("w", { ctrlKey: true, code: "KeyW" }), true)).toBe(false);
  });

  it("Ctrl+W closes it elsewhere", () => {
    expect(closesSettingsWindow(k("w", { ctrlKey: true, code: "KeyW" }), false)).toBe(true);
    expect(closesSettingsWindow(k("w", { metaKey: true, code: "KeyW" }), false)).toBe(false);
  });

  it("ignores plain typing, other modifiers and IME composition", () => {
    expect(closesSettingsWindow(k("w"), true)).toBe(false);
    expect(closesSettingsWindow(k("W", { metaKey: true, shiftKey: true, code: "KeyW" }), true)).toBe(false);
    expect(closesSettingsWindow(k("Escape", { shiftKey: true }), true)).toBe(false);
    expect(closesSettingsWindow(k("Escape", { isComposing: true }), true)).toBe(false);
  });
});
