/**
 * THIRD-PARTY-NOTICES.txt: the licences of the third-party code inside Satchel.app, written by
 * `pnpm notices` (scripts/notices.ts). This module is the pure part: reading the allow-list,
 * choosing licences from SPDX expressions, narrowing cargo-about's output to the crates that are
 * linked, and rendering the file. The script runs the tools and reads the files.
 */

/** A package (or code copied into one) that ships in Satchel. */
export interface Item {
  kind: "crate" | "npm";
  name: string;
  /** Absent for code copied into another package. */
  version?: string;
  repository?: string;
  /** Names only (no email addresses), for licence texts that carry no copyright line. */
  authors?: string[];
  note?: string;
}

/** One licence text and what it covers. */
export interface Entry {
  /** SPDX identifier. */
  license: string;
  text: string;
  /** True when the package has no licence file and the text is the standard one, with no copyright line. */
  canonical: boolean;
  item: Item;
}

// ---- allow-list ----

/** The `accepted` array of an about.toml (cargo-about's config), in order. */
export function parseAccepted(toml: string): string[] {
  const body = /^accepted\s*=\s*\[([\s\S]*?)\]/m.exec(toml)?.[1];
  if (body === undefined) throw new Error("about.toml has no `accepted` list.");
  const ids = body
    .split("\n")
    .map((line) => line.replace(/#.*$/, ""))
    .flatMap((line) => [...line.matchAll(/"([^"]+)"/g)].map((m) => m[1]));
  if (ids.length === 0) throw new Error("about.toml's `accepted` list is empty.");
  return ids;
}

// ---- SPDX expressions ----

type Expr = { id: string } | { op: "AND" | "OR"; args: Expr[] };

/** Parses an SPDX licence expression: identifiers, `WITH` exceptions, AND, OR and parentheses. */
export function parseSpdx(expression: string): Expr {
  const tokens = expression.replace(/[()]/g, " $& ").trim().split(/\s+/).filter(Boolean);
  let at = 0;
  const fail = (): never => {
    throw new Error(`Can't read the licence expression "${expression}".`);
  };
  const atom = (): Expr => {
    const t = tokens[at++];
    if (t === undefined || t === ")" || t === "AND" || t === "OR" || t === "WITH") return fail();
    if (t === "(") {
      const e = or();
      if (tokens[at++] !== ")") fail();
      return e;
    }
    if (tokens[at] === "WITH") {
      at++;
      const exception = tokens[at++];
      if (!exception || /^[()]$|^(AND|OR|WITH)$/.test(exception)) fail();
      return { id: `${t} WITH ${exception}` };
    }
    return { id: t };
  };
  const chain = (op: "AND" | "OR", next: () => Expr) => (): Expr => {
    const args = [next()];
    while (tokens[at] === op) {
      at++;
      args.push(next());
    }
    return args.length === 1 ? args[0] : { op, args };
  };
  const and = chain("AND", atom);
  const or = chain("OR", and);
  const e = or();
  if (at !== tokens.length) fail();
  return e;
}

/**
 * The licences to comply with for `expression`, taken from `accepted`, or null when it cannot be
 * satisfied. For OR, the alternative whose least preferred licence comes earliest in `accepted`.
 */
export function chooseLicenses(expression: string, accepted: string[]): string[] | null {
  const rank = (ids: string[]) => Math.max(...ids.map((id) => accepted.indexOf(id)));
  const pick = (e: Expr): string[] | null => {
    if ("id" in e) return accepted.includes(e.id) ? [e.id] : null;
    const parts = e.args.map(pick);
    if (e.op === "AND") return parts.every(Boolean) ? [...new Set(parts.flat() as string[])] : null;
    const ok = parts.filter((p): p is string[] => p !== null);
    return ok.length === 0 ? null : ok.reduce((best, p) => (rank(p) < rank(best) ? p : best));
  };
  return pick(parseSpdx(expression));
}

// ---- Rust ----

/** `name@version` of every registry crate in `cargo tree --prefix none -f '{p}'` output (path crates are Satchel's own). */
export function parseCargoTree(output: string): Set<string> {
  const crates = new Set<string>();
  for (const line of output.split("\n")) {
    const m = /^(\S+) v(\S+)(.*)$/.exec(line.trim());
    if (!m || m[3].includes("(/")) continue;
    crates.add(`${m[1]}@${m[2]}`);
  }
  return crates;
}

interface AboutCrate {
  name: string;
  version: string;
  source?: string | null;
  repository?: string | null;
  authors?: string[];
}
interface AboutLicense {
  id: string;
  text: string;
  source_path?: string | null;
  used_by: { crate: AboutCrate }[];
}
/** The parts of `cargo about generate --format json` that are used. */
export interface AboutOutput {
  licenses: AboutLicense[];
}

/** `Jane Doe <jane@example.com>` → `Jane Doe`. */
export const authorName = (author: string) => author.replace(/\s*<[^>]*>/g, "").replace(/\s*\([^)]*@[^)]*\)/g, "").trim();

/**
 * cargo-about's licence texts for the crates in `linked`. Fails when a linked crate has none
 * (cargo-about and cargo disagree about the dependency graph) or when a licence is not accepted.
 */
export function rustEntries(about: AboutOutput, linked: Set<string>, accepted: string[]): Entry[] {
  const entries: Entry[] = [];
  const covered = new Set<string>();
  for (const license of about.licenses) {
    for (const { crate } of license.used_by) {
      const key = `${crate.name}@${crate.version}`;
      if (!linked.has(key)) continue;
      if (!accepted.includes(license.id)) throw new Error(`${key} is under ${license.id}, which about.toml does not accept.`);
      covered.add(key);
      const authors = (crate.authors ?? []).map(authorName).filter(Boolean);
      entries.push({
        license: license.id,
        text: license.text,
        canonical: !license.source_path,
        item: {
          kind: "crate",
          name: crate.name,
          version: crate.version,
          repository: crate.repository ?? undefined,
          authors: authors.length ? authors : undefined,
        },
      });
    }
  }
  const missing = [...linked].filter((k) => !covered.has(k));
  if (missing.length) throw new Error(`cargo-about gave no licence for: ${missing.sort().join(", ")}`);
  return entries;
}

// ---- npm ----

/** The text of the first block comment in a source file, without its `*` decoration. */
export function leadingComment(source: string): string | null {
  const m = /^\s*\/\*([\s\S]*?)\*\//.exec(source);
  if (!m) return null;
  return m[1]
    .split("\n")
    .map((line) => line.replace(/^\s*\*+\s?/, "").replace(/\*+$/, ""))
    .join("\n");
}

/** `text` up to (not including) the first line that is exactly `end`. */
export function cutAt(text: string, end: string): string {
  const lines = normalizeText(text).split("\n");
  const i = lines.findIndex((l) => l.trim() === end);
  if (i < 0) throw new Error(`The licence text has no line "${end}".`);
  return lines.slice(0, i).join("\n");
}

/** Licence files to use for `license` among a package's licence files (by name). */
export function licenseFileFor(license: string, files: string[]): string | null {
  if (files.length === 1) return files[0];
  const key = license.split(" ")[0].toLowerCase();
  const named = files.filter((f) => f.toLowerCase().includes(key.split("-")[0]));
  return named.length === 1 ? named[0] : null;
}

// ---- rendering ----

/** Unix line ends, no trailing spaces, no blank lines at either end. */
export function normalizeText(text: string): string {
  return text
    .replace(/\r\n?/g, "\n")
    .split("\n")
    .map((l) => l.replace(/[ \t]+$/, ""))
    .join("\n")
    .replace(/^\n+|\n+$/g, "");
}

/** Full names for the headings. */
export const LICENSE_NAMES: Record<string, string> = {
  "0BSD": "BSD Zero Clause License",
  "Apache-2.0": "Apache License 2.0",
  "Apache-2.0 WITH LLVM-exception": "Apache License 2.0 with LLVM Exception",
  "BSD-2-Clause": 'BSD 2-Clause "Simplified" License',
  "BSD-3-Clause": 'BSD 3-Clause "New" or "Revised" License',
  "BSL-1.0": "Boost Software License 1.0",
  "CC0-1.0": "Creative Commons Zero v1.0 Universal",
  "CDLA-Permissive-2.0": "Community Data License Agreement Permissive 2.0",
  ISC: "ISC License",
  MIT: "MIT License",
  "MPL-2.0": "Mozilla Public License 2.0",
  "Unicode-3.0": "Unicode License v3",
  Unlicense: "The Unlicense",
  Zlib: "zlib License",
};

const itemKey = (i: Item) => `${i.kind === "crate" ? 0 : 1} ${i.name} ${i.version ?? ""}`;
const compare = (a: string, b: string) => (a < b ? -1 : a > b ? 1 : 0);
const label = (i: Item) => `${i.name}${i.version ? ` ${i.version}` : ""}`;
const kindName = (i: Item) => (i.kind === "crate" ? "Rust crate" : "npm package");
const RULE = "=".repeat(78);
const THIN = "-".repeat(78);

function sourceLink(i: Item): string {
  if (i.kind === "crate") return `https://crates.io/crates/${i.name}/${i.version}`;
  return i.version ? `https://www.npmjs.com/package/${i.name}/v/${i.version}` : (i.repository ?? "");
}

/** The whole notices file. Deterministic: the same entries in any order give the same text. */
export function renderNotices(entries: Entry[]): string {
  // license -> text -> items
  const byLicense = new Map<string, Map<string, { canonical: boolean; items: Map<string, Item> }>>();
  for (const e of entries) {
    const text = normalizeText(e.text);
    const texts = byLicense.get(e.license) ?? new Map();
    byLicense.set(e.license, texts);
    const slot = texts.get(text) ?? { canonical: e.canonical, items: new Map() };
    slot.canonical &&= e.canonical;
    slot.items.set(itemKey(e.item), e.item);
    texts.set(text, slot);
  }
  const licenses = [...byLicense.keys()].sort(compare);
  const packagesOf = (id: string) => {
    const all = new Map<string, Item>();
    for (const t of byLicense.get(id)!.values()) for (const [k, i] of t.items) all.set(k, i);
    return [...all.entries()].sort(([a], [b]) => compare(a, b)).map(([, i]) => i);
  };
  const everything = new Set(entries.map((e) => itemKey(e.item)));

  const out: string[] = [];
  out.push(
    "THIRD-PARTY SOFTWARE NOTICES FOR SATCHEL",
    "",
    "Satchel is open source under the MIT License (see LICENSE). The Satchel app also",
    "contains the third-party software listed below: Rust crates compiled into the app,",
    "and JavaScript packages whose code is in its web interface. Each is used under the",
    "licence it is listed with; the full texts, with their copyright notices, follow.",
    "Where a package offers a choice of licences, the one it is listed under applies.",
    "",
    "This file is generated by `pnpm notices` from Cargo.lock and the built web assets.",
    "Do not edit it by hand.",
    "",
    `${everything.size} packages under ${licenses.length} licences:`,
    "",
  );
  for (const id of licenses) out.push(`  ${String(packagesOf(id).length).padStart(4)}  ${id}  (${LICENSE_NAMES[id] ?? id})`);

  for (const id of licenses) {
    const items = packagesOf(id);
    out.push("", "", RULE, `${LICENSE_NAMES[id] ?? id} (${id})`, RULE, "", `Used by ${items.length} package${items.length === 1 ? "" : "s"}:`, "");
    for (const i of items) out.push(`  ${label(i)} (${kindName(i)})${i.note ? ` ${i.note}` : ""}`);
    if (id === "MPL-2.0") {
      out.push(
        "",
        "These packages are used unmodified. As the MPL-2.0 requires, their source code",
        "is available at:",
        "",
      );
      for (const i of items) out.push(`  ${label(i)}: ${sourceLink(i)}${i.repository ? ` (repository: ${i.repository})` : ""}`);
    }
    const texts = [...byLicense.get(id)!.entries()]
      .map(([text, slot]) => ({ text, canonical: slot.canonical, items: [...slot.items.entries()].sort(([a], [b]) => compare(a, b)).map(([, i]) => i) }))
      .sort((a, b) => compare(itemKey(a.items[0]), itemKey(b.items[0])) || compare(a.text, b.text));
    for (const t of texts) {
      out.push("", THIN, `${LICENSE_NAMES[id] ?? id}, for: ${t.items.map(label).join(", ")}`);
      if (t.canonical) {
        out.push(
          "",
          "These packages ship no licence file of their own; this is the standard text.",
          "Copyright holders, as each package lists its authors:",
        );
        for (const i of t.items) out.push(`  ${label(i)}: ${i.authors?.length ? i.authors.join(", ") : `no authors listed (see ${i.repository ?? sourceLink(i)})`}`);
      }
      out.push(THIN, "", t.text);
    }
  }
  return out.join("\n") + "\n";
}
