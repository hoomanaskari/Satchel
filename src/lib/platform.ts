// Platform facts the UI needs. Read once; never changes at runtime.

export const isMac: boolean =
  typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);

/** Label for the primary modifier in shortcut hints. */
export const modLabel = isMac ? "⌘" : "Ctrl+";

export function prefersReducedMotion(): boolean {
  return typeof matchMedia !== "undefined" && matchMedia("(prefers-reduced-motion: reduce)").matches;
}
