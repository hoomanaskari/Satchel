import { describe, expect, it } from "vitest";
import { execFileSync } from "node:child_process";
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { assertProduction, devOverlay, development, production, tauriArgs } from "./identity";

const read = (path: string) => JSON.parse(readFileSync(path, "utf8"));
const shipped = read("src-tauri/tauri.conf.json");
const overlay = read("src-tauri/tauri.dev.conf.json");

describe("the two Satchel apps", () => {
  it("ships the installed Satchel's identity from tauri.conf.json", () => {
    expect(shipped.identifier).toBe(production.identifier);
    expect(shipped.productName).toBe(production.productName);
    expect(() => assertProduction(shipped)).not.toThrow();
  });

  it("makes Satchel Dev with an overlay that changes everything keyed to the app", () => {
    expect(overlay.identifier).toBe(development.identifier);
    expect(overlay.productName).toBe(development.productName);
    expect(development.identifier).not.toBe(production.identifier);
    expect(overlay.bundle.icon.every((icon: string) => icon.startsWith("icons/dev/"))).toBe(true);
    // The Icon Composer file is what macOS 26 and later show; without it the icon is boxed in a tile.
    expect(overlay.bundle.icon).toContain("icons/dev/Satchel.icon");
    // The front end is built in the mode that names Satchel Dev (src/lib/app-name.ts).
    expect(overlay.build.beforeBuildCommand).toContain("--mode dev-app");
    expect(overlay.build.beforeDevCommand).toContain("--mode dev-app");
  });

  it("refuses to release Satchel Dev", () => {
    // Tauri replaces arrays and merges objects, as here.
    const merged = { ...shipped, ...overlay, bundle: { ...shipped.bundle, ...overlay.bundle } };
    expect(() => assertProduction(merged)).toThrow(
      /identifier is app\.satchel\.desktop\.dev, product name is Satchel Dev, the icon is Satchel Dev's/,
    );
    expect(() => assertProduction({})).toThrow("identifier is undefined");
  });

  it("releases only with GitHub's update feed, a key, and signed versions required", () => {
    const updater = shipped.plugins.updater;
    const withUpdater = (u: Record<string, unknown>) => ({ ...shipped, plugins: { updater: u } });
    expect(() => assertProduction(withUpdater({ ...updater, endpoints: ["http://127.0.0.1:8000/latest.json"] }))).toThrow(
      "its update feed is not GitHub's",
    );
    expect(() => assertProduction(withUpdater({ ...updater, pubkey: "" }))).toThrow("it has no update key");
    expect(() => assertProduction(withUpdater({ ...updater, requireSignedVersion: false }))).toThrow("signed versions");
    expect(() => assertProduction(withUpdater({ ...updater, dangerousInsecureTransportProtocol: true }))).toThrow(
      "it sets dangerousInsecureTransportProtocol",
    );
    expect(() => assertProduction({ ...shipped, plugins: {} })).toThrow("it has no updater");
    // Satchel Dev empties the feed.
    expect(overlay.plugins.updater.endpoints).toEqual([]);
  });

  it("runs `tauri dev` as Satchel Dev and leaves every other command alone", () => {
    expect(tauriArgs(["dev"], "/repo")).toEqual(["dev", "--config", join("/repo", "src-tauri", "tauri.dev.conf.json")]);
    expect(tauriArgs(["dev", "--release"], "/repo")).toEqual(["dev", "--config", devOverlay("/repo"), "--release"]);
    // `pnpm release` builds the installed Satchel through here.
    expect(tauriArgs(["build", "--bundles", "app"], "/repo")).toEqual(["build", "--bundles", "app"]);
    expect(tauriArgs([], "/repo")).toEqual([]);
  });

  it("builds Satchel Dev, never the installed Satchel, with pnpm app", () => {
    const root = mkdtempSync(join(tmpdir(), "satchel-app-test-"));
    try {
      mkdirSync(join(root, "bin"));
      const trace = join(root, "trace.jsonl");
      const fake = join(root, "bin", "fake");
      writeFileSync(
        fake,
        `#!${process.execPath}\nrequire("fs").appendFileSync(process.env.TRACE, JSON.stringify({ tool: require("path").basename(process.argv[1]), args: process.argv.slice(2) }) + "\\n");\n`,
      );
      chmodSync(fake, 0o755);
      for (const tool of ["pnpm", "security"]) symlinkSync(fake, join(root, "bin", tool));
      execFileSync("sh", ["scripts/app.sh", "--verbose"], {
        env: { ...process.env, PATH: `${join(root, "bin")}:${process.env.PATH}`, TRACE: trace, APPLE_SIGNING_IDENTITY: "synthetic" },
        stdio: "ignore",
      });
      const calls = readFileSync(trace, "utf8").trim().split("\n").map((line) => JSON.parse(line));
      expect(calls).toEqual([
        { tool: "pnpm", args: ["tauri", "build", "--bundles", "app", "--config", devOverlay(process.cwd()), "--verbose"] },
      ]);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });
});
