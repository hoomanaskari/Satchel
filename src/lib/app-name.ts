/**
 * The app's name as macOS shows it (Finder, System Settings > Accessibility): "Satchel", or
 * "Satchel Dev" for the development build. It matches `productName` in the Tauri config:
 * tauri.dev.conf.json builds the front end with `--mode dev-app`, and `vite dev` is only ever
 * run for Satchel Dev (`pnpm tauri dev`). Tests and release builds say "Satchel".
 */
export const APP_NAME = appName(import.meta.env.MODE);

export function appName(mode: string): string {
  return mode === "dev-app" || mode === "development" ? "Satchel Dev" : "Satchel";
}
