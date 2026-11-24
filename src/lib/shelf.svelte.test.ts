import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { api, setTransport } from "./ipc";
import { createMockBackend, type MockBackend } from "./mock/backend";
import { ACCESSIBILITY_NOTICE, Shelf } from "./shelf.svelte";

let mock: MockBackend;
let shelf: Shelf;
let dispose: () => void;

const flush = () => new Promise((r) => setTimeout(r, 0));
// Commands other than the list refreshes that events trigger in the background.
const cmds = () => mock.log.map((l) => l.cmd).filter((c) => c !== "list_clips" && c !== "list_pinboards");
const lastOf = (cmd: string) => [...mock.log].reverse().find((l) => l.cmd === cmd);

async function make(opts: { visible?: boolean } = {}) {
  mock = createMockBackend();
  setTransport(mock.transport);
  shelf = new Shelf({ api, pageSize: 20, debounceMs: 40 });
  dispose = await shelf.init();
  if (opts.visible === false) shelf.setVisible(false);
}

beforeEach(async () => {
  await make();
});

afterEach(() => {
  dispose?.();
  setTransport(null);
  vi.useRealTimers();
});

describe("startup", () => {
  it("loads settings, pinboards and the first page, selecting the first clip", () => {
    expect(shelf.settings?.shelfShortcut).toBe("CommandOrControl+Shift+V");
    expect(shelf.pinboards.map((p) => p.name)).toEqual(["Snippets", "Work"]);
    expect(shelf.clips).toHaveLength(20);
    expect(shelf.hasMore).toBe(true);
    expect(shelf.selection.ids).toEqual([shelf.clips[0].id]);
    // Nothing to say yet: the top bar's notice is hidden.
    expect(shelf.notice).toBeNull();
    expect(shelf.pasteLabels).toEqual({ paste: "Paste", alternate: "Paste as Plain Text", alternateShort: "Plain Text" });
  });
});

describe("filtering", () => {
  it("debounces search text and resets the selection to the first result", async () => {
    vi.useFakeTimers();
    const before = mock.log.length;
    shelf.setText("n");
    shelf.setText("ns");
    shelf.setText("nspasteboard");
    expect(mock.log.length).toBe(before);
    await vi.advanceTimersByTimeAsync(40);
    vi.useRealTimers();
    await flush();
    expect(lastOf("list_clips")!.args).toMatchObject({ query: { text: "nspasteboard", before: null } });
    expect(mock.log.length).toBe(before + 1);
    expect(shelf.clips.length).toBeGreaterThan(0);
    expect(shelf.selection.ids).toEqual([shelf.clips[0].id]);
  });

  it("switches kind and tab immediately", async () => {
    await shelf.setKind("color");
    expect(shelf.clips.every((c) => c.kind === "color")).toBe(true);
    await shelf.setTab(1);
    expect(lastOf("list_clips")!.args).toMatchObject({ query: { kind: "color", pinboardId: 1 } });
    await shelf.setKind("all");
    expect(shelf.clips.every((c) => c.pinboardIds.includes(1))).toBe(true);
  });

  it("an immediate filter change cancels a pending search", async () => {
    vi.useFakeTimers();
    shelf.setText("zzz");
    const p = shelf.setKind("code");
    expect(vi.getTimerCount()).toBe(0);
    vi.useRealTimers();
    await p;
    expect(lastOf("list_clips")!.args).toMatchObject({ query: { text: "zzz", kind: "code" } });
  });

  it("counts results", () => {
    expect(shelf.resultLabel).toBe("20+ items");
  });
});

describe("paging", () => {
  it("loads more when the rendered window nears the end", async () => {
    await shelf.loadMoreIfNeeded(5);
    expect(shelf.clips).toHaveLength(20);
    await shelf.loadMoreIfNeeded(15);
    expect(shelf.clips).toHaveLength(40);
    const q = lastOf("list_clips")!.args as { query: { before: [number, number] } };
    expect(q.query.before).toEqual([shelf.clips[19].lastUsedAt, shelf.clips[19].id]);
  });
});

describe("selection", () => {
  it("click, Cmd-click and Shift-click", () => {
    const [a, b, c, d] = shelf.clips.map((x) => x.id);
    shelf.select(b, "single");
    shelf.select(d, "toggle");
    expect(shelf.selection.ids).toEqual([b, d]);
    shelf.select(a, "single");
    shelf.select(c, "range");
    expect(shelf.selection.ids).toEqual([a, shelf.clips[1].id, c]);
    // The selection is not narrated; the paste buttons count it instead.
    expect(shelf.notice).toBeNull();
    expect(shelf.pasteLabels).toEqual({ paste: "Paste 3 Clips", alternate: "Paste as Plain Text", alternateShort: "Plain Text" });
  });

  it("moves with the arrows and jumps to the ends", () => {
    shelf.move(1, false);
    expect(shelf.selection.focus).toBe(shelf.clips[1].id);
    shelf.edge(true);
    expect(shelf.selection.focus).toBe(shelf.clips.at(-1)!.id);
    shelf.edge(false);
    expect(shelf.selection.focus).toBe(shelf.clips[0].id);
  });

  it("selects all loaded clips", () => {
    shelf.selectAll();
    expect(shelf.selection.ids).toHaveLength(20);
  });
});

describe("paste and copy", () => {
  it("pastes the selected clip, as plain text on request", async () => {
    const id = shelf.clips[2].id;
    shelf.select(id, "single");
    await shelf.paste(true);
    expect(lastOf("paste_clip")!.args).toEqual({ id, alternate: true });
    expect(shelf.notice).toBeNull();
    expect(shelf.announcement).toBe("Pasted as plain text");
  });

  it("pastes a multi-selection in list order with paste_clips", async () => {
    const [a, b, c] = shelf.clips.map((x) => x.id);
    shelf.select(c, "single");
    shelf.select(a, "toggle");
    shelf.select(b, "toggle");
    await shelf.paste(false);
    expect(lastOf("paste_clips")!.args).toEqual({ ids: [a, b, c], alternate: false });
    expect(shelf.announcement).toBe("Pasted 3 clips");
  });

  it("only copies and hides when paste-on-select is off", async () => {
    await api.saveSettings({ ...shelf.settings!, pasteOnSelect: false });
    await flush();
    const id = shelf.clips[0].id;
    await shelf.paste(false);
    expect(cmds().slice(-2)).toEqual(["copy_clip", "hide_shelf"]);
    expect(lastOf("copy_clip")!.args).toEqual({ id, alternate: false });
  });

  it("labels the paste actions Copy, uncounted, when paste-on-select is off", async () => {
    await api.saveSettings({ ...shelf.settings!, pasteOnSelect: false });
    await flush();
    shelf.select(shelf.clips[0].id, "single");
    shelf.select(shelf.clips[1].id, "toggle");
    // Copying takes the focused clip only, so the label must not promise two.
    expect(shelf.pasteLabels).toEqual({ paste: "Copy", alternate: "Copy as Plain Text", alternateShort: "Plain Text" });
  });

  it("with plain text as the default, the other action keeps formatting, in the open shelf", async () => {
    const text = shelf.clips.find((c) => c.kind === "text")!.id;
    shelf.select(text, "single");
    await api.saveSettings({ ...shelf.settings!, pastePlainText: true });
    await flush();
    expect(shelf.pasteLabels).toEqual({ paste: "Paste", alternate: "Paste with Formatting", alternateShort: "Formatted" });
    // The flag says which action was used; Rust picks the format from the setting.
    await shelf.paste(false);
    expect(lastOf("paste_clip")!.args).toEqual({ id: text, alternate: false });
    expect(shelf.announcement).toBe("Pasted as plain text");
    await shelf.paste(true);
    expect(lastOf("paste_clip")!.args).toEqual({ id: text, alternate: true });
    expect(shelf.announcement).toBe("Pasted with formatting");
    await shelf.copy(false);
    expect(shelf.announcement).toBe("Copied to the clipboard as plain text");

    await api.saveSettings({ ...shelf.settings!, pasteOnSelect: false });
    await flush();
    expect(shelf.pasteLabels).toEqual({ paste: "Copy", alternate: "Copy with Formatting", alternateShort: "Formatted" });
    await shelf.paste(true);
    expect(lastOf("copy_clip")!.args).toEqual({ id: text, alternate: true });
    expect(shelf.announcement).toBe("Copied with formatting");

    await api.saveSettings({ ...shelf.settings!, pasteOnSelect: true, pastePlainText: false });
    await flush();
    expect(shelf.pasteLabels.alternate).toBe("Paste as Plain Text");
  });

  it("does not announce a text format for images and files", async () => {
    const image = shelf.clips.find((c) => c.kind === "image")!.id;
    shelf.select(image, "single");
    await shelf.paste(true);
    expect(shelf.announcement).toBe("Pasted");
  });

  it("Cmd+n pastes the nth clip from the first visible one", async () => {
    shelf.setFirstVisible(3);
    const fifth = shelf.clips[4].id;
    await shelf.pasteNth(2, false);
    expect(lastOf("paste_clip")!.args).toEqual({ id: fifth, alternate: false });
    const before = mock.log.length;
    await shelf.pasteNth(9, false);
    shelf.setFirstVisible(18);
    await shelf.pasteNth(9, false);
    expect(mock.log.slice(before).filter((l) => l.cmd === "paste_clip")).toHaveLength(1);
  });

  it("copies without pasting or hiding", async () => {
    await shelf.copy(false);
    expect(cmds().at(-1)).toBe("copy_clip");
    // Not shown (the clipboard is the confirmation), only announced; cleared by the next selection change.
    expect(shelf.notice).toBeNull();
    expect(shelf.announcement).toBe("Copied to the clipboard");
    shelf.move(1, false);
    expect(shelf.announcement).toBe("");
  });

  it("explains a paste refused for lack of Accessibility and offers the settings page", async () => {
    // Double-click, Return and the menu all end in shelf.paste. Without the
    // permission Rust copies, leaves the shelf up and returns this error;
    // it used to vanish with the hidden shelf, so nothing seemed to happen.
    mock.setPastePermission(false);
    const id = shelf.clips[1].id;
    shelf.select(id, "single");
    await shelf.paste(false);
    expect(lastOf("paste_clip")!.args).toEqual({ id, alternate: false });
    expect(shelf.needsPastePermission).toBe(true);
    expect(shelf.notice).toEqual({ text: ACCESSIBILITY_NOTICE, tone: "permission" });
    expect(shelf.notice?.text).not.toContain("accessibility-permission-required");
    // The notice outlives a selection change; the button stays with it.
    shelf.select(shelf.clips[0].id, "single");
    expect(shelf.notice?.tone).toBe("permission");
  });

  it("a failed list load stays in the notice as an error", async () => {
    setTransport({
      invoke: async (cmd, args) => {
        if (cmd === "list_clips") throw "database is locked";
        return mock.transport.invoke(cmd, args);
      },
      listen: mock.transport.listen,
    });
    await shelf.reload();
    expect(shelf.notice).toEqual({ text: "database is locked", tone: "error" });
    // Not a transient message: a selection change does not hide it.
    shelf.move(1, false);
    expect(shelf.notice?.tone).toBe("error");

    await shelf.openAccessibilitySettings();
    expect(cmds().at(-1)).toBe("open_accessibility_settings");
    expect(shelf.needsPastePermission).toBe(false);
  });

  it("a multi-paste refused for lack of Accessibility explains the same way", async () => {
    mock.setPastePermission(false);
    const [a, b] = shelf.clips.map((x) => x.id);
    shelf.select(a, "single");
    shelf.select(b, "toggle");
    await shelf.paste(false);
    expect(shelf.needsPastePermission).toBe(true);
  });

  it("the Accessibility notice is cleared when the shelf opens again or a paste works", async () => {
    mock.setPastePermission(false);
    await shelf.paste(false);
    expect(shelf.needsPastePermission).toBe(true);
    await shelf.onShown();
    expect(shelf.needsPastePermission).toBe(false);

    await shelf.paste(false);
    mock.setPastePermission(true);
    await shelf.paste(false);
    expect(shelf.needsPastePermission).toBe(false);
    expect(shelf.notice).toBeNull();
    expect(shelf.announcement).toBe("Pasted");
  });

  it("does nothing without a selection", async () => {
    await shelf.setText("no such clip anywhere");
    await shelf.flushSearch();
    expect(shelf.clips).toEqual([]);
    const before = mock.log.length;
    await shelf.paste(false);
    await shelf.copy(false);
    expect(mock.log.length).toBe(before);
    expect(shelf.isEmpty).toBe(true);
  });
});

describe("pin, delete, rename", () => {
  it("pins to the open pinboard, else the first one; toggles off when already pinned", async () => {
    const clip = shelf.clips.find((c) => c.pinboardIds.length === 0)!;
    shelf.select(clip.id, "single");
    await shelf.togglePin();
    expect(lastOf("pin_clip")!.args).toEqual({ clipId: clip.id, pinboardId: 1 });
    expect(shelf.clips.find((c) => c.id === clip.id)!.pinboardIds).toContain(1);
    // The pin itself is the confirmation: nothing in the top bar, only announced.
    expect(shelf.notice).toBeNull();
    expect(shelf.announcement).toMatch(/pinned to Snippets$/);
    await shelf.togglePin();
    expect(lastOf("unpin_clip")!.args).toEqual({ clipId: clip.id, pinboardId: 1 });
    expect(shelf.notice).toBeNull();
    expect(shelf.announcement).toMatch(/removed from Snippets$/);
  });

  it("creates a pinboard when pinning with none", async () => {
    for (const p of shelf.pinboards) await shelf.deletePinboard(p.id);
    expect(shelf.pinboards).toEqual([]);
    await shelf.togglePin();
    expect(lastOf("create_pinboard")!.args).toMatchObject({ name: "Pinned" });
    expect(shelf.pinboards.map((p) => p.name)).toEqual(["Pinned"]);
    expect(cmds()).toContain("pin_clip");
  });

  it("unpinning inside a pinboard removes the card from that tab", async () => {
    await shelf.setTab(1);
    const n = shelf.clips.length;
    const id = shelf.clips[0].id;
    await shelf.togglePin();
    expect(lastOf("unpin_clip")!.args).toEqual({ clipId: id, pinboardId: 1 });
    expect(shelf.clips.some((c) => c.id === id)).toBe(false);
    expect(shelf.clips.length).toBe(n - 1);
  });

  it("pins a given card without changing the selection", async () => {
    const first = shelf.clips[0].id;
    const other = shelf.clips.find((c) => c.pinboardIds.length === 0 && c.id !== first)!.id;
    await shelf.togglePin([other]);
    expect(lastOf("pin_clip")!.args).toEqual({ clipId: other, pinboardId: 1 });
    expect(shelf.selection.ids).toEqual([first]);
  });

  it("toggles membership in a specific pinboard", async () => {
    const id = shelf.clips[0].id;
    await shelf.togglePinboard(id, 2);
    expect(lastOf("pin_clip")!.args).toEqual({ clipId: id, pinboardId: 2 });
  });

  it("deletes the selection and selects the clip that takes its place", async () => {
    const [a, b, c] = shelf.clips.map((x) => x.id);
    shelf.select(a, "single");
    shelf.select(b, "toggle");
    await shelf.deleteSelected();
    expect(mock.log.filter((l) => l.cmd === "delete_clip").map((l) => l.args)).toEqual([{ id: a }, { id: b }]);
    expect(shelf.clips[0].id).toBe(c);
    expect(shelf.selection.ids).toEqual([c]);
    expect(shelf.notice).toBeNull();
    expect(shelf.announcement).toBe("Deleted 2 clips");
  });

  it("renames a clip; blank clears the title", async () => {
    const id = shelf.clips[0].id;
    await shelf.rename(id, "  Plan  ");
    expect(lastOf("set_clip_title")!.args).toEqual({ id, title: "Plan" });
    expect(shelf.clips[0].title).toBe("Plan");
    await shelf.rename(id, "   ");
    expect(lastOf("set_clip_title")!.args).toEqual({ id, title: null });
  });
});

describe("pinboards", () => {
  it("creates (and opens), renames and deletes pinboards", async () => {
    const p = await shelf.createPinboard("  Recipes ");
    expect(p!.name).toBe("Recipes");
    expect(shelf.pinboardId).toBe(p!.id);
    await shelf.renamePinboard(p!.id, "Food");
    expect(shelf.pinboards.find((x) => x.id === p!.id)!.name).toBe("Food");
    await shelf.deletePinboard(p!.id);
    expect(shelf.pinboards.some((x) => x.id === p!.id)).toBe(false);
    expect(shelf.pinboardId).toBeNull();
  });

  it("ignores blank names", async () => {
    expect(await shelf.createPinboard("  ")).toBeNull();
    const n = mock.log.length;
    await shelf.renamePinboard(1, " ");
    expect(mock.log.length).toBe(n);
  });

  it("picks distinct colors for new pinboards", async () => {
    const a = await shelf.createPinboard("A");
    const b = await shelf.createPinboard("B");
    expect(a!.color).not.toBe(b!.color);
    expect(a!.color).toMatch(/^#[0-9A-F]{6}$/);
  });
});

describe("preview", () => {
  it("opens with the full text of the focused clip and follows the selection", async () => {
    const code = shelf.clips.find((c) => c.kind === "code")!;
    shelf.select(code.id, "single");
    await shelf.togglePreview();
    expect(shelf.previewOpen).toBe(true);
    expect(shelf.preview).toMatchObject({ id: code.id, loading: false });
    expect(shelf.preview!.text).toContain("NSPasteboard");
    shelf.move(1, false);
    await flush();
    expect(shelf.preview!.id).toBe(shelf.selection.focus);
    await shelf.togglePreview();
    expect(shelf.previewOpen).toBe(false);
  });
});

describe("events", () => {
  it("refreshes on clips-changed, keeping the selection", async () => {
    const id = shelf.clips[3].id;
    shelf.select(id, "single");
    const newId = mock.simulateCopy("brand new");
    await flush();
    await flush();
    expect(shelf.clips[0].id).toBe(newId);
    expect(shelf.selection.ids).toEqual([id]);
  });

  it("removes a deleted clip locally without refetching", async () => {
    const id = shelf.clips[1].id;
    const before = mock.log.length;
    await mock.transport.invoke("delete_clip", { id });
    await flush();
    expect(shelf.clips.some((c) => c.id === id)).toBe(false);
    expect(mock.log.slice(before).map((l) => l.cmd)).toEqual(["delete_clip"]);
  });

  it("does no work while hidden, and reloads when shown", async () => {
    shelf.setVisible(false);
    const before = mock.log.length;
    mock.simulateCopy("while hidden");
    await flush();
    expect(mock.log.length).toBe(before);
    await shelf.setText("abc");
    shelf.select(shelf.clips[2].id, "single");
    mock.show();
    await flush();
    await flush();
    expect(shelf.visible).toBe(true);
    expect(shelf.text).toBe("");
    expect(shelf.clips[0].preview).toBe("while hidden");
    expect(shelf.selection.ids).toEqual([shelf.clips[0].id]);
    expect(shelf.focusSearchTick).toBeGreaterThan(0);
  });

  it("says when a copy was too large to keep: now when shown, else on the next open", async () => {
    mock.simulateOversizedCopy("image", 64_000_000);
    expect(shelf.notice).toEqual({
      text: "An image you copied was not kept: it is over Satchel's 64 MB limit.",
      tone: "warning",
    });
    shelf.setVisible(false);
    mock.simulateOversizedCopy("image", 64_000_000);
    mock.show();
    await flush();
    await flush();
    expect(shelf.notice?.text).toBe("An image you copied was not kept: it is over Satchel's 64 MB limit.");
    // Said once: the next selection change clears it and the next open does not repeat it.
    shelf.select(shelf.clips[1].id, "single");
    expect(shelf.notice).toBeNull();
    mock.show();
    await flush();
    await flush();
    expect(shelf.notice).toBeNull();
  });

  it("reloads pinboards and leaves a pinboard tab that vanished", async () => {
    await shelf.setTab(2);
    await mock.transport.invoke("delete_pinboard", { id: 2 });
    await flush();
    await flush();
    expect(shelf.pinboards.map((p) => p.id)).toEqual([1]);
    expect(shelf.pinboardId).toBeNull();
  });

  it("applies settings-changed", async () => {
    await mock.transport.invoke("set_capture_paused", { paused: true });
    expect(shelf.settings!.capturePaused).toBe(true);
  });

  it("dispose stops listening", async () => {
    dispose();
    const before = mock.log.length;
    mock.simulateCopy("after dispose");
    await flush();
    expect(mock.log.length).toBe(before);
  });
});

describe("close and errors", () => {
  it("close hides the shelf and stops a pending search", async () => {
    vi.useFakeTimers();
    shelf.setText("abc");
    await shelf.close();
    expect(vi.getTimerCount()).toBe(0);
    vi.useRealTimers();
    expect(cmds().at(-1)).toBe("hide_shelf");
  });

  it("shows command errors in the top bar's notice, as errors", async () => {
    setTransport({
      invoke: async (cmd) => {
        if (cmd === "copy_clip") throw "clipboard unavailable";
        return mock.transport.invoke(cmd);
      },
      listen: mock.transport.listen,
    });
    await shelf.copy(false);
    expect(shelf.notice).toEqual({ text: "clipboard unavailable", tone: "error" });
    // Cleared by the next selection change, leaving nothing to say.
    shelf.move(1, false);
    expect(shelf.notice).toBeNull();
  });

  it("a confirmation leaves the Accessibility notice in place", async () => {
    mock.setPastePermission(false);
    await shelf.paste(false);
    await shelf.copy(false);
    expect(shelf.notice?.tone).toBe("permission");
    expect(shelf.announcement).toBe("Copied to the clipboard");
  });
});

describe("keyboard dispatch", () => {
  it("runs the mapped action", async () => {
    const second = shelf.clips[1].id;
    await shelf.run({ type: "move", delta: 1, extend: false });
    expect(shelf.selection.focus).toBe(second);
    await shelf.run({ type: "paste", alternate: false });
    expect(lastOf("paste_clip")!.args).toEqual({ id: second, alternate: false });
    await shelf.run({ type: "close" });
    expect(cmds().at(-1)).toBe("hide_shelf");
  });

  it("Cmd+, opens the settings window instead of anything in the shelf", async () => {
    vi.useFakeTimers();
    shelf.setText("abc");
    await expect(shelf.run({ type: "settings" })).resolves.toBe(true);
    expect(vi.getTimerCount()).toBe(0);
    vi.useRealTimers();
    expect(cmds().at(-1)).toBe("open_settings");
    expect(cmds()).not.toContain("hide_shelf");
  });
});

describe("paused indicator", () => {
  it("is off by default", () => {
    expect(shelf.paused).toBe(false);
  });

  it("follows the shelf's own toggle, and resuming clears it", async () => {
    await shelf.setPaused(true);
    expect(shelf.paused).toBe(true);
    await shelf.setPaused(false);
    expect(shelf.paused).toBe(false);
    expect(lastOf("set_capture_paused")!.args).toEqual({ paused: false });
    // A state, not a notice: the notice slot stays empty throughout.
    expect(shelf.notice).toBeNull();
  });

  it("follows a pause from elsewhere (tray, Settings) while the shelf is up", async () => {
    // The tray calls the same Rust command, which emits settings-changed.
    await mock.transport.invoke("set_capture_paused", { paused: true });
    expect(shelf.paused).toBe(true);
    await mock.transport.invoke("set_capture_paused", { paused: false });
    expect(shelf.paused).toBe(false);
  });

  it("follows a pause from elsewhere while the shelf is hidden", async () => {
    shelf.setVisible(false);
    // The Settings window saves the whole settings object.
    const s = (await mock.transport.invoke("get_settings")) as Record<string, unknown>;
    await mock.transport.invoke("save_settings", { settings: { ...s, capturePaused: true } });
    mock.show();
    await flush();
    await flush();
    expect(shelf.paused).toBe(true);
  });

  it("reloads settings on open, so a missed event never leaves it stale", async () => {
    shelf.setVisible(false);
    mock.setPausedSilently(true);
    expect(shelf.paused).toBe(false);
    mock.show();
    await flush();
    await flush();
    expect(shelf.paused).toBe(true);
  });
});

describe("update control", () => {
  it("is hidden until an update is on offer, and never as a notice", async () => {
    expect(shelf.update).toBeNull();
    mock.publishUpdate("0.2.0");
    await mock.transport.invoke("check_for_updates");
    expect(shelf.update).toMatchObject({ label: "Update to 0.2.0", actionable: true });
    expect(shelf.notice).toBeNull();
  });

  it("starts the update and follows its progress", async () => {
    mock.publishUpdate("0.2.0");
    await mock.transport.invoke("check_for_updates");
    await shelf.installUpdate();
    expect(cmds()).toContain("install_update");
    expect(shelf.update).toMatchObject({ label: "Installing…", actionable: false });
    // Nothing more to start while it installs.
    const n = mock.log.length;
    await shelf.installUpdate();
    expect(mock.log.length).toBe(n);
  });

  it("offers to try a failed update again", async () => {
    mock.publishUpdate("0.2.0");
    mock.failNextInstall("verify");
    await mock.transport.invoke("check_for_updates");
    await shelf.installUpdate();
    expect(shelf.update).toMatchObject({ label: "Update Failed", actionable: true, tone: "failed" });
    mock.failNextInstall(null);
    await shelf.installUpdate();
    expect(shelf.update?.label).toBe("Installing…");
  });

  it("picks up an offer found while the shelf was hidden when it opens", async () => {
    shelf.setVisible(false);
    mock.publishUpdate("0.2.0");
    await mock.transport.invoke("check_for_updates");
    mock.show();
    await flush();
    expect(shelf.update?.label).toBe("Update to 0.2.0");
  });
});
