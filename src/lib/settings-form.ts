// Settings view logic: validation, shortcut recording and display.

import type { Settings } from "./types";

export const MAX_ITEMS_RANGE = { min: 10, max: 100_000 } as const;
export const RETENTION_RANGE = { min: 1, max: 3650 } as const;

const MODIFIERS = ["CommandOrControl", "Control", "Super", "Alt", "Shift"] as const;
type Modifier = (typeof MODIFIERS)[number];
const MODIFIER_ALIASES: Record<string, Modifier> = {
  CommandOrControl: "CommandOrControl",
  CmdOrCtrl: "CommandOrControl",
  Command: "Super",
  Cmd: "Super",
  Super: "Super",
  Control: "Control",
  Ctrl: "Control",
  Alt: "Alt",
  Option: "Alt",
  Shift: "Shift",
};

const NAMED_CODES: Record<string, string> = {
  Space: "Space",
  Enter: "Enter",
  Tab: "Tab",
  Backspace: "Backspace",
  Delete: "Delete",
  Insert: "Insert",
  Home: "Home",
  End: "End",
  PageUp: "PageUp",
  PageDown: "PageDown",
  ArrowUp: "Up",
  ArrowDown: "Down",
  ArrowLeft: "Left",
  ArrowRight: "Right",
  Minus: "Minus",
  Equal: "Equal",
  Comma: "Comma",
  Period: "Period",
  Slash: "Slash",
  Backslash: "Backslash",
  Semicolon: "Semicolon",
  Quote: "Quote",
  Backquote: "Backquote",
  BracketLeft: "BracketLeft",
  BracketRight: "BracketRight",
};

const KEY_NAMES = new Set(Object.values(NAMED_CODES));

function keyFromCode(code: string): string | null {
  let m = /^Key([A-Z])$/.exec(code);
  if (m) return m[1];
  m = /^Digit([0-9])$/.exec(code);
  if (m) return m[1];
  if (/^F([1-9]|1[0-9]|2[0-4])$/.test(code)) return code;
  return NAMED_CODES[code] ?? null;
}

const isFunctionKey = (k: string) => /^F([1-9]|1[0-9]|2[0-4])$/.test(k);
const isKeyName = (k: string) => /^[A-Z0-9]$/.test(k) || isFunctionKey(k) || KEY_NAMES.has(k);

export interface ShortcutEvent {
  code: string;
  key: string;
  metaKey: boolean;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
}

/** Tauri accelerator for a key press, or null while it is not a usable shortcut yet. */
export function acceleratorFromEvent(e: ShortcutEvent, mac: boolean): string | null {
  const key = keyFromCode(e.code);
  if (!key) return null;
  const mods: Modifier[] = [];
  if (mac ? e.metaKey : e.ctrlKey) mods.push("CommandOrControl");
  if (mac && e.ctrlKey) mods.push("Control");
  if (!mac && e.metaKey) mods.push("Super");
  if (e.altKey) mods.push("Alt");
  if (e.shiftKey) mods.push("Shift");
  const strong = mods.some((m) => m !== "Shift");
  if (!strong && !isFunctionKey(key)) return null;
  return [...mods, key].join("+");
}

function parse(acc: string): { mods: Modifier[]; key: string } | null {
  const parts = acc.split("+");
  if (parts.some((p) => p === "")) return null;
  const mods: Modifier[] = [];
  const keys: string[] = [];
  for (const p of parts) {
    const m = MODIFIER_ALIASES[p];
    if (m) mods.push(m);
    else keys.push(p);
  }
  if (keys.length !== 1) return null;
  return { mods, key: keys[0] };
}

export function isValidAccelerator(acc: string): boolean {
  const p = parse(acc);
  if (!p || !isKeyName(p.key)) return false;
  return p.mods.some((m) => m !== "Shift") || isFunctionKey(p.key);
}

const MAC_SYMBOLS: [Modifier, string][] = [
  ["Control", "⌃"],
  ["Alt", "⌥"],
  ["Shift", "⇧"],
  ["CommandOrControl", "⌘"],
  ["Super", "⌘"],
];
const OTHER_NAMES: [Modifier, string][] = [
  ["CommandOrControl", "Ctrl"],
  ["Control", "Ctrl"],
  ["Super", "Win"],
  ["Alt", "Alt"],
  ["Shift", "Shift"],
];

/** Human form: "⇧⌘V" on macOS, "Ctrl+Shift+V" elsewhere. */
export function formatAccelerator(acc: string, mac: boolean): string {
  const p = parse(acc);
  if (!p) return acc;
  const table = mac ? MAC_SYMBOLS : OTHER_NAMES;
  const mods = [...new Set(table.filter(([m]) => p.mods.includes(m)).map(([, s]) => s))];
  return mac ? mods.join("") + p.key : [...mods, p.key].join("+");
}

export type SettingsErrors = Partial<Record<keyof Settings, string>>;

export function validateSettings(s: Settings): SettingsErrors {
  const errors: SettingsErrors = {};
  if (!Number.isInteger(s.maxItems) || s.maxItems < MAX_ITEMS_RANGE.min || s.maxItems > MAX_ITEMS_RANGE.max) {
    errors.maxItems = `Enter a whole number from ${MAX_ITEMS_RANGE.min} to ${MAX_ITEMS_RANGE.max.toLocaleString("en-US")}.`;
  }
  if (
    s.retentionDays !== null &&
    (!Number.isInteger(s.retentionDays) || s.retentionDays < RETENTION_RANGE.min || s.retentionDays > RETENTION_RANGE.max)
  ) {
    errors.retentionDays = "Choose how long to keep clips.";
  }
  if (!isValidAccelerator(s.shelfShortcut)) {
    errors.shelfShortcut = "Use a key together with Command, Control or Option.";
  }
  return errors;
}

export function normalizeExcluded(ids: string[]): string[] {
  const out: string[] = [];
  for (const raw of ids) {
    const id = raw.trim();
    if (id && !out.includes(id)) out.push(id);
  }
  return out;
}

const RETENTION_PRESETS: { value: number; label: string }[] = [
  { value: 1, label: "1 day" },
  { value: 7, label: "1 week" },
  { value: 30, label: "1 month" },
  { value: 90, label: "3 months" },
  { value: 365, label: "1 year" },
];

export function retentionOptions(current: number | null): { value: number | null; label: string }[] {
  const opts: { value: number | null; label: string }[] = [...RETENTION_PRESETS];
  if (current !== null && !opts.some((o) => o.value === current)) {
    opts.push({ value: current, label: `${current} days` });
    opts.sort((a, b) => (a.value as number) - (b.value as number));
  }
  opts.push({ value: null, label: "Forever" });
  return opts;
}
