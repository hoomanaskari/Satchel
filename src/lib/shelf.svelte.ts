// Shelf state and actions. Components read these fields and call these
// methods; all decisions live here and in the pure modules it uses.
// Only `$state` fields, getters and methods: no effects, no timers other
// than the search debounce, which exists only while a search is pending.

import { APP_NAME } from "./app-name";
import { debounce } from "./debounce";
import { resultLabel, skippedNotice } from "./format";
import type { Api } from "./ipc";
import type { KeyAction } from "./keyboard";
import { Pager } from "./paging";
import type { Filters, KindScope } from "./query";
import * as sel from "./selection";
import type {
  CaptureSkipped,
  ClipId,
  ClipKind,
  ClipSummary,
  ClipsChanged,
  Pinboard,
  PinboardId,
  Settings,
  UpdateStatus,
} from "./types";
import { shelfUpdate, type ShelfUpdate } from "./updates";

export type ClickMode = "single" | "toggle" | "range";

export interface ShelfOptions {
  api: Api;
  pageSize?: number;
  debounceMs?: number;
  now?: () => number;
}

export interface PreviewState {
  id: ClipId;
  kind: ClipKind;
  text: string | null;
  loading: boolean;
}

export const PINBOARD_COLORS = ["#FF9F0A", "#0A84FF", "#30D158", "#FF375F", "#BF5AF2", "#64D2FF", "#FFD60A", "#AC8E68"];
const DEFAULT_PINBOARD = "Pinned";

function message(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/** Prefix of the error Rust returns when a paste was refused for lack of the Accessibility permission. */
export const ACCESSIBILITY_ERROR_PREFIX = "accessibility-permission-required";
/** Shown in the top bar's notice, beside the button that opens the right System Settings page. */
export const ACCESSIBILITY_NOTICE =
  `Copied, not pasted: ${APP_NAME} needs Accessibility access. If it is already on, remove it and add it again.`;

/**
 * What the top bar's notice says, when something needs the user's attention.
 * Confirmations of an action are not notices (the action's visible result is
 * the confirmation); they only go to screen readers (`announcement`). All
 * notices are alerts (role="alert"). `warning`: something happened that the
 * user cannot otherwise see (a copy too large to keep). `permission` comes
 * with the button that opens the Accessibility settings.
 */
export interface Notice {
  text: string;
  tone: "warning" | "error" | "permission";
}

export class Shelf {
  text = $state("");
  kind = $state<KindScope>("all");
  pinboardId = $state<PinboardId | null>(null);

  clips = $state.raw<ClipSummary[]>([]);
  hasMore = $state(false);
  loading = $state(false);
  error = $state<string | null>(null);

  pinboards = $state.raw<Pinboard[]>([]);
  settings = $state.raw<Settings | null>(null);
  /** Where updates stand (Rust sends every change as `update-status`). */
  updateStatus = $state.raw<UpdateStatus | null>(null);
  selection = $state.raw<sel.Selection>(sel.empty());
  firstVisible = $state(0);

  previewOpen = $state(false);
  preview = $state.raw<PreviewState | null>(null);

  /**
   * A paste was refused because Satchel lacks the Accessibility permission.
   * Rust then leaves the shelf up so the shelf can say so and offer the way to grant it.
   */
  needsPastePermission = $state(false);
  /** Transient message (an action's result or error); cleared by the next selection change. */
  flash = $state.raw<Notice | null>(null);
  /** The latest action's confirmation, for screen readers only; cleared by the next selection change. */
  announcement = $state("");
  /** Reference time for relative times; refreshed on load instead of by a timer. */
  now = $state(0);
  visible = $state(true);
  /** Bumped when the list starts over (new filters); the view scrolls back to the start. */
  resetTick = $state(0);
  /** Bumped when the search field should take focus. */
  focusSearchTick = $state(0);

  private readonly api: Api;
  private readonly clock: () => number;
  private readonly pager: Pager;
  private readonly search;
  private stale = false;
  /** Said when the shelf next opens (see `onCaptureSkipped`). */
  private pendingNotice: Notice | null = null;
  private previewToken = 0;

  constructor(opts: ShelfOptions) {
    this.api = opts.api;
    this.clock = opts.now ?? Date.now;
    this.now = this.clock();
    this.pager = new Pager((q) => this.api.listClips(q), opts.pageSize ?? 50, () => this.mirror());
    this.search = debounce(() => void this.reload(), opts.debounceMs ?? 40);
  }

  // ---- derived ----

  get filters(): Filters {
    return { text: this.text, kind: this.kind, pinboardId: this.pinboardId };
  }

  get order(): ClipId[] {
    return this.clips.map((c) => c.id);
  }

  get selectedClips(): ClipSummary[] {
    const ids = new Set(this.selection.ids);
    return this.clips.filter((c) => ids.has(c.id));
  }

  get focusClip(): ClipSummary | null {
    const f = this.selection.focus;
    return f == null ? null : (this.clips.find((c) => c.id === f) ?? null);
  }

  /**
   * The top bar's notice, or null when there is nothing to say. The latest
   * error or warning wins, then the missing permission, then a failed list
   * load. Confirmations and the selection are never shown: each card shows
   * its app, kind and time, and the paste buttons count a multi-selection.
   */
  get notice(): Notice | null {
    if (this.flash) return this.flash;
    if (this.needsPastePermission) return { text: ACCESSIBILITY_NOTICE, tone: "permission" };
    if (this.error) return { text: this.error, tone: "error" };
    return null;
  }

  /** Confirms an action to screen readers; nothing is shown. */
  say(text: string): void {
    this.announcement = text;
  }

  /** Shows an error (messages from Rust never contain clip content). */
  fail(e: unknown): void {
    this.flash = { text: message(e), tone: "error" };
  }

  get resultLabel(): string {
    return resultLabel(this.clips.length, this.hasMore);
  }

  get isEmpty(): boolean {
    return !this.loading && this.clips.length === 0;
  }

  get activePinboard(): Pinboard | null {
    return this.pinboards.find((p) => p.id === this.pinboardId) ?? null;
  }

  /**
   * Capture is paused (from the shelf, the tray menu or the Settings window):
   * the header shows it until it resumes. Follows `settings`, which every
   * pause change updates through `settings-changed` and each open reloads.
   */
  get paused(): boolean {
    return this.settings?.capturePaused ?? false;
  }

  /**
   * The top bar's update control while an update is on offer: a state like
   * "Paused", not a notice. It never appears by itself in front of the user:
   * the shelf shows it only when opened.
   */
  get update(): ShelfUpdate | null {
    return shelfUpdate(this.updateStatus);
  }

  get pasteOnSelect(): boolean {
    return this.settings?.pasteOnSelect ?? true;
  }

  /** True when a normal paste puts text clips down as plain text ("Paste text as"). */
  get pastePlainText(): boolean {
    return this.settings?.pastePlainText ?? false;
  }

  /**
   * Labels of the paste actions (top-bar buttons, clip menu). They say
   * "Copy" when selecting only copies, and count a multi-selection when it
   * is pasted (copying takes the focused clip only, so it is not counted).
   * The other action (`alternate`, Shift+Return) pastes text in the format
   * the "Paste text as" setting does not choose; `alternateShort` is its
   * label in a narrow window.
   */
  get pasteLabels(): { paste: string; alternate: string; alternateShort: string } {
    const verb = this.pasteOnSelect ? "Paste" : "Copy";
    const n = this.selection.ids.length;
    const paste = this.pasteOnSelect && n > 1 ? `Paste ${n} Clips` : verb;
    return this.pastePlainText
      ? { paste, alternate: `${verb} with Formatting`, alternateShort: "Formatted" }
      : { paste, alternate: `${verb} as Plain Text`, alternateShort: "Plain Text" };
  }

  /**
   * How a paste or copy of `ids` is announced: as plain text, or with the
   * formatting the other action asked for. Only text clips have a format
   * (images and files are announced as they are).
   */
  private formatSuffix(ids: ClipId[], alternate: boolean): string {
    const text = ids.some((id) => {
      const kind = this.clips.find((c) => c.id === id)?.kind;
      return kind !== "image" && kind !== "file";
    });
    if (!text) return "";
    const plain = alternate !== this.pastePlainText;
    return plain ? " as plain text" : alternate ? " with formatting" : "";
  }

  // ---- lifecycle ----

  async init(): Promise<() => void> {
    const stops = await Promise.all([
      this.api.onClipsChanged((p) => void this.onClipsChanged(p)),
      this.api.onPinboardsChanged(() => void this.onPinboardsChanged()),
      this.api.onShelfShown(() => void this.onShown()),
      this.api.onSettingsChanged((s) => (this.settings = s)),
      this.api.onCaptureSkipped((p) => this.onCaptureSkipped(p)),
      this.api.onUpdateStatus((s) => (this.updateStatus = s)),
    ]);
    await Promise.all([this.loadSettings(), this.loadPinboards(), this.loadUpdateStatus(), this.reload()]);
    return () => {
      this.search.cancel();
      for (const stop of stops) stop();
    };
  }

  /**
   * A copy was over a size cap and not kept. Said right away when the shelf
   * is up, otherwise the next time it opens, so the clip is not just missing.
   */
  onCaptureSkipped(p: CaptureSkipped): void {
    const notice: Notice = { text: skippedNotice(p), tone: "warning" };
    if (this.visible) this.flash = notice;
    else this.pendingNotice = notice;
  }

  setVisible(v: boolean): void {
    this.visible = v;
    if (!v) this.search.cancel();
  }

  async onShown(): Promise<void> {
    this.visible = true;
    this.search.cancel();
    this.text = "";
    this.previewOpen = false;
    this.preview = null;
    this.flash = null;
    this.announcement = "";
    this.needsPastePermission = false;
    this.stale = false;
    this.focusSearchTick++;
    // Settings too, so a missed `settings-changed` (the pause state above
    // all) never leaves the header stale.
    await Promise.all([this.loadSettings(), this.loadUpdateStatus(), this.reload()]);
    // After the reload, whose new selection clears the notice.
    if (this.pendingNotice) {
      this.flash = this.pendingNotice;
      this.pendingNotice = null;
    }
  }

  // ---- loading ----

  private mirror(): void {
    this.clips = this.pager.items;
    this.hasMore = this.pager.hasMore;
    this.loading = this.pager.loading;
    this.error = this.pager.error;
  }

  private async loadSettings(): Promise<void> {
    try {
      this.settings = await this.api.getSettings();
    } catch (e) {
      this.fail(e);
    }
  }

  private async loadUpdateStatus(): Promise<void> {
    try {
      this.updateStatus = await this.api.getUpdateStatus();
    } catch {
      // Not worth a notice: the control just stays hidden.
    }
  }

  private async loadPinboards(): Promise<void> {
    try {
      this.pinboards = await this.api.listPinboards();
    } catch (e) {
      this.fail(e);
    }
  }

  /** Start the list over for the current filters and select the first clip. */
  async reload(): Promise<void> {
    this.search.cancel();
    this.now = this.clock();
    this.resetTick++;
    this.firstVisible = 0;
    if (await this.pager.reset(this.filters)) {
      this.setSelection(this.clips.length ? sel.single(this.clips[0].id) : sel.empty());
    }
  }

  /** Reload in place after a change, keeping the selection where possible. */
  private async refresh(): Promise<void> {
    this.now = this.clock();
    if (await this.pager.refresh()) this.setSelection(sel.prune(this.selection, this.order), false);
  }

  async loadMoreIfNeeded(renderedEnd: number): Promise<void> {
    if (this.pager.wantsMore(renderedEnd)) await this.pager.loadMore();
  }

  /** Run a pending search now (tests, and Return pressed right after typing). */
  async flushSearch(): Promise<void> {
    if (this.search.pending()) await this.reload();
  }

  // ---- filters ----

  setText(v: string): void {
    this.text = v;
    this.search();
  }

  setKind(k: KindScope): Promise<void> {
    this.kind = k;
    return this.reload();
  }

  setTab(id: PinboardId | null): Promise<void> {
    this.pinboardId = id;
    return this.reload();
  }

  setFirstVisible(i: number): void {
    this.firstVisible = i;
  }

  // ---- selection ----

  private setSelection(s: sel.Selection, clearFlash = true): void {
    const prevFocus = this.selection.focus;
    this.selection = s;
    if (clearFlash) {
      this.flash = null;
      this.announcement = "";
    }
    if (this.previewOpen && s.focus !== prevFocus) void this.loadPreview();
  }

  select(id: ClipId, mode: ClickMode): void {
    const s =
      mode === "toggle"
        ? sel.toggle(this.selection, id)
        : mode === "range"
          ? sel.range(this.selection, id, this.order)
          : sel.single(id);
    this.setSelection(s);
  }

  move(delta: number, extend: boolean): void {
    this.setSelection(sel.move(this.selection, delta, this.order, extend));
  }

  edge(end: boolean): void {
    const order = this.order;
    if (order.length) this.setSelection(sel.single(end ? order[order.length - 1] : order[0]));
  }

  selectAll(): void {
    this.setSelection(sel.all(this.order, this.selection.focus));
  }

  // ---- clip actions ----

  private async attempt(fn: () => Promise<void>): Promise<void> {
    try {
      await fn();
    } catch (e) {
      this.fail(e);
    }
  }

  private selectedIds(): ClipId[] {
    return sel.ordered(this.selection, this.order);
  }

  /** Pastes (or copies) the selection; `alternate` asks for the other text format. */
  async paste(alternate: boolean): Promise<void> {
    await this.flushSearch();
    const ids = this.selectedIds();
    if (ids.length === 0) return;
    try {
      if (!this.pasteOnSelect) {
        // No multi-copy command exists; the focused clip is copied.
        await this.api.copyClip(this.selection.focus ?? ids[0], alternate);
        await this.api.hideShelf();
      } else if (ids.length > 1) {
        await this.api.pasteClips(ids, alternate);
      } else {
        await this.api.pasteClip(ids[0], alternate);
      }
      // Seen only where the window stays up (browser development); Rust hides the shelf.
      const what = !this.pasteOnSelect ? "Copied" : ids.length > 1 ? `Pasted ${ids.length} clips` : "Pasted";
      const copied = !this.pasteOnSelect ? [this.selection.focus ?? ids[0]] : ids;
      this.say(what + this.formatSuffix(copied, alternate));
      this.needsPastePermission = false;
      this.afterHide();
    } catch (e) {
      const text = message(e);
      if (text.startsWith(ACCESSIBILITY_ERROR_PREFIX)) {
        this.needsPastePermission = true;
        this.flash = null;
      } else {
        this.fail(text);
      }
    }
  }

  /** Opens System Settings at the Accessibility list; Rust hides the shelf first. */
  async openAccessibilitySettings(): Promise<void> {
    this.search.cancel();
    await this.attempt(() => this.api.openAccessibilitySettings());
    this.needsPastePermission = false;
    this.afterHide();
  }

  async pasteNth(n: number, alternate: boolean): Promise<void> {
    const clip = this.clips[this.firstVisible + n - 1];
    if (!clip) return;
    this.setSelection(sel.single(clip.id));
    await this.paste(alternate);
  }

  async copy(alternate: boolean): Promise<void> {
    const id = this.selection.focus;
    if (id == null) return;
    await this.attempt(async () => {
      await this.api.copyClip(id, alternate);
      this.say("Copied to the clipboard" + this.formatSuffix([id], alternate));
    });
  }

  async close(): Promise<void> {
    this.search.cancel();
    await this.attempt(() => this.api.hideShelf());
    this.afterHide();
  }

  private afterHide(): void {
    this.previewOpen = false;
    this.preview = null;
    // Rust hides the window; in a plain browser (mock) nothing hides.
    if (!this.api.isMock?.()) this.setVisible(false);
  }

  private async defaultPinboard(): Promise<Pinboard | null> {
    if (this.activePinboard) return this.activePinboard;
    if (this.pinboards.length) return [...this.pinboards].sort((a, b) => a.position - b.position)[0];
    const p = await this.api.createPinboard(DEFAULT_PINBOARD, PINBOARD_COLORS[0]);
    this.pinboards = [...this.pinboards, p];
    return p;
  }

  /** Cmd+P: pin the selection (or `ids`) to the open pinboard, else the first; unpin if all are pinned there. */
  async togglePin(ids?: ClipId[]): Promise<void> {
    const want = ids ? new Set(ids) : null;
    const targets = want ? this.clips.filter((c) => want.has(c.id)) : this.selectedClips;
    if (targets.length === 0) return;
    await this.attempt(async () => {
      const board = await this.defaultPinboard();
      if (!board) return;
      const unpin = targets.every((c) => c.pinboardIds.includes(board.id));
      await this.setMembership(
        targets.map((c) => c.id),
        board.id,
        !unpin,
      );
    });
  }

  /** Toggle one clip in one pinboard (card menu). */
  async togglePinboard(clipId: ClipId, boardId: PinboardId): Promise<void> {
    const clip = this.clips.find((c) => c.id === clipId);
    if (!clip) return;
    await this.attempt(() => this.setMembership([clipId], boardId, !clip.pinboardIds.includes(boardId)));
  }

  private async setMembership(ids: ClipId[], boardId: PinboardId, pin: boolean): Promise<void> {
    for (const id of ids) {
      const clip = this.clips.find((c) => c.id === id);
      if (!clip || clip.pinboardIds.includes(boardId) === pin) continue;
      if (pin) await this.api.pinClip(id, boardId);
      else await this.api.unpinClip(id, boardId);
      const pinboardIds = pin ? [...clip.pinboardIds, boardId] : clip.pinboardIds.filter((b) => b !== boardId);
      this.pager.patch(id, { pinboardIds });
    }
    if (!pin && this.pinboardId === boardId) this.removeLocally(ids);
    const board = this.pinboards.find((p) => p.id === boardId);
    const what = ids.length === 1 ? "Clip" : `${ids.length} clips`;
    this.say(`${what} ${pin ? "pinned to" : "removed from"} ${board?.name ?? "pinboard"}`);
  }

  private removeLocally(ids: ClipId[]): void {
    const order = this.order;
    const next = sel.afterRemoval(this.selection, ids, order);
    this.pager.remove(ids);
    this.setSelection(next);
  }

  async deleteSelected(): Promise<void> {
    const ids = this.selectedIds();
    if (ids.length === 0) return;
    await this.attempt(async () => {
      const done: ClipId[] = [];
      try {
        for (const id of ids) {
          await this.api.deleteClip(id);
          done.push(id);
        }
      } finally {
        if (done.length) this.removeLocally(done);
      }
      this.say(done.length === 1 ? "Deleted 1 clip" : `Deleted ${done.length} clips`);
    });
  }

  async rename(id: ClipId, title: string): Promise<void> {
    const t = title.trim() || null;
    await this.attempt(async () => {
      await this.api.setClipTitle(id, t);
      this.pager.patch(id, { title: t });
    });
  }

  // ---- preview ----

  async togglePreview(): Promise<void> {
    if (this.previewOpen) return this.closePreview();
    if (this.selection.focus == null) return;
    this.previewOpen = true;
    await this.loadPreview();
  }

  closePreview(): void {
    this.previewOpen = false;
    this.preview = null;
    this.previewToken++;
  }

  private async loadPreview(): Promise<void> {
    const clip = this.focusClip;
    const token = ++this.previewToken;
    if (!clip) {
      this.preview = null;
      return;
    }
    if (clip.kind === "image") {
      this.preview = { id: clip.id, kind: clip.kind, text: null, loading: false };
      return;
    }
    this.preview = { id: clip.id, kind: clip.kind, text: null, loading: true };
    let text: string | null = null;
    try {
      text = await this.api.getClipText(clip.id);
    } catch (e) {
      this.fail(e);
    }
    if (token !== this.previewToken) return;
    this.preview = { id: clip.id, kind: clip.kind, text: text ?? clip.preview, loading: false };
  }

  // ---- pinboards ----

  async createPinboard(name: string): Promise<Pinboard | null> {
    const n = name.trim();
    if (!n) return null;
    let created: Pinboard | null = null;
    await this.attempt(async () => {
      const used = new Set(this.pinboards.map((p) => p.color));
      const color = PINBOARD_COLORS.find((c) => !used.has(c)) ?? PINBOARD_COLORS[this.pinboards.length % PINBOARD_COLORS.length];
      created = await this.api.createPinboard(n, color);
      const c = created;
      this.pinboards = [...this.pinboards.filter((p) => p.id !== c.id), c];
      await this.setTab(c.id);
    });
    return created;
  }

  async renamePinboard(id: PinboardId, name: string): Promise<void> {
    const n = name.trim();
    if (!n) return;
    await this.attempt(async () => {
      await this.api.renamePinboard(id, n);
      this.pinboards = this.pinboards.map((p) => (p.id === id ? { ...p, name: n } : p));
    });
  }

  async deletePinboard(id: PinboardId): Promise<void> {
    await this.attempt(async () => {
      await this.api.deletePinboard(id);
      this.pinboards = this.pinboards.filter((p) => p.id !== id);
      if (this.pinboardId === id) await this.setTab(null);
    });
  }

  // ---- settings shortcuts ----

  async setPaused(paused: boolean): Promise<void> {
    await this.attempt(async () => {
      await this.api.setCapturePaused(paused);
      if (this.settings) this.settings = { ...this.settings, capturePaused: paused };
    });
  }

  /**
   * Downloads, verifies and installs the update on offer; Satchel then
   * reopens. Progress comes back through `update-status`.
   */
  async installUpdate(): Promise<void> {
    if (!this.update?.actionable) return;
    await this.attempt(() => this.api.installUpdate());
  }

  /** Opens the settings window; Rust hides the shelf first. */
  async openSettings(): Promise<void> {
    this.search.cancel();
    await this.attempt(() => this.api.openSettings());
    this.afterHide();
  }

  // ---- events ----

  private async onClipsChanged(p: ClipsChanged): Promise<void> {
    if (!this.visible) {
      this.stale = true;
      return;
    }
    if (p.reason === "deleted" && p.id != null) {
      if (this.clips.some((c) => c.id === p.id)) this.removeLocally([p.id]);
      return;
    }
    await this.refresh();
  }

  private async onPinboardsChanged(): Promise<void> {
    await this.loadPinboards();
    if (this.pinboardId != null && !this.pinboards.some((p) => p.id === this.pinboardId)) {
      if (this.visible) await this.setTab(null);
      else this.pinboardId = null;
      return;
    }
    if (this.visible) await this.refresh();
    else this.stale = true;
  }

  /** True when changes arrived while hidden (the next show reloads anyway). */
  get isStale(): boolean {
    return this.stale;
  }

  // ---- keyboard ----

  /** Run a keyboard action. Returns false for actions the view handles itself. */
  async run(a: KeyAction): Promise<boolean> {
    switch (a.type) {
      case "move":
        this.move(a.delta, a.extend);
        return true;
      case "edge":
        this.edge(a.end);
        return true;
      case "paste":
        await this.paste(a.alternate);
        return true;
      case "pasteNth":
        await this.pasteNth(a.n, a.alternate);
        return true;
      case "copy":
        await this.copy(a.alternate);
        return true;
      case "pin":
        await this.togglePin();
        return true;
      case "delete":
        await this.deleteSelected();
        return true;
      case "selectAll":
        this.selectAll();
        return true;
      case "preview":
        await this.togglePreview();
        return true;
      case "closePreview":
        this.closePreview();
        return true;
      case "close":
        await this.close();
        return true;
      case "focusSearch":
        this.focusSearchTick++;
        return true;
      case "settings":
        await this.openSettings();
        return true;
      case "typeToSearch":
        return false;
    }
  }
}
