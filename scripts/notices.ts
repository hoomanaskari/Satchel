// pnpm notices           writes THIRD-PARTY-NOTICES.txt: the licences of the third-party code in
//                        Satchel.app (see docs/BUILDING.md, "Third-party notices").
// pnpm notices --check   fails when the file is not what would be written now.
//
// Rust: the crates compiled into the app binary for the Mac targets in scripts/notices/about.toml
// (`cargo tree -e normal,no-proc-macro`: no build scripts, proc-macros or dev dependencies), with
// licence texts and copyright notices found by cargo-about in each crate's sources.
// Web: the npm packages whose code is in the built web assets, as the bundler reports them
// (scripts/lib/bundle-report.js), for every build mode in scripts/notices/config.json.
// A licence missing from about.toml's `accepted` list stops generation.
import { execFileSync } from "node:child_process";
import { existsSync, mkdtempSync, readdirSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { basename, dirname, join, relative, sep } from "node:path";
import { fileURLToPath } from "node:url";
import {
  chooseLicenses,
  cutAt,
  leadingComment,
  licenseFileFor,
  parseAccepted,
  parseCargoTree,
  renderNotices,
  rustEntries,
  type AboutOutput,
  type Entry,
} from "./lib/notices.ts";

const root = fileURLToPath(new URL("..", import.meta.url));
process.chdir(root);
const OUTPUT = "THIRD-PARTY-NOTICES.txt";
const CONFIG_DIR = "scripts/notices";

interface PackageConfig {
  licenseEnd?: string;
  extraFiles?: string[];
  note?: string;
}
interface Config {
  cargoAbout: string;
  rust: { manifest: string; package: string; features: string[] };
  web: {
    modes: string[];
    virtualModules: Record<string, string>;
    packages: Record<string, PackageConfig>;
    embedded: { name: string; license: string; module: string; note?: string; repository?: string }[];
  };
}

const config: Config = JSON.parse(readFileSync(join(CONFIG_DIR, "config.json"), "utf8"));
const aboutToml = join(CONFIG_DIR, "about.toml");
const accepted = parseAccepted(readFileSync(aboutToml, "utf8"));
const targets = JSON.parse(/^targets\s*=\s*(\[.*\])/m.exec(readFileSync(aboutToml, "utf8"))?.[1] ?? "[]") as string[];

function stop(message: string): never {
  console.error(`Stopped: ${message}`);
  process.exit(1);
}

const run = (cmd: string, args: string[], env?: NodeJS.ProcessEnv) => {
  try {
    return execFileSync(cmd, args, { encoding: "utf8", maxBuffer: 1 << 28, env: { ...process.env, ...env }, stdio: ["ignore", "pipe", "pipe"] });
  } catch (e) {
    const err = e as { stderr?: string; stdout?: string };
    stop(`${cmd} ${args.join(" ")} failed:\n${err.stdout ?? ""}${err.stderr ?? String(e)}`);
  }
};

// ---- Rust ----

function rust(scratch: string): Entry[] {
  let version = "";
  try {
    version = execFileSync("cargo", ["about", "--version"], { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }).trim();
  } catch {
    /* reported below */
  }
  if (version !== `cargo-about ${config.cargoAbout}`) {
    stop(
      `pnpm notices needs cargo-about ${config.cargoAbout}${version ? ` (found ${version})` : ""}. Install it with:\n` +
        `  cargo install cargo-about --locked --features cli --version ${config.cargoAbout}`,
    );
  }
  // Every crate's sources are needed; after this nothing touches the network.
  run("cargo", ["fetch", "--locked", "--manifest-path", config.rust.manifest]);
  const linked = new Set<string>();
  for (const target of targets) {
    const tree = run("cargo", [
      "tree", "--frozen", "--manifest-path", config.rust.manifest, "-p", config.rust.package,
      "--target", target, "-e", "normal,no-proc-macro", "--features", config.rust.features.join(","),
      "--prefix", "none", "-f", "{p}",
    ]);
    for (const c of parseCargoTree(tree)) linked.add(c);
  }
  const json = join(scratch, "about.json");
  run("cargo", ["about", "generate", "--format", "json", "--frozen", "--fail", "-c", aboutToml, "-m", config.rust.manifest, "--features", config.rust.features.join(" "), "-o", json]);
  return rustEntries(JSON.parse(readFileSync(json, "utf8")) as AboutOutput, linked, accepted);
}

// ---- web ----

interface Pkg {
  dir: string;
  name: string;
  version: string;
  license: string;
  repository?: string;
}

/** The npm package a file belongs to: the nearest package.json with a name, inside node_modules. */
function packageOf(file: string): Pkg | null {
  let dir = dirname(realpathSync(file));
  while (dir !== dirname(dir)) {
    const pj = join(dir, "package.json");
    if (existsSync(pj) && dir.split(sep).includes("node_modules")) {
      const p = JSON.parse(readFileSync(pj, "utf8"));
      if (p.name && p.version) {
        const repository = typeof p.repository === "string" ? p.repository : p.repository?.url;
        return {
          dir,
          name: p.name,
          version: p.version,
          license: typeof p.license === "string" ? p.license : "",
          repository: repository?.replace(/^git\+/, "").replace(/\.git$/, "") || undefined,
        };
      }
    }
    dir = dirname(dir);
  }
  return null;
}

/** `vite > rolldown`: rolldown as resolved from vite's own folder. */
function resolveChain(chain: string): Pkg {
  let from = join(root, "package.json");
  for (const name of chain.split(">").map((s) => s.trim())) from = createRequire(from).resolve(`${name}/package.json`);
  return packageOf(from) ?? stop(`can't find the package for ${chain}.`);
}

function webEntries(scratch: string): Entry[] {
  const modules = new Set<string>();
  for (const mode of config.web.modes) {
    const report = join(scratch, `bundle-${mode}.json`);
    run(process.execPath, [join(root, "node_modules", "vite", "bin", "vite.js"), "build", "--mode", mode], { SATCHEL_BUNDLE_REPORT: report });
    if (!existsSync(report)) stop(`the ${mode} web build wrote no bundle report.`);
    for (const id of JSON.parse(readFileSync(report, "utf8")) as string[]) modules.add(id);
  }

  const packages = new Map<string, Pkg>();
  for (const id of modules) {
    let pkg: Pkg | null;
    if (id.startsWith("\0")) {
      const chain = config.web.virtualModules[id];
      if (!chain) stop(`the bundle has a generated module that scripts/notices/config.json doesn't name: ${JSON.stringify(id)}`);
      pkg = resolveChain(chain);
    } else {
      const file = id.split("?")[0];
      if (!existsSync(file)) stop(`can't find the bundled module ${id}`);
      pkg = packageOf(file);
      if (!pkg) {
        const rel = relative(root, realpathSync(file));
        // Satchel's own code, and what SvelteKit generates from it.
        if (rel.startsWith(`src${sep}`) || rel.startsWith(`.svelte-kit${sep}`)) continue;
        stop(`the bundled module ${rel} belongs to no package.`);
      }
    }
    packages.set(`${pkg.name}@${pkg.version}`, pkg);
  }

  const entries: Entry[] = [];
  for (const pkg of [...packages.values()]) {
    const extra = config.web.packages[pkg.name] ?? {};
    const chosen = pkg.license ? chooseLicenses(pkg.license, accepted) : null;
    if (!chosen) stop(`${pkg.name}@${pkg.version} is under "${pkg.license || "no licence"}", which about.toml does not accept.`);
    const files = readdirSync(pkg.dir).filter((f) => /^(licen[sc]e|copying)/i.test(f)).sort();
    for (const license of chosen) {
      const file = licenseFileFor(license, files);
      if (!file) stop(`${pkg.name}@${pkg.version}: can't tell which of [${files.join(", ")}] is its ${license} licence.`);
      let text = readFileSync(join(pkg.dir, file), "utf8");
      if (extra.licenseEnd) text = cutAt(text, extra.licenseEnd);
      for (const f of extra.extraFiles ?? []) text += "\n\n" + readFileSync(join(pkg.dir, f), "utf8");
      entries.push({
        license,
        text,
        canonical: false,
        item: { kind: "npm", name: pkg.name, version: pkg.version, repository: pkg.repository, note: extra.note },
      });
    }
  }

  for (const e of config.web.embedded) {
    const [scope, name, ...rest] = e.module.split("/");
    const owner = e.module.startsWith("@") ? `${scope}/${name}` : scope;
    const tail = e.module.startsWith("@") ? rest : [name, ...rest];
    const file = [...modules].find((id) => !id.startsWith("\0") && id.split(sep).join("/").endsWith(`/node_modules/${owner}/${tail.join("/")}`));
    if (!file) stop(`scripts/notices/config.json names ${e.module}, which is no longer in the bundle.`);
    if (!accepted.includes(e.license)) stop(`${e.name} is under ${e.license}, which about.toml does not accept.`);
    const text = leadingComment(readFileSync(file, "utf8"));
    if (!text) stop(`${e.module} has no licence comment at the top.`);
    entries.push({ license: e.license, text, canonical: false, item: { kind: "npm", name: e.name, repository: e.repository, note: e.note } });
  }
  return entries;
}

// ---- main ----

const args = process.argv.slice(2);
const check = args[0] === "--check";
if (args.length > (check ? 1 : 0)) stop("use: pnpm notices [--check]");

const scratch = mkdtempSync(join(tmpdir(), "satchel-notices-"));
let text: string;
try {
  text = renderNotices([...rust(scratch), ...webEntries(scratch)]);
} finally {
  rmSync(scratch, { recursive: true, force: true });
}
// Paths on this Mac must never end up in a shipped file.
for (const leak of [root, process.env.HOME, tmpdir()].filter((p): p is string => !!p && p.length > 1)) {
  if (text.includes(leak)) stop(`the notices would contain a local path (${basename(leak)}).`);
}

if (check) {
  const current = existsSync(OUTPUT) ? readFileSync(OUTPUT, "utf8") : "";
  if (current !== text) stop(`${OUTPUT} is out of date. Run pnpm notices and commit the result.`);
  console.log(`${OUTPUT} is up to date.`);
} else {
  writeFileSync(OUTPUT, text);
  console.log(`Wrote ${OUTPUT} (${text.length} bytes).`);
}
