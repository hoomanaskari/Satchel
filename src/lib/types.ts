// TypeScript mirror of the IPC contract in docs/ARCHITECTURE.md.
// Source of truth: crates/satchel-core/src/model.rs (serde, camelCase).

export type ClipId = number;
export type PinboardId = number;
/** Milliseconds since the Unix epoch. */
export type Millis = number;

export const CLIP_KINDS = ["text", "link", "code", "color", "image", "file"] as const;
export type ClipKind = (typeof CLIP_KINDS)[number];

export interface SourceApp {
  /** Bundle identifier on macOS, executable name elsewhere. */
  id: string;
  name: string;
}

/** Lightweight row for lists. Never carries full content or image bytes. */
export interface ClipSummary {
  id: ClipId;
  kind: ClipKind;
  /** User-given title, if any. */
  title: string | null;
  /** At most 300 characters (file names for files, hex for colors, empty for images). */
  preview: string;
  sourceApp: SourceApp | null;
  createdAt: Millis;
  lastUsedAt: Millis;
  byteSize: number;
  hasThumbnail: boolean;
  pinboardIds: PinboardId[];
}

/** An application for the excluded-apps picker. */
export interface AppInfo {
  /** What the excluded-app list stores: the bundle identifier on macOS. */
  id: string;
  /** Localized display name. */
  name: string;
  /** Running right now. */
  running: boolean;
}

export interface Pinboard {
  id: PinboardId;
  name: string;
  /** `#RRGGBB`. */
  color: string;
  position: number;
}

/** Keyset cursor: `(lastUsedAt, id)` of the last row already shown. */
export type Cursor = [Millis, ClipId];

/** Keyset-paginated list query. Results are newest first by `(lastUsedAt, id)`. */
export interface Query {
  text: string | null;
  kind: ClipKind | null;
  pinboardId: PinboardId | null;
  /** Clamped to 1..=200 by the store; 0 means the default of 50. */
  limit: number;
  /** Return rows strictly older than this cursor. */
  before: Cursor | null;
}

export interface Settings {
  /** Unpinned clips beyond this count are pruned, oldest first. */
  maxItems: number;
  /** Unpinned clips older than this many days are pruned. `null` keeps them. */
  retentionDays: number | null;
  /** Source app ids that are never captured. */
  excludedAppIds: string[];
  capturePaused: boolean;
  captureImages: boolean;
  /** Global shortcut toggling the shelf, in Tauri accelerator syntax. */
  shelfShortcut: string;
  /** Paste into the frontmost app on selection instead of only copying. */
  pasteOnSelect: boolean;
}

export type ClipsChangedReason = "inserted" | "bumped" | "deleted" | "cleared" | "updated";

export interface ClipsChanged {
  reason: ClipsChangedReason;
  id?: ClipId;
}

/** A copy that was not kept because it is over a size cap (no content). */
export interface CaptureSkipped {
  kind: ClipKind;
  limitBytes: number;
}

/** Every command in the contract: argument object and resolved value. */
export interface Commands {
  list_clips: { args: { query: Query }; ret: ClipSummary[] };
  get_clip_text: { args: { id: ClipId }; ret: string | null };
  /** Raw PNG bytes. Tauri delivers `ArrayBuffer`; a JSON byte array is tolerated. */
  get_clip_thumbnail: { args: { id: ClipId }; ret: ArrayBuffer | Uint8Array | number[] };
  paste_clip: { args: { id: ClipId; plainText: boolean }; ret: void };
  paste_clips: { args: { ids: ClipId[]; plainText: boolean }; ret: void };
  copy_clip: { args: { id: ClipId; plainText: boolean }; ret: void };
  delete_clip: { args: { id: ClipId }; ret: boolean };
  set_clip_title: { args: { id: ClipId; title: string | null }; ret: void };
  clear_history: { args: undefined; ret: number };
  list_pinboards: { args: undefined; ret: Pinboard[] };
  create_pinboard: { args: { name: string; color: string }; ret: Pinboard };
  rename_pinboard: { args: { id: PinboardId; name: string }; ret: void };
  delete_pinboard: { args: { id: PinboardId }; ret: void };
  pin_clip: { args: { clipId: ClipId; pinboardId: PinboardId }; ret: void };
  unpin_clip: { args: { clipId: ClipId; pinboardId: PinboardId }; ret: void };
  get_settings: { args: undefined; ret: Settings };
  save_settings: { args: { settings: Settings }; ret: void };
  set_capture_paused: { args: { paused: boolean }; ret: void };
  hide_shelf: { args: undefined; ret: void };
  /** Hides the shelf and opens (or focuses) the settings window. */
  open_settings: { args: undefined; ret: void };
  /** Hides the shelf and opens System Settings > Privacy & Security > Accessibility. */
  open_accessibility_settings: { args: undefined; ret: void };
  /** Closes the settings window (called from inside it). */
  close_settings: { args: undefined; ret: void };
  /** Installed and running apps, one per id, sorted by name (settings window only). */
  list_apps: { args: undefined; ret: AppInfo[] };
  /** The installed app for each id, in order; null when it is not on this machine. */
  resolve_apps: { args: { ids: string[] }; ret: (AppInfo | null)[] };
  /** The app's icon as raw PNG bytes (64 px square); rejects with "not found". */
  get_app_icon: { args: { id: string }; ret: ArrayBuffer | Uint8Array | number[] };
  /** Native "choose an application" panel; null when cancelled. */
  choose_app: { args: undefined; ret: AppInfo | null };
}

export type CommandName = keyof Commands;

/** Every event Rust emits, with its payload. */
export interface Events {
  "clips-changed": ClipsChanged;
  "pinboards-changed": null;
  "shelf-shown": null;
  "settings-changed": Settings;
  "capture-skipped": CaptureSkipped;
}

export type EventName = keyof Events;
