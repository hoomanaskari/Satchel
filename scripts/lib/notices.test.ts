import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import {
  authorName,
  chooseLicenses,
  cutAt,
  leadingComment,
  licenseFileFor,
  parseAccepted,
  parseCargoTree,
  parseSpdx,
  renderNotices,
  rustEntries,
  type AboutOutput,
  type Entry,
} from "./notices";

const accepted = ["MIT", "Apache-2.0", "ISC", "MPL-2.0"];

describe("the licence allow-list", () => {
  it("reads about.toml's accepted list in order, skipping comments", () => {
    expect(parseAccepted('x = 1\naccepted = [\n  "MIT",\n  # "GPL-3.0" no\n  "Apache-2.0", "ISC",\n]\n')).toEqual(["MIT", "Apache-2.0", "ISC"]);
  });

  it("reads the checked-in config, which has no copyleft beyond MPL-2.0", () => {
    const ids = parseAccepted(readFileSync("scripts/notices/about.toml", "utf8"));
    expect(ids[0]).toBe("MIT");
    expect(ids).toContain("MPL-2.0");
    expect(ids.filter((id) => /GPL|EUPL|SSPL|CC-BY-SA|OSL/.test(id))).toEqual([]);
  });

  it("fails without a list", () => {
    expect(() => parseAccepted("targets = []")).toThrow(/accepted/);
  });
});

describe("SPDX expressions", () => {
  it("parses AND, OR, WITH and parentheses", () => {
    expect(parseSpdx("(MIT OR Apache-2.0) AND Unicode-3.0")).toEqual({
      op: "AND",
      args: [{ op: "OR", args: [{ id: "MIT" }, { id: "Apache-2.0" }] }, { id: "Unicode-3.0" }],
    });
    expect(parseSpdx("Apache-2.0 WITH LLVM-exception OR MIT")).toEqual({
      op: "OR",
      args: [{ id: "Apache-2.0 WITH LLVM-exception" }, { id: "MIT" }],
    });
  });

  it("rejects malformed expressions", () => {
    for (const bad of ["", "MIT OR", "(MIT", "MIT Apache-2.0", "AND MIT"]) expect(() => parseSpdx(bad)).toThrow();
  });

  it("chooses the most preferred accepted alternative", () => {
    expect(chooseLicenses("Apache-2.0 OR MIT", accepted)).toEqual(["MIT"]);
    expect(chooseLicenses("GPL-3.0 OR ISC", accepted)).toEqual(["ISC"]);
    expect(chooseLicenses("MIT AND ISC", accepted)).toEqual(["MIT", "ISC"]);
  });

  it("refuses what the allow-list can't satisfy", () => {
    expect(chooseLicenses("GPL-3.0", accepted)).toBeNull();
    expect(chooseLicenses("MIT AND GPL-3.0", accepted)).toBeNull();
    expect(chooseLicenses("Apache-2.0 WITH LLVM-exception", accepted)).toBeNull();
  });
});

describe("Rust crates", () => {
  it("takes registry crates from cargo tree and leaves out Satchel's own", () => {
    const tree = "satchel v0.1.0 (/x/src-tauri)\nserde v1.0.1\nserde v1.0.1 (*)\nring v0.17.14\nsatchel-core v0.1.0 (/x/crates/satchel-core)\n\n";
    expect([...parseCargoTree(tree)]).toEqual(["serde@1.0.1", "ring@0.17.14"]);
  });

  const about: AboutOutput = {
    licenses: [
      {
        id: "MIT",
        text: "MIT text",
        source_path: "/registry/serde/LICENSE-MIT",
        used_by: [
          { crate: { name: "serde", version: "1.0.1", authors: ["Jane Doe <jane@example.com>"] } },
          { crate: { name: "syn", version: "2.0.0" } },
        ],
      },
      { id: "MPL-2.0", text: "MPL text", source_path: null, used_by: [{ crate: { name: "option-ext", version: "0.2.0" } }] },
    ],
  };

  it("keeps only linked crates, without email addresses", () => {
    const entries = rustEntries(about, new Set(["serde@1.0.1", "option-ext@0.2.0"]), accepted);
    expect(entries.map((e) => `${e.item.name} ${e.license} ${e.canonical}`)).toEqual(["serde MIT false", "option-ext MPL-2.0 true"]);
    expect(entries[0].item.authors).toEqual(["Jane Doe"]);
  });

  it("fails when a linked crate has no licence or one that isn't accepted", () => {
    expect(() => rustEntries(about, new Set(["serde@1.0.1", "tokio@1.0.0"]), accepted)).toThrow(/tokio@1.0.0/);
    expect(() => rustEntries(about, new Set(["option-ext@0.2.0"]), ["MIT"])).toThrow(/MPL-2.0/);
  });

  it("strips email addresses from author names", () => {
    expect(authorName("Jane Doe <jane@example.com>")).toBe("Jane Doe");
    expect(authorName("The Tauri Programme")).toBe("The Tauri Programme");
  });
});

describe("npm packages", () => {
  it("picks the licence file for the chosen licence", () => {
    expect(licenseFileFor("MIT", ["LICENSE.md"])).toBe("LICENSE.md");
    expect(licenseFileFor("MIT", ["LICENSE-APACHE-2.0", "LICENSE-MIT"])).toBe("LICENSE-MIT");
    expect(licenseFileFor("Apache-2.0", ["LICENSE-APACHE-2.0", "LICENSE-MIT"])).toBe("LICENSE-APACHE-2.0");
    expect(licenseFileFor("ISC", ["LICENSE-A", "LICENSE-B"])).toBeNull();
  });

  it("reads a licence from a file's leading comment", () => {
    const src = "/****\nCopyright (c) Someone.\n\nPermission granted.\n**** */\ncode();\n";
    expect(leadingComment(src)?.trim()).toBe("Copyright (c) Someone.\n\nPermission granted.");
    expect(leadingComment("code(); /* not leading */")).toBeNull();
  });

  it("cuts a licence file at a marker line", () => {
    expect(cutAt("A\nB\n# Bundled\nC\n", "# Bundled")).toBe("A\nB");
    expect(() => cutAt("A\n", "# Bundled")).toThrow();
  });
});

describe("the notices file", () => {
  const entries: Entry[] = [
    { license: "MIT", text: "Copyright A\r\nMIT terms  \n", canonical: false, item: { kind: "crate", name: "b-crate", version: "1.0.0" } },
    { license: "MIT", text: "Copyright A\nMIT terms\n", canonical: false, item: { kind: "npm", name: "svelte", version: "5.0.0" } },
    { license: "MIT", text: "MIT standard", canonical: true, item: { kind: "crate", name: "a-crate", version: "0.1.0", authors: ["Ann"] } },
    {
      license: "MPL-2.0",
      text: "MPL terms",
      canonical: false,
      item: { kind: "crate", name: "option-ext", version: "0.2.0", repository: "https://github.com/soc/option-ext.git" },
    },
  ];

  it("groups by licence, shares identical texts and is the same in any order", () => {
    const text = renderNotices(entries);
    expect(renderNotices([...entries].reverse())).toBe(text);
    expect(text).toContain("4 packages under 2 licences");
    expect(text).toContain("MIT License, for: b-crate 1.0.0, svelte 5.0.0\n");
    expect(text.match(/MIT terms/g)).toHaveLength(1);
    expect(text).toContain("  a-crate 0.1.0: Ann");
    expect(text.indexOf("MIT License (MIT)")).toBeLessThan(text.indexOf("Mozilla Public License 2.0 (MPL-2.0)"));
  });

  it("points at the source of MPL-2.0 packages", () => {
    expect(renderNotices(entries)).toContain(
      "option-ext 0.2.0: https://crates.io/crates/option-ext/0.2.0 (repository: https://github.com/soc/option-ext.git)",
    );
  });
});
