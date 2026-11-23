// Logic behind the settings window's "Never capture from" list and its app
// picker. The stored list stays a list of app identifiers (bundle ids on
// macOS, executables elsewhere); this turns it into rows people recognize.

import type { AppIconCache } from "./app-icons";
import type { AppInfo } from "./types";

/** What the settings window needs from the backend to show and pick apps. */
export interface AppDirectory {
  listApps(): Promise<AppInfo[]>;
  resolveApps(ids: string[]): Promise<(AppInfo | null)[]>;
  /** Native "choose an application" panel; null when cancelled or unavailable. */
  chooseApp(): Promise<AppInfo | null>;
  icons: AppIconCache;
}

/** Same bound as `MAX_APP_ID_CHARS` in `src-tauri/src/validate.rs`. */
export const MAX_APP_ID_CHARS = 255;

/**
 * The built-in defaults: a copy of `DEFAULT_EXCLUDED_APP_IDS` in
 * `crates/satchel-core/src/privacy.rs` (a Rust test keeps the two equal).
 * They stay stored and enforced whether or not the app is installed, so an
 * app installed later is already excluded; the list only shows the ones that
 * are installed here.
 */
export const BUILT_IN_DEFAULTS: readonly string[] = [
  "com.1password.1password",
  "com.agilebits.onepassword7",
  "com.agilebits.onepassword-osx",
  "com.agilebits.onepassword4",
  "com.bitwarden.desktop",
  "org.keepassxc.keepassxc",
  "com.keepassium.ios",
  "com.markmcguill.strongbox.mac",
  "com.hicknhacksoftware.MacPass",
  "com.lastpass.LastPass",
  "com.dashlane.Dashlane",
  "in.sinew.Enpass-Desktop",
  "com.nordsec.nordpass",
  "me.proton.pass.electron",
  "com.callpod.keepermac.lite",
  "com.keepersecurity.passwordmanager",
  "com.siber.RoboForm",
  "com.outercorner.Secrets",
  "com.apple.keychainaccess",
  "com.apple.Passwords",
  "com.authy.authy-mac",
  "1Password.exe",
  "KeePass.exe",
  "KeePassXC.exe",
  "Bitwarden.exe",
  "Enpass.exe",
  "NordPass.exe",
  "Proton Pass.exe",
  "keeperpasswordmanager.exe",
  "RoboForm.exe",
  "Dashlane.exe",
  "CredentialUIBroker.exe",
  "keepassxc",
  "org.keepassxc.KeePassXC",
  "keepass2",
  "bitwarden",
  "1password",
  "enpass",
  "seahorse",
  "org.gnome.seahorse.Application",
  "kwalletmanager5",
  "org.kde.kwalletmanager5",
  "kwalletmanager",
  "gnome-passwordsafe",
  "org.gnome.World.Secrets",
  "keeper-password-manager",
];

const DEFAULT_KEYS = new Set(BUILT_IN_DEFAULTS.map((id) => id.toLowerCase()));

const key = (id: string) => id.trim().toLowerCase();

/** Whether `id` is one of the built-in defaults (case-insensitive, like capture). */
export function isBuiltInDefault(id: string): boolean {
  return DEFAULT_KEYS.has(key(id));
}

/** Capture matches ids case-insensitively, so the list does too. */
export function isExcluded(list: readonly string[], id: string): boolean {
  const k = key(id);
  return list.some((x) => key(x) === k);
}

/** The list with `id` appended, unless it (in any case) is already there. */
export function addExcluded(list: readonly string[], id: string): string[] {
  const trimmed = id.trim();
  if (!trimmed || isExcluded(list, trimmed)) return [...list];
  return [...list, trimmed];
}

/** The list without exactly these stored ids. */
export function removeExcluded(list: readonly string[], ids: readonly string[]): string[] {
  return list.filter((x) => !ids.includes(x));
}

/** Error for the "Add by identifier" field, or null when it can be added. */
export function identifierError(raw: string, list: readonly string[]): string | null {
  const id = raw.trim();
  if (!id) return "Enter an app identifier.";
  // eslint-disable-next-line no-control-regex
  if ([...id].length > MAX_APP_ID_CHARS || /[\u0000-\u001f\u007f-\u009f]/.test(id)) {
    return "That is not a valid app identifier.";
  }
  if (isExcluded(list, id)) {
    return isBuiltInDefault(id) ? "Satchel already excludes that app by default." : "That app is already excluded.";
  }
  return null;
}

export interface ExcludedRow {
  /** Stable, unique key for keyed lists: the exact stored id (the list may hold ids differing only in case). */
  key: string;
  /** Display name; null when nothing better than the identifier is known. */
  name: string | null;
  /** The stored ids this row stands for; Remove removes all of them. */
  ids: string[];
  installed: boolean;
  /** Id to fetch the icon for (installed rows only). */
  iconId: string | null;
  /** Small secondary text: identifiers when they help tell rows apart. */
  detail: string | null;
}

const label = (r: ExcludedRow) => (r.name ?? r.ids[0]).toLowerCase();
const byLabel = (a: ExcludedRow, b: ExcludedRow) =>
  label(a).localeCompare(label(b)) || a.key.localeCompare(b.key);

/**
 * The rows the settings window lists, sorted by name: every excluded app
 * that is installed here (same-named copies merged into one row), plus
 * entries the user added for apps that are not installed (shown by
 * identifier, so they can always be removed). Built-in defaults that are not
 * installed, including every other-platform default, are not listed; they
 * stay in the stored list untouched. `resolved` maps lowercased ids to the
 * installed app (null or missing: not installed).
 */
export function excludedView(list: readonly string[], resolved: ReadonlyMap<string, AppInfo | null>): ExcludedRow[] {
  const installed = new Map<string, ExcludedRow>();
  const rows: ExcludedRow[] = [];
  for (const id of list) {
    const app = resolved.get(key(id)) ?? null;
    if (app) {
      const nameKey = app.name.trim().toLowerCase();
      const row = installed.get(nameKey);
      if (row) {
        row.ids.push(id);
        row.detail = row.ids.join(", ");
      } else {
        installed.set(nameKey, { key: `app:${id}`, name: app.name, ids: [id], installed: true, iconId: app.id, detail: null });
      }
    } else if (!isBuiltInDefault(id)) {
      rows.push({ key: `id:${id}`, name: null, ids: [id], installed: false, iconId: null, detail: null });
    }
  }
  rows.push(...installed.values());
  return rows.sort(byLabel);
}

export interface Candidate extends AppInfo {
  excluded: boolean;
}

/**
 * Picker entries matching `query` (every word must appear in the name or
 * identifier), names that start with the query first, already-excluded apps
 * marked.
 */
export function filterCandidates(apps: readonly AppInfo[], query: string, list: readonly string[]): Candidate[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  const q = query.trim().toLowerCase();
  return apps
    .filter((a) => {
      const hay = `${a.name} ${a.id}`.toLowerCase();
      return words.every((w) => hay.includes(w));
    })
    .map((a, i) => ({ a, i, prefix: q !== "" && a.name.toLowerCase().startsWith(q) ? 0 : 1 }))
    .sort((x, y) => x.prefix - y.prefix || x.i - y.i)
    .map(({ a }) => ({ ...a, excluded: isExcluded(list, a.id) }));
}

/**
 * Next selectable index from `from` moving by `step` (±1, or ±Infinity for
 * Home/End), skipping disabled entries and stopping at the ends. -1 when
 * nothing is selectable.
 */
export function moveActive(items: readonly { excluded: boolean }[], from: number, step: number): number {
  const enabled = items.map((it, i) => (it.excluded ? -1 : i)).filter((i) => i >= 0);
  if (enabled.length === 0) return -1;
  if (step === Infinity) return enabled[enabled.length - 1];
  if (step === -Infinity) return enabled[0];
  if (from < 0) return step > 0 ? enabled[0] : enabled[enabled.length - 1];
  if (step > 0) return enabled.find((i) => i > from) ?? enabled.find((i) => i >= from) ?? enabled[enabled.length - 1];
  return [...enabled].reverse().find((i) => i < from) ?? enabled.find((i) => i >= from) ?? enabled[0];
}
