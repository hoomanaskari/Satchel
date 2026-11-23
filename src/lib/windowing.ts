// Geometry for the horizontally windowed card list: fixed-width items at a
// fixed stride, so everything is arithmetic and nothing reads layout.

export interface Layout {
  /** Card width. */
  item: number;
  gap: number;
  /** Padding before the first and after the last card. */
  pad: number;
}

export interface Window {
  start: number;
  /** Exclusive. */
  end: number;
}

/** Viewport assumed before the first measurement. */
const FALLBACK_VIEWPORT = 1600;

export const offsetOf = (index: number, l: Layout): number => l.pad + index * (l.item + l.gap);

export function contentWidth(count: number, l: Layout): number {
  return 2 * l.pad + count * l.item + Math.max(0, count - 1) * l.gap;
}

export function computeWindow(scroll: number, viewport: number, count: number, l: Layout, overscan = 2): Window {
  if (count <= 0) return { start: 0, end: 0 };
  const vw = viewport > 0 ? viewport : FALLBACK_VIEWPORT;
  const stride = l.item + l.gap;
  // First item whose right edge is past the left edge of the viewport.
  const first = Math.max(0, Math.floor((scroll - l.pad - l.item) / stride) + 1);
  // Last item whose left edge is before the right edge.
  const last = Math.floor((scroll + vw - l.pad - 1) / stride);
  return {
    start: Math.max(0, Math.min(count, first) - overscan),
    end: Math.min(count, Math.max(0, last + 1) + overscan),
  };
}

/** First item fully inside the viewport (base for Cmd+1…9). */
export function firstVisible(scroll: number, _viewport: number, count: number, l: Layout): number {
  if (count <= 0) return 0;
  const i = Math.ceil((scroll - l.pad) / (l.item + l.gap));
  return Math.min(count - 1, Math.max(0, i));
}

/** Scroll position that brings `index` fully into view, moving as little as possible. */
export function revealScroll(index: number, scroll: number, viewport: number, l: Layout): number {
  const left = offsetOf(index, l);
  const right = left + l.item;
  if (left - l.pad < scroll) return Math.max(0, left - l.pad);
  if (right + l.pad > scroll + viewport) return right + l.pad - viewport;
  return scroll;
}
