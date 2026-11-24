// Shelf keyboard shortcuts as a pure mapping from a key event plus context
// to an action. The search field keeps focus while the shelf is open, so
// shortcuts that collide with text editing defer to the field when it has
// text to edit.

export interface KeyInput {
  key: string;
  code?: string;
  metaKey: boolean;
  ctrlKey: boolean;
  shiftKey: boolean;
  altKey: boolean;
  isComposing?: boolean;
}

export interface KeyContext {
  mac: boolean;
  /** `search`: the search field; `text`: any other editable field; `other`: anything else. */
  focus: "search" | "text" | "other";
  searchEmpty: boolean;
  /** The focused field has selected text. */
  hasTextSelection: boolean;
  previewOpen: boolean;
  /** Focus is on a control other than a card (tab, scope, footer button): it handles Return and Space. */
  onControl?: boolean;
}

export type KeyAction =
  | { type: "move"; delta: -1 | 1; extend: boolean }
  | { type: "edge"; end: boolean }
  // `alternate`: Shift was held, asking for the other paste format (plain
  // text, or formatted text when plain text is the default).
  | { type: "paste"; alternate: boolean }
  | { type: "pasteNth"; n: number; alternate: boolean }
  | { type: "copy"; alternate: boolean }
  | { type: "pin" }
  | { type: "delete" }
  | { type: "selectAll" }
  | { type: "preview" }
  | { type: "closePreview" }
  | { type: "close" }
  | { type: "focusSearch" }
  | { type: "settings" }
  | { type: "typeToSearch" };

function digit(e: KeyInput): number | null {
  const m = /^Digit([0-9])$/.exec(e.code ?? "") ?? /^([0-9])$/.exec(e.key);
  return m ? Number(m[1]) : null;
}

function letter(e: KeyInput): string {
  const m = /^Key([A-Z])$/.exec(e.code ?? "");
  return m ? m[1].toLowerCase() : e.key.toLowerCase();
}

export function mapKey(e: KeyInput, ctx: KeyContext): KeyAction | null {
  if (e.isComposing || ctx.focus === "text") return null;
  const mod = ctx.mac ? e.metaKey : e.ctrlKey;
  // The other platform's primary modifier (Ctrl on macOS, Win elsewhere) is never ours.
  const foreign = ctx.mac ? e.ctrlKey : e.metaKey;
  if (e.altKey || foreign) return null;
  const inSearch = ctx.focus === "search";
  const editingText = inSearch && !ctx.searchEmpty;

  if (mod) {
    const n = digit(e);
    if (n !== null) return n >= 1 ? { type: "pasteNth", n, alternate: e.shiftKey } : null;
    if (e.key === "Backspace" || e.key === "Delete") return editingText ? null : { type: "delete" };
    if (e.key === "," || e.code === "Comma") return e.shiftKey ? null : { type: "settings" };
    switch (letter(e)) {
      case "c":
        return ctx.hasTextSelection ? null : { type: "copy", alternate: e.shiftKey };
      case "p":
        return e.shiftKey ? null : { type: "pin" };
      case "a":
        return editingText || e.shiftKey ? null : { type: "selectAll" };
      case "f":
        return e.shiftKey ? null : { type: "focusSearch" };
    }
    return null;
  }

  if (ctx.onControl && !inSearch && (e.key === "Enter" || e.key === " ")) return null;

  switch (e.key) {
    case "Escape":
      return ctx.previewOpen ? { type: "closePreview" } : { type: "close" };
    case "Enter":
      return { type: "paste", alternate: e.shiftKey };
    case "ArrowLeft":
    case "ArrowRight":
      if (e.shiftKey && editingText) return null;
      return { type: "move", delta: e.key === "ArrowLeft" ? -1 : 1, extend: e.shiftKey };
    case "Home":
    case "End":
      return inSearch ? null : { type: "edge", end: e.key === "End" };
    case " ":
      return ctx.previewOpen || !editingText ? { type: "preview" } : null;
  }

  if (!inSearch && e.key.length === 1) return { type: "typeToSearch" };
  return null;
}

/**
 * Settings window: Escape or Cmd+W (Ctrl+W elsewhere) closes it. Other
 * modifiers, IME composition and Escape on an open control are left alone.
 */
export function closesSettingsWindow(e: KeyInput, mac: boolean): boolean {
  if (e.isComposing) return false;
  const mod = mac ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey;
  if (e.key === "Escape") return !e.metaKey && !e.ctrlKey && !e.altKey && !e.shiftKey;
  const w = e.key.toLowerCase() === "w" || e.code === "KeyW";
  return w && mod && !e.altKey && !e.shiftKey;
}
