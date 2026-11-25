// Immutable selection model: single, toggle (Cmd/Ctrl-click), range
// (Shift-click / Shift+arrow) and keyboard movement over an ordered list.

import type { ClipId } from "./types";

export interface Selection {
  /** Selected ids, in the order they were added. */
  ids: ClipId[];
  /** Fixed end of a range. */
  anchor: ClipId | null;
  /** Moving end; the clip the preview and status describe. */
  focus: ClipId | null;
}

export const empty = (): Selection => ({ ids: [], anchor: null, focus: null });

export const single = (id: ClipId): Selection => ({ ids: [id], anchor: id, focus: id });

export function isSelected(s: Selection, id: ClipId): boolean {
  return s.ids.includes(id);
}

export function toggle(s: Selection, id: ClipId): Selection {
  if (!s.ids.includes(id)) return { ids: [...s.ids, id], anchor: id, focus: id };
  const ids = s.ids.filter((x) => x !== id);
  const last = ids.length ? ids[ids.length - 1] : null;
  return { ids, anchor: s.anchor === id ? last : s.anchor, focus: s.focus === id ? last : s.focus };
}

function span(order: ClipId[], from: ClipId, to: ClipId): ClipId[] {
  const a = order.indexOf(from);
  const b = order.indexOf(to);
  if (a < 0 || b < 0) return [to];
  const step = a <= b ? 1 : -1;
  const out: ClipId[] = [];
  for (let i = a; i !== b + step; i += step) out.push(order[i]);
  return out;
}

export function range(s: Selection, id: ClipId, order: ClipId[]): Selection {
  if (s.anchor == null || !order.includes(s.anchor)) return single(id);
  return { ids: span(order, s.anchor, id), anchor: s.anchor, focus: id };
}

export function move(s: Selection, delta: number, order: ClipId[], extend = false): Selection {
  if (order.length === 0) return empty();
  const at = s.focus == null ? -1 : order.indexOf(s.focus);
  const next = at < 0 ? 0 : Math.min(order.length - 1, Math.max(0, at + delta));
  const id = order[next];
  return extend ? range(s, id, order) : single(id);
}

export function all(order: ClipId[], focus: ClipId | null): Selection {
  if (order.length === 0) return empty();
  return { ids: [...order], anchor: order[0], focus: focus != null && order.includes(focus) ? focus : order[0] };
}

/** Selected ids in list order (the order multi-paste uses). */
export function ordered(s: Selection, order: ClipId[]): ClipId[] {
  const set = new Set(s.ids);
  return order.filter((id) => set.has(id));
}

/**
 * The clip that takes the place of `prev[at]` once only `present` remain:
 * the next survivor after it, else the nearest one before it.
 */
function neighbour(prev: ClipId[], at: number, present: Set<ClipId>): ClipId | null {
  for (let i = at; i < prev.length; i++) if (present.has(prev[i])) return prev[i];
  for (let i = at - 1; i >= 0; i--) if (present.has(prev[i])) return prev[i];
  return null;
}

/**
 * Drop ids that left the list. When nothing selected remains, select the
 * clip that took the first selected clip's place in `prev` (the order before
 * the change), so the selection, and the view that follows it, stays where
 * it was; without `prev`, or with none of it left, the first clip.
 */
export function prune(s: Selection, order: ClipId[], prev: ClipId[] = []): Selection {
  const present = new Set(order);
  if (s.ids.every((id) => present.has(id)) && (s.ids.length > 0 || order.length === 0)) return s;
  const ids = s.ids.filter((id) => present.has(id));
  if (ids.length === 0) {
    if (order.length === 0) return empty();
    const was = new Set(s.ids);
    const at = prev.findIndex((id) => was.has(id));
    const next = at < 0 ? null : neighbour(prev, at, present);
    return single(next ?? order[0]);
  }
  const last = ids[ids.length - 1];
  return {
    ids,
    anchor: s.anchor != null && present.has(s.anchor) ? s.anchor : ids[0],
    focus: s.focus != null && present.has(s.focus) ? s.focus : last,
  };
}

/**
 * Selection after `removed` leave `order` (the order before removal): what
 * is still selected stays selected; if the whole selection went, the clip
 * now at the first removed selected clip's index, or the new last one.
 * Ids already gone from `order` change nothing.
 */
export function afterRemoval(s: Selection, removed: ClipId[], order: ClipId[]): Selection {
  const gone = new Set(removed);
  if (!order.some((id) => gone.has(id))) return s;
  return prune(s, order.filter((id) => !gone.has(id)), order);
}
