// In-memory stand-in for the Rust side, used by `pnpm dev` in a plain browser
// and by tests. Development only: `ipc.ts` loads it behind
// `import.meta.env.DEV`, so it is not part of production builds.
//
// It follows the contract's semantics closely (ordering, keyset cursor,
// limit clamping, events) so the UI behaves the same as against the store.

import { APP_NAME } from "../app-name";
import type { Transport } from "../ipc";
import type {
  AppInfo,
  ClipId,
  LinkPreview,
  ClipKind,
  ClipSummary,
  Cursor,
  EventName,
  Pinboard,
  Query,
  Settings,
  SourceApp,
  UpdateFailure,
  UpdateStatus,
} from "../types";

const PREVIEW_CHARS = 300;
/** Same text as `error::ACCESSIBILITY` in Rust. */
const ACCESSIBILITY_ERROR =
  `accessibility-permission-required: ${APP_NAME} copied the clip but needs Accessibility access to paste it. ` +
  `Press the paste shortcut yourself, or allow ${APP_NAME} in System Settings > Privacy & Security > Accessibility.`;

interface Row extends ClipSummary {
  text: string | null;
}

interface Sample {
  kind: ClipKind;
  app: [string, string];
  ago: number;
  text: string;
  title?: string;
  pins?: number[];
  bytes?: number;
  /** A cached link preview (what Rust would have fetched with the setting on). */
  link?: LinkPreview;
}

const MIN = 60_000;
const HR = 60 * MIN;
const DAY = 24 * HR;

const APPS = {
  notes: ["com.apple.Notes", "Notes"],
  safari: ["com.apple.Safari", "Safari"],
  xcode: ["com.apple.dt.Xcode", "Xcode"],
  figma: ["com.figma.Desktop", "Figma"],
  shot: ["com.apple.screenshot", "Screenshot"],
  mail: ["com.apple.mail", "Mail"],
  finder: ["com.apple.finder", "Finder"],
  terminal: ["com.apple.Terminal", "Terminal"],
  slack: ["com.tinyspeck.slackmacgap", "Slack"],
  vscode: ["com.microsoft.VSCode", "Visual Studio Code"],
  pages: ["com.apple.iWork.Pages", "Pages"],
} satisfies Record<string, [string, string]>;

const SAMPLES: Sample[] = [
  { kind: "text", app: APPS.notes, ago: 0, text: "Ship the clipboard watcher behind a flag first, then turn on image capture once memory use looks sane." },
  { kind: "link", app: APPS.safari, ago: 2 * MIN, text: "https://developer.apple.com/documentation/appkit/nspasteboard", link: { title: "NSPasteboard | Apple Developer Documentation", site: "Apple Developer", image: "icon" } },
  { kind: "link", app: APPS.safari, ago: 5 * MIN, text: "https://www.youtube.com/watch?v=aqz-KE-bpKQ", link: { title: "Big Buck Bunny 60fps 4K - Official Blender Foundation Short Film", site: "YouTube", image: "photo" } },
  { kind: "link", app: APPS.mail, ago: 6 * MIN, text: "https://accounts.example.com/reset-password?token=8f14e45fceea167a5a36dedd4bea2543" },
  { kind: "code", app: APPS.xcode, ago: 8 * MIN, pins: [1], text: "let pb = NSPasteboard.general\nif pb.changeCount != last {\n  last = pb.changeCount\n  store(pb.pasteboardItems)\n}" },
  { kind: "color", app: APPS.figma, ago: 14 * MIN, text: "#34C759" },
  { kind: "image", app: APPS.shot, ago: 31 * MIN, text: "", bytes: 412_000 },
  { kind: "text", app: APPS.mail, ago: HR, title: "Email signature", pins: [1], text: "Thanks,\n[YOUR NAME]\n[YOUR ROLE]" },
  { kind: "file", app: APPS.finder, ago: 2 * HR, text: "roadmap-q1.pdf" },
  { kind: "code", app: APPS.terminal, ago: 3 * HR, text: "git log --oneline --graph --decorate -20" },
  { kind: "link", app: APPS.safari, ago: 26 * HR, title: "Sparkle: updates for Mac apps", text: "https://sparkle-project.org/", link: { title: "Sparkle: open source software update framework for macOS", site: "sparkle-project.org", image: "photo" } },
  { kind: "link", app: APPS.notes, ago: 27 * HR + 30 * MIN, text: "https://en.wikipedia.org/wiki/Clipboard_(computing)", link: { title: "Clipboard (computing) - Wikipedia", site: "Wikipedia", image: null } },
  { kind: "color", app: APPS.xcode, ago: 27 * HR, text: "#FF9F0A" },
  { kind: "text", app: APPS.slack, ago: 28 * HR, pins: [2], text: "Standup moved to 10:15 tomorrow — same room, bring the latency numbers." },
  { kind: "code", app: APPS.vscode, ago: 29 * HR, text: "const debounced = debounce(() => search(query), 40);\nfield.addEventListener('input', debounced);" },
  { kind: "file", app: APPS.finder, ago: 30 * HR, text: "IMG_2041.HEIC\nIMG_2042.HEIC\nIMG_2043.HEIC" },
  { kind: "text", app: APPS.pages, ago: 2 * DAY, text: "Clipboard history contains passwords and secrets, so the default is to keep less, not more. Concealed pasteboard types are skipped before content is read, and excluded apps are never captured. ".repeat(3).trim() },
  { kind: "image", app: APPS.safari, ago: 2 * DAY + HR, text: "", bytes: 1_840_000 },
  { kind: "link", app: APPS.slack, ago: 3 * DAY, pins: [2], text: "https://v2.tauri.app/security/csp/", link: { title: "Content Security Policy (CSP)", site: "Tauri", image: "photo" } },
  { kind: "code", app: APPS.terminal, ago: 3 * DAY + HR, text: "cargo test -p satchel-core -- --nocapture" },
  { kind: "color", app: APPS.figma, ago: 4 * DAY, text: "#5E5CE6" },
  { kind: "text", app: APPS.notes, ago: 5 * DAY, text: "<b>Not HTML</b> — clip content is shown as text, so this tag stays visible: <img src=x onerror=alert(1)>" },
  { kind: "file", app: APPS.finder, ago: 6 * DAY, text: "Satchel.dmg" },
  { kind: "text", app: APPS.mail, ago: 8 * DAY, text: "1 Infinite Loop\nCupertino, CA 95014" },
];

const FILLER_TEXT = [
  "Remember to rotate the staging keys before Friday.",
  "Lunch order: two falafel wraps, one without onions.",
  "The build is green again after pinning the toolchain.",
  "Draft reply: happy to take a look early next week.",
  "Flight UA 924 lands at 16:40, terminal 2.",
];
const FILLER_CODE = [
  "SELECT id, preview FROM clips ORDER BY last_used_at DESC, id DESC LIMIT ?1;",
  "pnpm build && du -sh build/_app",
  "fn main() {\n    println!(\"hello\");\n}",
  "export const ssr = false;",
];
const FILLER_LINKS = [
  "https://svelte.dev/docs/svelte/$state",
  "https://www.sqlite.org/fts5.html",
  "https://github.com/tauri-apps/tauri",
  "https://developer.mozilla.org/en-US/docs/Web/HTTP/CSP",
];
const FILLER_COLORS = ["#FF375F", "#64D2FF", "#30D158", "#BF5AF2", "#FFD60A", "#AC8E68"];
const FILLER_APPS = [APPS.notes, APPS.safari, APPS.terminal, APPS.slack, APPS.vscode, APPS.mail];

function buildSamples(): Sample[] {
  const out = [...SAMPLES];
  for (let i = 0; i < 110; i++) {
    const ago = 9 * DAY + i * 7 * HR;
    const app = FILLER_APPS[i % FILLER_APPS.length];
    const pick = i % 5;
    if (pick === 0) out.push({ kind: "code", app, ago, text: FILLER_CODE[i % FILLER_CODE.length] });
    else if (pick === 1) out.push({ kind: "link", app, ago, text: FILLER_LINKS[i % FILLER_LINKS.length] });
    else if (pick === 2) out.push({ kind: "color", app, ago, text: FILLER_COLORS[i % FILLER_COLORS.length] });
    else out.push({ kind: "text", app, ago, text: `${FILLER_TEXT[i % FILLER_TEXT.length]} (#${i + 1})` });
  }
  return out;
}

const PNG_1X1 = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";

function staticPng(): ArrayBuffer {
  const bin = atob(PNG_1X1);
  const bytes = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
  return bytes.buffer;
}

async function drawThumbnail(seed: number, width = 320, height = 180): Promise<ArrayBuffer> {
  if (typeof OffscreenCanvas === "undefined") return staticPng();
  const c = new OffscreenCanvas(width, height);
  const g = c.getContext("2d");
  if (!g) return staticPng();
  const hue = (seed * 67) % 360;
  const grad = g.createLinearGradient(0, 0, 320, 180);
  grad.addColorStop(0, `hsl(${hue} 70% 62%)`);
  grad.addColorStop(1, `hsl(${(hue + 60) % 360} 70% 42%)`);
  g.fillStyle = grad;
  g.fillRect(0, 0, 320, 180);
  g.fillStyle = "rgba(255,255,255,0.85)";
  g.fillRect(24, 24, 180, 14);
  g.fillRect(24, 50, 120, 10);
  g.fillStyle = "rgba(255,255,255,0.35)";
  g.beginPath();
  g.arc(250, 120, 40, 0, Math.PI * 2);
  g.fill();
  const blob = await c.convertToBlob({ type: "image/png" });
  return blob.arrayBuffer();
}

/** Installed apps for the excluded-apps picker, unsorted like a folder scan. */
const INSTALLED_APPS: [string, string, string][] = [
  ["com.apple.Safari", "Safari", "#0A84FF"],
  ["com.apple.Notes", "Notes", "#FFD60A"],
  ["com.apple.mail", "Mail", "#0A84FF"],
  ["com.apple.TextEdit", "TextEdit", "#8E8E93"],
  ["com.apple.Terminal", "Terminal", "#2B2B2E"],
  ["com.apple.calculator", "Calculator", "#FF9F0A"],
  ["com.apple.iWork.Pages", "Pages", "#FF9F0A"],
  ["com.apple.keychainaccess", "Keychain Access", "#8E8E93"],
  ["com.apple.Passwords", "Passwords", "#30D158"],
  ["com.apple.dt.Xcode", "Xcode", "#0A84FF"],
  ["com.1password.1password", "1Password", "#1A8CFF"],
  ["com.bitwarden.desktop", "Bitwarden", "#175DDC"],
  ["com.figma.Desktop", "Figma", "#A259FF"],
  ["com.tinyspeck.slackmacgap", "Slack", "#4A154B"],
  ["com.microsoft.VSCode", "Visual Studio Code", "#007ACC"],
  ["org.mozilla.firefox", "Firefox", "#FF7139"],
  ["com.spotify.client", "Spotify", "#1DB954"],
];
/** Running apps that are not in an Applications folder. */
const RUNNING_ONLY: [string, string, string][] = [["com.apple.finder", "Finder", "#1E90FF"]];
const RUNNING = new Set(["com.apple.finder", "com.apple.Safari", "com.apple.Notes", "com.tinyspeck.slackmacgap"]);
/** What the mock's "Choose Other App…" panel picks. */
const CHOSEN_APP: [string, string, string] = ["com.example.PortableVault", "Portable Vault", "#B4235A"];

async function drawAppIcon(name: string, color: string): Promise<ArrayBuffer> {
  if (typeof OffscreenCanvas === "undefined") return staticPng();
  const c = new OffscreenCanvas(64, 64);
  const g = c.getContext("2d");
  if (!g) return staticPng();
  g.fillStyle = color;
  g.beginPath();
  g.roundRect(6, 6, 52, 52, 12);
  g.fill();
  g.fillStyle = "#fff";
  g.font = "600 28px -apple-system, system-ui, sans-serif";
  g.textAlign = "center";
  g.textBaseline = "middle";
  g.fillText(Array.from(name)[0] ?? "?", 32, 34);
  const blob = await c.convertToBlob({ type: "image/png" });
  return blob.arrayBuffer();
}

function defaultSettings(): Settings {
  return {
    maxItems: 2000,
    retentionDays: 90,
    // A slice of the real defaults (installed, not installed and other-platform
    // entries; only the installed ones are listed) plus one user-added entry
    // for an app that is not installed (listed by identifier).
    excludedAppIds: [
      "com.1password.1password",
      "com.agilebits.onepassword7",
      "com.agilebits.onepassword-osx",
      "com.bitwarden.desktop",
      "org.keepassxc.keepassxc",
      // Differs from the line above only in case, like the real defaults.
      "org.keepassxc.KeePassXC",
      "com.apple.keychainaccess",
      "com.apple.Passwords",
      "1Password.exe",
      "KeePass.exe",
      "keepassxc",
      "seahorse",
      "com.example.OldVault",
    ],
    capturePaused: false,
    captureImages: true,
    shelfShortcut: "CommandOrControl+Shift+V",
    pasteOnSelect: true,
    // On by default, as in the app.
    linkPreviews: true,
    checkForUpdates: true,
    // Undecided (on) until the first launch registers.
    openAtLogin: null,
  };
}

function previewOf(kind: ClipKind, text: string): string {
  if (kind === "image") return "";
  return Array.from(text).slice(0, PREVIEW_CHARS).join("");
}

function isOlder(r: Row, c: Cursor): boolean {
  return r.lastUsedAt < c[0] || (r.lastUsedAt === c[0] && r.id < c[1]);
}

function summary(r: Row): ClipSummary {
  const { text: _text, ...s } = r;
  return {
    ...s,
    sourceApp: s.sourceApp ? { ...s.sourceApp } : null,
    pinboardIds: [...s.pinboardIds],
    linkPreview: s.linkPreview ? { ...s.linkPreview } : null,
  };
}

export interface MockOptions {
  now?: () => number;
}

export interface MockBackend {
  transport: Transport;
  /** Every command received, in order. */
  log: { cmd: string; args: Record<string, unknown> | undefined }[];
  count(): number;
  /** Pretend the user copied some text in another app. Returns the new id. */
  simulateCopy(text: string, app?: SourceApp): ClipId;
  /** Pretend the user copied something over Satchel's size cap. */
  simulateOversizedCopy(kind: ClipKind, limitBytes: number): void;
  /** Pretend the global shortcut opened the shelf. */
  show(): void;
  /** Pretend Satchel does (not) hold the Accessibility permission. */
  setPastePermission(granted: boolean): void;
  /** Change the pause state without announcing it, as if `settings-changed` were missed. */
  setPausedSilently(paused: boolean): void;
  /** Put a newer version on the (pretend) update feed; the next check finds it. */
  publishUpdate(version: string | null): void;
  /** Make the next install fail at `during`, or succeed again with null. */
  failNextInstall(during: UpdateFailure | null): void;
}

export function createMockBackend(opts: MockOptions = {}): MockBackend {
  const now = opts.now ?? (() => Date.now());
  const start = now();
  let rows: Row[] = [];
  let nextId = 1;
  let pastePermission = true;
  const samples = buildSamples();
  // Oldest first so ids grow with time, like a real insert order.
  for (let i = samples.length - 1; i >= 0; i--) {
    const s = samples[i];
    const at = start - s.ago;
    rows.push({
      id: nextId++,
      kind: s.kind,
      title: s.title ?? null,
      preview: previewOf(s.kind, s.text),
      text: s.kind === "image" ? null : s.text,
      sourceApp: { id: s.app[0], name: s.app[1] },
      createdAt: at,
      lastUsedAt: at,
      byteSize: s.bytes ?? new TextEncoder().encode(s.text).length,
      hasThumbnail: s.kind === "image",
      pinboardIds: s.pins ? [...s.pins] : [],
      linkPreview: s.link ? { ...s.link } : null,
    });
  }
  let pinboards: Pinboard[] = [
    { id: 1, name: "Snippets", color: "#FF9F0A", position: 0 },
    { id: 2, name: "Work", color: "#0A84FF", position: 1 },
  ];
  let nextBoard = 3;
  let settings = defaultSettings();
  // Pretend update feed: nothing newer until `publishUpdate`.
  let feed: string | null = null;
  let installFailure: UpdateFailure | null = null;
  let update: UpdateStatus = { supported: true, currentVersion: "0.1.0", checking: false, phase: { state: "idle" }, lastCheck: null };
  const setUpdate = (next: UpdateStatus) => {
    update = next;
    emit("update-status", structuredClone(update));
  };
  const listeners = new Map<string, Set<(p: unknown) => void>>();
  const log: MockBackend["log"] = [];

  function emit(event: EventName, payload: unknown) {
    for (const h of [...(listeners.get(event) ?? [])]) h(payload);
  }

  function find(id: unknown): Row {
    const r = rows.find((x) => x.id === id);
    if (!r) throw "clip not found";
    return r;
  }

  function bump(r: Row) {
    r.lastUsedAt = Math.max(now(), ...rows.map((x) => x.lastUsedAt + 1));
  }

  function list(query: Query): ClipSummary[] {
    const limit = query.limit === 0 ? 50 : Math.min(200, Math.max(1, query.limit));
    const needle = query.text?.trim().toLowerCase() || null;
    return rows
      .filter((r) => query.kind == null || r.kind === query.kind)
      .filter((r) => query.pinboardId == null || r.pinboardIds.includes(query.pinboardId))
      .filter((r) => {
        if (!needle) return true;
        const hay = `${r.text ?? r.preview} ${r.title ?? ""} ${r.linkPreview?.title ?? ""} ${r.sourceApp?.name ?? ""}`.toLowerCase();
        return hay.includes(needle);
      })
      .filter((r) => !query.before || isOlder(r, query.before))
      .sort((a, b) => b.lastUsedAt - a.lastUsedAt || b.id - a.id)
      .slice(0, limit)
      .map(summary);
  }

  type Args = Record<string, any>; // eslint-disable-line @typescript-eslint/no-explicit-any
  const handlers: Record<string, (a: Args) => unknown> = {
    list_clips: (a) => list(a.query as Query),
    get_clip_text: (a) => find(a.id).text,
    get_clip_thumbnail: (a) => {
      const r = find(a.id);
      if (!r.hasThumbnail) throw "clip has no thumbnail";
      return drawThumbnail(r.id);
    },
    get_link_image: (a) => {
      const r = find(a.id);
      const p = r.linkPreview;
      if (!p?.image) throw "not found";
      return p.image === "photo" ? drawThumbnail(r.id, 480, 250) : drawAppIcon(p.site, "#0A84FF");
    },
    paste_clip: (a) => {
      bump(find(a.id));
      emit("clips-changed", { reason: "bumped", id: a.id });
      if (!pastePermission) throw ACCESSIBILITY_ERROR;
    },
    paste_clips: (a) => {
      for (const id of a.ids as ClipId[]) bump(find(id));
      emit("clips-changed", { reason: "bumped" });
      if (!pastePermission) throw ACCESSIBILITY_ERROR;
    },
    copy_clip: (a) => {
      bump(find(a.id));
      emit("clips-changed", { reason: "bumped", id: a.id });
    },
    delete_clip: (a) => {
      const before = rows.length;
      rows = rows.filter((r) => r.id !== a.id);
      const removed = rows.length !== before;
      if (removed) emit("clips-changed", { reason: "deleted", id: a.id });
      return removed;
    },
    set_clip_title: (a) => {
      const t = a.title as string | null;
      find(a.id).title = t && t.trim() ? t : null;
      emit("clips-changed", { reason: "updated", id: a.id });
    },
    clear_history: () => {
      const before = rows.length;
      rows = rows.filter((r) => r.pinboardIds.length > 0);
      emit("clips-changed", { reason: "cleared" });
      return before - rows.length;
    },
    list_pinboards: () => pinboards.map((p) => ({ ...p })).sort((a, b) => a.position - b.position),
    create_pinboard: (a) => {
      const p: Pinboard = {
        id: nextBoard++,
        name: String(a.name),
        color: String(a.color),
        position: Math.max(-1, ...pinboards.map((x) => x.position)) + 1,
      };
      pinboards.push(p);
      emit("pinboards-changed", null);
      return { ...p };
    },
    rename_pinboard: (a) => {
      const p = pinboards.find((x) => x.id === a.id);
      if (!p) throw "pinboard not found";
      p.name = String(a.name);
      emit("pinboards-changed", null);
    },
    delete_pinboard: (a) => {
      pinboards = pinboards.filter((x) => x.id !== a.id);
      for (const r of rows) r.pinboardIds = r.pinboardIds.filter((id) => id !== a.id);
      emit("pinboards-changed", null);
    },
    pin_clip: (a) => {
      const r = find(a.clipId);
      if (!pinboards.some((p) => p.id === a.pinboardId)) throw "pinboard not found";
      if (!r.pinboardIds.includes(a.pinboardId)) r.pinboardIds.push(a.pinboardId);
      emit("pinboards-changed", null);
    },
    unpin_clip: (a) => {
      const r = find(a.clipId);
      r.pinboardIds = r.pinboardIds.filter((id) => id !== a.pinboardId);
      emit("pinboards-changed", null);
    },
    get_settings: () => structuredClone(settings),
    save_settings: (a) => {
      const was = settings.linkPreviews;
      // Like Rust: the form never changes "open at login".
      settings = { ...structuredClone(a.settings as Settings), openAtLogin: settings.openAtLogin };
      emit("settings-changed", structuredClone(settings));
      if (was && !settings.linkPreviews) {
        // Like Rust: turning previews off deletes the cached ones.
        for (const r of rows) r.linkPreview = null;
        emit("clips-changed", { reason: "updated" });
      }
    },
    set_capture_paused: (a) => {
      settings = { ...settings, capturePaused: !!a.paused };
      emit("settings-changed", structuredClone(settings));
    },
    // A browser is never a login item: shown as on and changeable, like the app in /Applications.
    get_login_item: () => ({ enabled: settings.openAtLogin ?? true, requiresApproval: false, unavailable: null }),
    set_open_at_login: (a) => {
      // Like Rust: stored, not announced.
      settings = { ...settings, openAtLogin: !!a.enabled };
      return { enabled: !!a.enabled, requiresApproval: false, unavailable: null };
    },
    hide_shelf: () => undefined,
    // There is no second window in a plain browser; open /settings by hand.
    open_settings: () => undefined,
    close_settings: () => undefined,
    open_accessibility_settings: () => undefined,
    list_apps: () => {
      const seen = new Set<string>();
      const out: AppInfo[] = [];
      for (const [id, name] of [...INSTALLED_APPS, ...RUNNING_ONLY]) {
        if (seen.has(id.toLowerCase())) continue;
        seen.add(id.toLowerCase());
        out.push({ id, name, running: RUNNING.has(id) });
      }
      return out.sort((a, b) => a.name.toLowerCase().localeCompare(b.name.toLowerCase()));
    },
    resolve_apps: (a) =>
      (a.ids as string[]).map((id) => {
        const hit = knownApp(id);
        return hit ? { id: hit[0], name: hit[1], running: false } : null;
      }),
    get_app_icon: (a) => {
      const hit = knownApp(String(a.id));
      if (!hit) throw "not found";
      return drawAppIcon(hit[1], hit[2]);
    },
    choose_app: () => ({ id: CHOSEN_APP[0], name: CHOSEN_APP[1], running: false }),
    get_update_status: () => structuredClone(update),
    check_for_updates: () => {
      setUpdate({ ...update, checking: true });
      const phase: UpdateStatus["phase"] = feed
        ? update.phase.state === "failed" && update.phase.version === feed
          ? update.phase
          : { state: "available", version: feed }
        : { state: "idle" };
      setUpdate({ ...update, checking: false, phase, lastCheck: { at: now(), outcome: feed ? "available" : "upToDate" } });
    },
    install_update: () => {
      const p = update.phase;
      if (p.state !== "available" && p.state !== "failed") return;
      const version = p.version;
      setUpdate({ ...update, phase: { state: "downloading", version, downloaded: 0, total: 8_000_000 } });
      setUpdate({ ...update, phase: { state: "downloading", version, downloaded: 4_000_000, total: 8_000_000 } });
      if (installFailure) {
        setUpdate({ ...update, phase: { state: "failed", version, during: installFailure } });
        return;
      }
      // The app would now replace itself and reopen; the mock stays here.
      setUpdate({ ...update, phase: { state: "installing", version } });
    },
    get_third_party_notices: () =>
      "THIRD-PARTY SOFTWARE NOTICES FOR SATCHEL\n\n(Mock mode: the app shows THIRD-PARTY-NOTICES.txt from its bundle.)\n",
  };

  function knownApp(id: string): [string, string, string] | undefined {
    const k = id.toLowerCase();
    return [...INSTALLED_APPS, ...RUNNING_ONLY, CHOSEN_APP].find(([x]) => x.toLowerCase() === k);
  }

  const transport: Transport = {
    async invoke(cmd, args) {
      log.push({ cmd, args });
      const h = handlers[cmd];
      if (!h) throw `unknown command: ${cmd}`;
      return h(args ?? {});
    },
    async listen(event, handler) {
      let set = listeners.get(event);
      if (!set) listeners.set(event, (set = new Set()));
      set.add(handler);
      return () => set.delete(handler);
    },
  };

  return {
    transport,
    log,
    count: () => rows.length,
    simulateCopy(text, app = { id: "com.apple.TextEdit", name: "TextEdit" }) {
      const at = Math.max(now(), ...rows.map((x) => x.lastUsedAt + 1));
      const id = nextId++;
      rows.push({
        id,
        kind: "text",
        title: null,
        preview: previewOf("text", text),
        text,
        sourceApp: app,
        createdAt: at,
        lastUsedAt: at,
        byteSize: new TextEncoder().encode(text).length,
        hasThumbnail: false,
        pinboardIds: [],
        linkPreview: null,
      });
      emit("clips-changed", { reason: "inserted", id });
      return id;
    },
    simulateOversizedCopy(kind, limitBytes) {
      emit("capture-skipped", { kind, limitBytes });
    },
    show() {
      emit("shelf-shown", null);
    },
    setPastePermission(granted) {
      pastePermission = granted;
    },
    setPausedSilently(paused) {
      settings = { ...settings, capturePaused: paused };
    },
    publishUpdate(version) {
      feed = version;
    },
    failNextInstall(during) {
      installFailure = during;
    },
  };
}

// A wallpaper behind the transparent page, so the panel's translucency is visible in a browser.
const BACKDROP_CSS = `
body { background:
  radial-gradient(900px 700px at 12% 18%, #8fb8ff, transparent 70%),
  radial-gradient(900px 700px at 88% 30%, #f2a7d0, transparent 70%),
  radial-gradient(1000px 700px at 50% 100%, #9be0d2, transparent 70%), #5b7be0; }
@media (prefers-color-scheme: dark) { body { background:
  radial-gradient(900px 700px at 12% 18%, #3b4fb8, transparent 70%),
  radial-gradient(900px 700px at 88% 30%, #8a3f86, transparent 70%),
  radial-gradient(1000px 700px at 50% 100%, #1f6f78, transparent 70%), #10122a; } }`;

/** Transport for `pnpm dev` in a browser. Exposes `window.satchelMock` for poking at it. */
export function createMockTransport(): Transport {
  const m = createMockBackend();
  if (typeof window !== "undefined") {
    (window as unknown as { satchelMock: MockBackend }).satchelMock = m;
    // Constructed stylesheet: CSSOM, so it works under a strict style-src too.
    const sheet = new CSSStyleSheet();
    sheet.replaceSync(BACKDROP_CSS);
    document.adoptedStyleSheets = [...document.adoptedStyleSheets, sheet];
  }
  return m.transport;
}
