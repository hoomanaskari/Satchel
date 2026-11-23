import { describe, expect, it } from "vitest";
import {
  addExcluded,
  excludedView,
  filterCandidates,
  identifierError,
  isBuiltInDefault,
  isExcluded,
  moveActive,
  removeExcluded,
} from "./excluded-apps";
import { normalizeExcluded } from "./settings-form";
import type { AppInfo } from "./types";

const app = (id: string, name: string, running = false): AppInfo => ({ id, name, running });
const resolvedMap = (entries: [string, AppInfo | null][]) =>
  new Map(entries.map(([id, a]) => [id.toLowerCase(), a] as const));

describe("isBuiltInDefault", () => {
  it("knows the built-in defaults of every platform, ignoring case", () => {
    expect(isBuiltInDefault("com.agilebits.onepassword7")).toBe(true);
    expect(isBuiltInDefault("COM.APPLE.KEYCHAINACCESS")).toBe(true);
    expect(isBuiltInDefault("KeePass.exe")).toBe(true);
    expect(isBuiltInDefault("bitwarden")).toBe(true);
    expect(isBuiltInDefault("com.example.unknown")).toBe(false);
  });
});

describe("list edits", () => {
  it("adds once, ignoring case and whitespace", () => {
    expect(addExcluded(["a.b"], " c.d ")).toEqual(["a.b", "c.d"]);
    expect(addExcluded(["a.b"], "A.B")).toEqual(["a.b"]);
    expect(addExcluded(["a.b"], "  ")).toEqual(["a.b"]);
    expect(isExcluded(["Com.X"], "com.x")).toBe(true);
  });

  it("removes exactly the stored ids of a row", () => {
    expect(removeExcluded(["a.b", "c.d", "e.f"], ["a.b", "e.f"])).toEqual(["c.d"]);
  });

  it("validates identifiers like the backend", () => {
    expect(identifierError("  ", [])).toMatch(/Enter/);
    expect(identifierError("a\nb", [])).toMatch(/not a valid/);
    expect(identifierError("x".repeat(256), [])).toMatch(/not a valid/);
    expect(identifierError("A.B", ["a.b"])).toMatch(/already excluded/);
    expect(identifierError("com.bitwarden.desktop", ["com.bitwarden.desktop"])).toMatch(/by default/);
    expect(identifierError(" com.example.app ", ["a.b"])).toBeNull();
  });
});

describe("excludedView", () => {
  const list = [
    "com.1password.1password",
    "com.agilebits.onepassword7",
    "com.apple.keychainaccess",
    "KeePass.exe",
    "bitwarden",
    "com.example.secret",
    "seahorse-unknown",
  ];
  const installed = resolvedMap([
    ["com.1password.1password", app("com.1password.1password", "1Password")],
    ["com.apple.keychainaccess", app("com.apple.keychainaccess", "Keychain Access")],
    ["com.agilebits.onepassword7", null],
  ]);

  it("lists installed apps and user-added entries, never defaults that are not installed", () => {
    const rows = excludedView(list, installed);
    expect(rows.map((r) => [r.name, r.ids, r.installed, r.iconId])).toEqual([
      ["1Password", ["com.1password.1password"], true, "com.1password.1password"],
      [null, ["com.example.secret"], false, null],
      ["Keychain Access", ["com.apple.keychainaccess"], true, "com.apple.keychainaccess"],
      [null, ["seahorse-unknown"], false, null],
    ]);
  });

  it("lists a default once it is installed", () => {
    const rows = excludedView(["com.bitwarden.desktop"], resolvedMap([["com.bitwarden.desktop", app("com.bitwarden.desktop", "Bitwarden")]]));
    expect(rows.map((r) => r.name)).toEqual(["Bitwarden"]);
    expect(excludedView(["com.bitwarden.desktop"], new Map())).toEqual([]);
  });

  it("keeps hidden defaults through adding, removing and saving visible apps", () => {
    let draft = [...list];
    draft = addExcluded(draft, "com.apple.TextEdit");
    const rows = excludedView(draft, new Map([...installed, ["com.apple.textedit", app("com.apple.TextEdit", "TextEdit")]]));
    const onePassword = rows.find((r) => r.name === "1Password")!;
    draft = removeExcluded(draft, onePassword.ids);
    draft = addExcluded(draft, "COM.AGILEBITS.ONEPASSWORD7"); // a hidden default typed again: no duplicate
    const saved = normalizeExcluded(draft);
    expect(saved).toEqual([
      "com.agilebits.onepassword7",
      "com.apple.keychainaccess",
      "KeePass.exe",
      "bitwarden",
      "com.example.secret",
      "seahorse-unknown",
      "com.apple.TextEdit",
    ]);
  });

  it("merges installed copies that share a name into one row that removes all of them", () => {
    const rows = excludedView(
      ["com.agilebits.onepassword7", "com.agilebits.onepassword-osx"],
      resolvedMap([
        ["com.agilebits.onepassword7", app("com.agilebits.onepassword7", "1Password")],
        ["com.agilebits.onepassword-osx", app("com.agilebits.onepassword-osx", "1password")],
      ]),
    );
    expect(rows).toHaveLength(1);
    expect(rows[0].ids).toEqual(["com.agilebits.onepassword7", "com.agilebits.onepassword-osx"]);
    expect(rows[0].detail).toBe("com.agilebits.onepassword7, com.agilebits.onepassword-osx");
  });

  it("gives user-added ids that differ only in case distinct row keys", () => {
    const rows = excludedView(["com.example.Vault", "com.example.vault"], new Map());
    expect(new Set(rows.map((r) => r.key)).size).toBe(2);
  });

  it("matches resolutions case-insensitively", () => {
    const rows = excludedView(["COM.APPLE.NOTES"], resolvedMap([["com.apple.notes", app("com.apple.Notes", "Notes")]]));
    expect(rows[0]).toMatchObject({ name: "Notes", ids: ["COM.APPLE.NOTES"], iconId: "com.apple.Notes" });
  });
});

describe("filterCandidates", () => {
  const apps = [
    app("com.apple.calculator", "Calculator"),
    app("com.apple.Notes", "Notes", true),
    app("com.microsoft.onenote.mac", "OneNote"),
    app("com.1password.1password", "1Password"),
  ];

  it("matches every word against name and id, name prefixes first", () => {
    expect(filterCandidates(apps, "note", []).map((a) => a.name)).toEqual(["Notes", "OneNote"]);
    expect(filterCandidates(apps, "apple calc", []).map((a) => a.name)).toEqual(["Calculator"]);
    expect(filterCandidates(apps, "", []).map((a) => a.name)).toEqual(["Calculator", "Notes", "OneNote", "1Password"]);
  });

  it("marks apps that are already excluded", () => {
    const out = filterCandidates(apps, "", ["COM.1PASSWORD.1PASSWORD"]);
    expect(out.filter((a) => a.excluded).map((a) => a.name)).toEqual(["1Password"]);
  });
});

describe("moveActive", () => {
  const items = [{ excluded: true }, { excluded: false }, { excluded: true }, { excluded: false }, { excluded: true }];

  it("skips excluded apps and stops at the ends", () => {
    expect(moveActive(items, -1, 1)).toBe(1);
    expect(moveActive(items, 1, 1)).toBe(3);
    expect(moveActive(items, 3, 1)).toBe(3);
    expect(moveActive(items, 3, -1)).toBe(1);
    expect(moveActive(items, 1, -1)).toBe(1);
    expect(moveActive(items, 1, Infinity)).toBe(3);
    expect(moveActive(items, 3, -Infinity)).toBe(1);
  });

  it("returns -1 when nothing can be picked", () => {
    expect(moveActive([{ excluded: true }], -1, 1)).toBe(-1);
    expect(moveActive([], 0, 1)).toBe(-1);
  });
});
