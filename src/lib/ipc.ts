// The only door between the UI and Rust. Every command and event in the
// IPC contract (docs/ARCHITECTURE.md) has exactly one typed function here.
//
// Inside Tauri the transport is `invoke` / `listen`. Outside Tauri, in
// development only, an in-memory mock is loaded with a dynamic import; the
// `import.meta.env.DEV` guard is a compile-time constant, so production
// builds drop the import and the mock never reaches the bundle.

import { invoke as tauriInvoke, isTauri } from "@tauri-apps/api/core";
import { listen as tauriListen } from "@tauri-apps/api/event";
import type {
  AppInfo,
  CaptureSkipped,
  ClipId,
  ClipsChanged,
  ClipSummary,
  CommandName,
  Commands,
  EventName,
  Events,
  Pinboard,
  PinboardId,
  Query,
  Settings,
} from "./types";

export interface Transport {
  invoke(cmd: string, args?: Record<string, unknown>): Promise<unknown>;
  listen(event: string, handler: (payload: unknown) => void): Promise<() => void>;
}

const tauriTransport: Transport = {
  invoke: (cmd, args) => tauriInvoke(cmd, args),
  listen: (event, handler) => tauriListen(event, (e) => handler(e.payload)),
};

let transport: Promise<Transport> | null = null;

/** Replace the transport (tests). `null` restores the default. */
export function setTransport(t: Transport | null): void {
  transport = t ? Promise.resolve(t) : null;
}

/** True when the default transport is the development mock. */
export function usingMock(): boolean {
  return !isTauri() && import.meta.env.DEV;
}

function getTransport(): Promise<Transport> {
  if (!transport) {
    if (isTauri()) {
      transport = Promise.resolve(tauriTransport);
    } else if (import.meta.env.DEV) {
      transport = import("./mock/backend").then((m) => m.createMockTransport());
    } else {
      transport = Promise.reject(new Error("Satchel's interface only runs inside the Satchel app."));
    }
  }
  return transport;
}

function toError(e: unknown): Error {
  if (e instanceof Error) return e;
  return new Error(typeof e === "string" ? e : "Unexpected error");
}

async function call<K extends CommandName>(
  cmd: K,
  ...args: Commands[K]["args"] extends undefined ? [] : [Commands[K]["args"]]
): Promise<Commands[K]["ret"]> {
  const t = await getTransport();
  try {
    return (await t.invoke(cmd, args[0] as Record<string, unknown> | undefined)) as Commands[K]["ret"];
  } catch (e) {
    throw toError(e);
  }
}

async function on<E extends EventName>(event: E, handler: (payload: Events[E]) => void): Promise<() => void> {
  const t = await getTransport();
  return t.listen(event, (p) => handler(p as Events[E]));
}

// ---- commands ----

export const listClips = (query: Query): Promise<ClipSummary[]> => call("list_clips", { query });
export const getClipText = (id: ClipId): Promise<string | null> => call("get_clip_text", { id });

function bytes(raw: ArrayBuffer | Uint8Array | number[]): Uint8Array {
  if (raw instanceof Uint8Array) return raw;
  if (raw instanceof ArrayBuffer) return new Uint8Array(raw);
  return Uint8Array.from(raw ?? []);
}

export async function getClipThumbnail(id: ClipId): Promise<Uint8Array> {
  return bytes(await call("get_clip_thumbnail", { id }));
}

export async function getLinkImage(id: ClipId): Promise<Uint8Array> {
  return bytes(await call("get_link_image", { id }));
}

export const pasteClip = (id: ClipId, plainText: boolean): Promise<void> => call("paste_clip", { id, plainText });
export const pasteClips = (ids: ClipId[], plainText: boolean): Promise<void> =>
  call("paste_clips", { ids, plainText });
export const copyClip = (id: ClipId, plainText: boolean): Promise<void> => call("copy_clip", { id, plainText });
export const deleteClip = (id: ClipId): Promise<boolean> => call("delete_clip", { id });
export const setClipTitle = (id: ClipId, title: string | null): Promise<void> =>
  call("set_clip_title", { id, title });
export const clearHistory = (): Promise<number> => call("clear_history");
export const listPinboards = (): Promise<Pinboard[]> => call("list_pinboards");
export const createPinboard = (name: string, color: string): Promise<Pinboard> =>
  call("create_pinboard", { name, color });
export const renamePinboard = (id: PinboardId, name: string): Promise<void> => call("rename_pinboard", { id, name });
export const deletePinboard = (id: PinboardId): Promise<void> => call("delete_pinboard", { id });
export const pinClip = (clipId: ClipId, pinboardId: PinboardId): Promise<void> =>
  call("pin_clip", { clipId, pinboardId });
export const unpinClip = (clipId: ClipId, pinboardId: PinboardId): Promise<void> =>
  call("unpin_clip", { clipId, pinboardId });
export const getSettings = (): Promise<Settings> => call("get_settings");
export const saveSettings = (settings: Settings): Promise<void> => call("save_settings", { settings });
export const setCapturePaused = (paused: boolean): Promise<void> => call("set_capture_paused", { paused });
export const hideShelf = (): Promise<void> => call("hide_shelf");
export const openSettings = (): Promise<void> => call("open_settings");
export const closeSettings = (): Promise<void> => call("close_settings");
export const openAccessibilitySettings = (): Promise<void> => call("open_accessibility_settings");
export const listApps = (): Promise<AppInfo[]> => call("list_apps");
export const resolveApps = (ids: string[]): Promise<(AppInfo | null)[]> => call("resolve_apps", { ids });
export async function getAppIcon(id: string): Promise<Uint8Array> {
  return bytes(await call("get_app_icon", { id }));
}
export const chooseApp = (): Promise<AppInfo | null> => call("choose_app");

// ---- events ----

export const onClipsChanged = (h: (p: ClipsChanged) => void) => on("clips-changed", h);
export const onPinboardsChanged = (h: (p: null) => void) => on("pinboards-changed", h);
export const onShelfShown = (h: (p: null) => void) => on("shelf-shown", h);
export const onSettingsChanged = (h: (p: Settings) => void) => on("settings-changed", h);
export const onCaptureSkipped = (h: (p: CaptureSkipped) => void) => on("capture-skipped", h);

/** Everything above as one object, so state modules can take it as a dependency. */
export const api = {
  isMock: usingMock,
  listClips,
  getClipText,
  getClipThumbnail,
  getLinkImage,
  pasteClip,
  pasteClips,
  copyClip,
  deleteClip,
  setClipTitle,
  clearHistory,
  listPinboards,
  createPinboard,
  renamePinboard,
  deletePinboard,
  pinClip,
  unpinClip,
  getSettings,
  saveSettings,
  setCapturePaused,
  hideShelf,
  openSettings,
  closeSettings,
  openAccessibilitySettings,
  listApps,
  resolveApps,
  getAppIcon,
  chooseApp,
  onClipsChanged,
  onPinboardsChanged,
  onShelfShown,
  onSettingsChanged,
  onCaptureSkipped,
};

export type Api = typeof api;
