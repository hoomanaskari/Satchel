import type { LoginItem } from "./types";

const LOGIN_ITEMS = "System Settings › General › Login Items";

/** The line under "Open … at login" in Settings, for where it stands. */
export function loginItemHint(item: LoginItem, appName: string): string {
  switch (item.unavailable) {
    case "dev":
      return `Only the installed Satchel opens at login; ${appName} never does.`;
    case "macos":
      return `Needs macOS 13 or later. You can add ${appName} yourself in System Settings › Users & Groups › Login Items.`;
    case "location":
      return `Move ${appName} to your Applications folder to open it at login.`;
  }
  if (item.requiresApproval) return `Waiting for you to allow ${appName} in ${LOGIN_ITEMS}.`;
  return `Starts quietly in the menu bar when you log in. Also listed in ${LOGIN_ITEMS}.`;
}
