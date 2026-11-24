// What the shelf and the settings window say about updates. Rust does the
// checking, downloading, verifying and installing (see `src-tauri/src/updates.rs`);
// these are the words and states, as pure functions.

import { APP_NAME } from "./app-name";
import { relativeTime } from "./format";
import type { UpdateFailure, UpdatePhase, UpdateStatus } from "./types";

/** How far a download is, as a whole percent; null until the size is known. */
export function percent(phase: UpdatePhase): number | null {
  if (phase.state !== "downloading" || !phase.total) return null;
  return Math.min(100, Math.floor((phase.downloaded / phase.total) * 100));
}

/** Why an update did not happen, without any detail that could identify anything. */
export function failureText(during: UpdateFailure, version: string, name = APP_NAME): string {
  switch (during) {
    case "download":
      return `Couldn’t download ${name} ${version}. Check your connection and try again.`;
    case "verify":
      return `${name} ${version} couldn’t be verified, so it was not installed.`;
    case "install":
      return `Couldn’t install ${name} ${version}. Try again, or download it from GitHub.`;
  }
}

/**
 * The shelf's update control: a persistent state in the top bar, like
 * "Paused", shown while an update is on offer, never a transient notice.
 * Null when there is nothing to show.
 */
export interface ShelfUpdate {
  /** Visible text, short: the bar has little room. */
  label: string;
  /** What a screen reader hears; for buttons, what clicking does. */
  aria: string;
  /** Tooltip. */
  title: string;
  /** Clicking starts (or retries) the update. */
  actionable: boolean;
  /** Visual tone. */
  tone: "offer" | "busy" | "failed";
  /** 0–100 while downloading with a known size. */
  progress: number | null;
}

export function shelfUpdate(s: UpdateStatus | null, name = APP_NAME): ShelfUpdate | null {
  if (!s?.supported) return null;
  const p = s.phase;
  switch (p.state) {
    case "idle":
      return null;
    case "available":
      return {
        label: `Update to ${p.version}`,
        aria: `Update to ${name} ${p.version}`,
        title: `${name} ${p.version} is available. Click to download and install it; ${name} reopens when it is done.`,
        actionable: true,
        tone: "offer",
        progress: null,
      };
    case "downloading": {
      const pct = percent(p);
      return {
        label: pct === null ? "Updating…" : `Updating… ${pct}%`,
        aria: `Downloading ${name} ${p.version}` + (pct === null ? "" : `, ${pct} percent`),
        title: `Downloading ${name} ${p.version}`,
        actionable: false,
        tone: "busy",
        progress: pct,
      };
    }
    case "installing":
      return {
        label: "Installing…",
        aria: `Installing ${name} ${p.version}`,
        title: `Installing ${name} ${p.version}; ${name} reopens in a moment.`,
        actionable: false,
        tone: "busy",
        progress: null,
      };
    case "failed":
      return {
        label: "Update Failed",
        aria: `Couldn’t update to ${name} ${p.version}. Try again`,
        title: `${failureText(p.during, p.version, name)} Click to try again.`,
        actionable: true,
        tone: "failed",
        progress: null,
      };
  }
}

/** The settings window's line about where updates stand. */
export function updateLine(s: UpdateStatus, now: number, name = APP_NAME): string {
  if (!s.supported) return `${name} doesn’t update itself. The installed Satchel does.`;
  const p = s.phase;
  switch (p.state) {
    case "available":
      return `${name} ${p.version} is available.`;
    case "downloading": {
      const pct = percent(p);
      return `Downloading ${name} ${p.version}…` + (pct === null ? "" : ` ${pct}%`);
    }
    case "installing":
      return `Installing ${name} ${p.version}. ${name} reopens in a moment.`;
    case "failed":
      return failureText(p.during, p.version, name);
  }
  if (s.checking) return "Checking for updates…";
  const last = s.lastCheck;
  if (!last) return "Not checked yet.";
  const when = relativeTime(last.at, now);
  const ago =
    when === "Now" ? "just now" : when === "Yesterday" || when.endsWith(" ago") ? when.toLowerCase() : `on ${when}`;
  if (last.outcome === "failed") return `Couldn’t check for updates (${ago}). You may be offline.`;
  return `${name} is up to date (checked ${ago}).`;
}
