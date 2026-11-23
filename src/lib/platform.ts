// Platform facts the UI needs. Read once; never changes at runtime.

export const isMac: boolean =
  typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);

/** Label for the primary modifier in shortcut hints. */
export const modLabel = isMac ? "⌘" : "Ctrl+";

export function prefersReducedMotion(): boolean {
  return typeof matchMedia !== "undefined" && matchMedia("(prefers-reduced-motion: reduce)").matches;
}

/**
 * The native material behind the shelf panel, set by Rust in an
 * initialization script before the page loads: Liquid Glass (macOS 26+),
 * the older frosted blur (macOS 11–15) or none. Anything else, including a
 * plain browser in mock mode, counts as "none" and keeps the default look.
 */
export type ShelfBackground = "glass" | "blur" | "none";

export function shelfBackground(): ShelfBackground {
  const value: unknown = (globalThis as { __SATCHEL_SHELF_BACKGROUND__?: unknown }).__SATCHEL_SHELF_BACKGROUND__;
  return value === "glass" || value === "blur" ? value : "none";
}
