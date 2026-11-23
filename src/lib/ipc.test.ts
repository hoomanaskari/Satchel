import { afterEach, describe, expect, it, vi } from "vitest";
import * as ipc from "./ipc";
import type { Transport } from "./ipc";
import type { Query, Settings } from "./types";

function fakeTransport(result: unknown = undefined) {
  const invoke = vi.fn(async (_cmd: string, _args?: Record<string, unknown>) => result);
  const unlisten = vi.fn();
  const handlers = new Map<string, (p: unknown) => void>();
  const listen = vi.fn(async (event: string, handler: (p: unknown) => void) => {
    handlers.set(event, handler);
    return unlisten;
  });
  const t: Transport = { invoke, listen };
  return { t, invoke, listen, unlisten, handlers };
}

const query: Query = { text: "a", kind: "code", pinboardId: null, limit: 50, before: [10, 2] };
const settings: Settings = {
  maxItems: 2000,
  retentionDays: 90,
  excludedAppIds: [],
  capturePaused: false,
  captureImages: true,
  shelfShortcut: "CommandOrControl+Shift+V",
  pasteOnSelect: true,
};

afterEach(() => ipc.setTransport(null));

describe("commands", () => {
  const cases: [string, () => Promise<unknown>, string, Record<string, unknown> | undefined][] = [
    ["listClips", () => ipc.listClips(query), "list_clips", { query }],
    ["getClipText", () => ipc.getClipText(4), "get_clip_text", { id: 4 }],
    ["pasteClip", () => ipc.pasteClip(4, true), "paste_clip", { id: 4, plainText: true }],
    ["pasteClips", () => ipc.pasteClips([3, 1], false), "paste_clips", { ids: [3, 1], plainText: false }],
    ["copyClip", () => ipc.copyClip(4, false), "copy_clip", { id: 4, plainText: false }],
    ["deleteClip", () => ipc.deleteClip(4), "delete_clip", { id: 4 }],
    ["setClipTitle", () => ipc.setClipTitle(4, null), "set_clip_title", { id: 4, title: null }],
    ["clearHistory", () => ipc.clearHistory(), "clear_history", undefined],
    ["listPinboards", () => ipc.listPinboards(), "list_pinboards", undefined],
    ["createPinboard", () => ipc.createPinboard("Work", "#FF9F0A"), "create_pinboard", { name: "Work", color: "#FF9F0A" }],
    ["renamePinboard", () => ipc.renamePinboard(2, "Home"), "rename_pinboard", { id: 2, name: "Home" }],
    ["deletePinboard", () => ipc.deletePinboard(2), "delete_pinboard", { id: 2 }],
    ["pinClip", () => ipc.pinClip(4, 2), "pin_clip", { clipId: 4, pinboardId: 2 }],
    ["unpinClip", () => ipc.unpinClip(4, 2), "unpin_clip", { clipId: 4, pinboardId: 2 }],
    ["getSettings", () => ipc.getSettings(), "get_settings", undefined],
    ["saveSettings", () => ipc.saveSettings(settings), "save_settings", { settings }],
    ["setCapturePaused", () => ipc.setCapturePaused(true), "set_capture_paused", { paused: true }],
    ["hideShelf", () => ipc.hideShelf(), "hide_shelf", undefined],
    ["openSettings", () => ipc.openSettings(), "open_settings", undefined],
    ["closeSettings", () => ipc.closeSettings(), "close_settings", undefined],
    ["openAccessibilitySettings", () => ipc.openAccessibilitySettings(), "open_accessibility_settings", undefined],
    ["listApps", () => ipc.listApps(), "list_apps", undefined],
    ["resolveApps", () => ipc.resolveApps(["com.apple.Notes"]), "resolve_apps", { ids: ["com.apple.Notes"] }],
    ["getAppIcon", () => ipc.getAppIcon("com.apple.Notes"), "get_app_icon", { id: "com.apple.Notes" }],
    ["chooseApp", () => ipc.chooseApp(), "choose_app", undefined],
  ];

  it.each(cases)("%s invokes the contract command with camelCase args", async (_n, call, cmd, args) => {
    const { t, invoke } = fakeTransport();
    ipc.setTransport(t);
    await call();
    expect(invoke).toHaveBeenCalledTimes(1);
    expect(invoke).toHaveBeenCalledWith(cmd, args);
  });

  it("returns what the backend returns", async () => {
    const { t } = fakeTransport([{ id: 1 }]);
    ipc.setTransport(t);
    await expect(ipc.listClips(query)).resolves.toEqual([{ id: 1 }]);
  });

  it("turns string rejections into Error objects", async () => {
    ipc.setTransport({
      invoke: () => Promise.reject("store is locked"),
      listen: async () => () => {},
    });
    await expect(ipc.deleteClip(1)).rejects.toThrow("store is locked");
  });
});

describe("getClipThumbnail", () => {
  it.each([
    ["ArrayBuffer", new Uint8Array([137, 80, 78]).buffer],
    ["Uint8Array", new Uint8Array([137, 80, 78])],
    ["number[]", [137, 80, 78]],
  ])("normalizes %s to Uint8Array", async (_n, raw) => {
    const { t, invoke } = fakeTransport(raw);
    ipc.setTransport(t);
    const bytes = await ipc.getClipThumbnail(9);
    expect(invoke).toHaveBeenCalledWith("get_clip_thumbnail", { id: 9 });
    expect(bytes).toBeInstanceOf(Uint8Array);
    expect(Array.from(bytes)).toEqual([137, 80, 78]);
  });
});

describe("getAppIcon", () => {
  it("normalizes the PNG bytes like thumbnails", async () => {
    const { t } = fakeTransport([137, 80]);
    ipc.setTransport(t);
    expect(Array.from(await ipc.getAppIcon("a.b"))).toEqual([137, 80]);
  });
});

describe("events", () => {
  it.each([
    ["onClipsChanged", ipc.onClipsChanged, "clips-changed", { reason: "inserted", id: 3 }],
    ["onPinboardsChanged", ipc.onPinboardsChanged, "pinboards-changed", null],
    ["onShelfShown", ipc.onShelfShown, "shelf-shown", null],
    ["onSettingsChanged", ipc.onSettingsChanged, "settings-changed", settings],
    ["onCaptureSkipped", ipc.onCaptureSkipped, "capture-skipped", { kind: "image", limitBytes: 64_000_000 }],
  ] as const)("%s listens to %s and hands over the payload", async (_n, fn, event, payload) => {
    const { t, handlers, unlisten } = fakeTransport();
    ipc.setTransport(t);
    const seen: unknown[] = [];
    const stop = await (fn as (h: (p: unknown) => void) => Promise<() => void>)((p) => seen.push(p));
    handlers.get(event)!(payload);
    expect(seen).toEqual([payload]);
    stop();
    expect(unlisten).toHaveBeenCalled();
  });
});

describe("default transport", () => {
  it("falls back to the in-memory mock outside Tauri in development", async () => {
    const clips = await ipc.listClips({ text: null, kind: null, pinboardId: null, limit: 5, before: null });
    expect(clips.length).toBe(5);
    expect(typeof clips[0].id).toBe("number");
  });
});
