import { join } from "node:path";

/**
 * The two Satchel apps. The installed one is made only by `pnpm release`, from tauri.conf.json
 * as it is. Satchel Dev is what `pnpm app` and `pnpm tauri dev` build, with tauri.dev.conf.json
 * laid over it: its own name, identifier, data, Accessibility grant, shortcut and icon.
 */
export const production = { identifier: "app.satchel.desktop", productName: "Satchel" } as const;
export const development = { identifier: "app.satchel.desktop.dev", productName: "Satchel Dev" } as const;

/** The overlay that makes Satchel Dev, in the checkout at `root`. */
export const devOverlay = (root: string) => join(root, "src-tauri", "tauri.dev.conf.json");

/**
 * The arguments for Tauri's CLI when `pnpm tauri <args>` is run in the checkout at `root`:
 * `tauri dev` always runs Satchel Dev. Everything else goes through as it is, `tauri build` too:
 * `pnpm app` names the overlay itself, and `pnpm release` must build the installed Satchel.
 */
export function tauriArgs(args: string[], root: string): string[] {
  return args[0] === "dev" ? ["dev", "--config", devOverlay(root), ...args.slice(1)] : args;
}

interface TauriConfig {
  identifier?: unknown;
  productName?: unknown;
  bundle?: { icon?: unknown };
}

/** Stops a release whose config isn't the installed Satchel's: the wrong one would replace it. */
export function assertProduction(config: TauriConfig): void {
  const icons = Array.isArray(config.bundle?.icon) ? (config.bundle.icon as unknown[]) : [];
  const problems = [
    config.identifier !== production.identifier && `identifier is ${String(config.identifier)}`,
    config.productName !== production.productName && `product name is ${String(config.productName)}`,
    icons.some((icon) => String(icon).startsWith("icons/dev/")) && "the icon is Satchel Dev's",
  ].filter(Boolean);
  if (problems.length) throw new Error(`This is not the installed Satchel's config: ${problems.join(", ")}`);
}
