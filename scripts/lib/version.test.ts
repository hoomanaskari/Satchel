import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { currentVersion, isVersion, setVersion, versionFiles, type Io } from "./version";

const crate = (name: string) =>
  `[package]\nname = "${name}"\nversion = "0.1.0"\nedition = "2024"\n\n[dependencies]\nserde = { version = "1" }\n`;
const files: Record<string, string> = {
  "package.json": '{\n  "name": "satchel",\n  "version": "0.1.0",\n  "dependencies": { "x": { "version": "9.9.9" } }\n}\n',
  "src-tauri/tauri.conf.json": '{\n  "productName": "Satchel",\n  "version": "0.1.0",\n  "app": {}\n}\n',
  "src-tauri/Cargo.toml": crate("satchel"),
  "crates/satchel-core/Cargo.toml": crate("satchel-core"),
};

/** A pretend repo: what was written, run and said. */
function repo(start = files) {
  const disk = { ...start };
  const ran: string[][] = [];
  const said: string[] = [];
  const io: Io = {
    read: (path) => disk[path],
    write: (path, text) => void (disk[path] = text),
    run: (cmd, args) => void ran.push([cmd, ...args]),
    log: (line) => void said.push(line),
  };
  return { disk, ran, said, io };
}

describe("isVersion", () => {
  it.each(["0.2.0", "1.10.3", "1.0.0-beta.1"])("takes %s", (v) => expect(isVersion(v)).toBe(true));
  it.each(["", "v0.2.0", "0.2", "0.2.0.1", "one"])("refuses %j", (v) => expect(isVersion(v)).toBe(false));
});

describe("setVersion", () => {
  it("sets Satchel's own version in every file, and only that one", () => {
    const { disk, ran, said, io } = repo();
    expect(setVersion(["0.2.0"], io)).toBe(0);
    expect(disk["package.json"]).toContain('  "version": "0.2.0",');
    expect(disk["package.json"]).toContain('"version": "9.9.9"');
    expect(disk["src-tauri/tauri.conf.json"]).toContain('  "version": "0.2.0",');
    for (const path of ["src-tauri/Cargo.toml", "crates/satchel-core/Cargo.toml"]) {
      expect(disk[path]).toContain('\nversion = "0.2.0"\n');
      expect(disk[path]).toContain('serde = { version = "1" }');
    }
    expect(ran).toEqual([["cargo", "update", "--workspace", "--offline"]]);
    expect(said).toEqual([
      "Satchel is now 0.2.0 (was 0.1.0) in package.json, src-tauri/tauri.conf.json, src-tauri/Cargo.toml, crates/satchel-core/Cargo.toml and Cargo.lock.",
    ]);
  });

  it("says how to use it when the version is missing or wrong, and changes nothing", () => {
    for (const args of [[], ["v0.2"]]) {
      const { disk, ran, said, io } = repo();
      expect(setVersion(args, io)).toBe(1);
      expect(disk).toEqual(files);
      expect(ran).toEqual([]);
      expect(said).toEqual(["Give the new version, like: pnpm set-version 0.2.0"]);
    }
  });

  it("stops before writing anything when a file has no version to change", () => {
    const { disk, ran, said, io } = repo({ ...files, "src-tauri/Cargo.toml": "[workspace]\n" });
    expect(setVersion(["0.2.0"], io)).toBe(1);
    expect(disk).toEqual({ ...files, "src-tauri/Cargo.toml": "[workspace]\n" });
    expect(ran).toEqual([]);
    expect(said).toEqual(["Couldn’t find the version in src-tauri/Cargo.toml."]);
  });

  it("finds the version in this repository's real files, all the same", () => {
    const real = { read: (path: string) => readFileSync(path, "utf8") } as Io;
    const versions = versionFiles.map(({ path, at }) => at.exec(real.read(path))?.[2]);
    expect(versions.every((v) => v === currentVersion(real))).toBe(true);
  });
});
