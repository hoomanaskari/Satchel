// Writes the update feed (latest.json) for a signed update archive and checks both against the
// public key every installed Satchel carries. Run by scripts/release.sh and
// scripts/update-rehearsal.sh, not by hand:
//
//   node scripts/update-feed.ts --version 0.2.0 --archive <dir>/Satchel_0.2.0_aarch64.app.tar.gz \
//     --notes docs/release-notes/0.2.0.md --url <where the archive will be> --out <dir>/latest.json
//
// The signature is read from <archive>.sig (written by `tauri signer sign --app-version`). The
// public key is the one in src-tauri/tauri.conf.json.
import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";
import { latestJson, verifyLatest } from "./lib/release-update.ts";

process.chdir(fileURLToPath(new URL("..", import.meta.url)));
const { values } = parseArgs({
  options: {
    version: { type: "string" },
    archive: { type: "string" },
    notes: { type: "string" },
    url: { type: "string" },
    out: { type: "string" },
  },
});
const { version, archive, notes, url, out } = values;
if (!version || !archive || !notes || !url || !out) {
  console.error("Use: node scripts/update-feed.ts --version V --archive FILE --notes FILE --url URL --out FILE");
  process.exit(1);
}

const publicKey: string = JSON.parse(readFileSync("src-tauri/tauri.conf.json", "utf8")).plugins.updater.pubkey;
const signature = readFileSync(`${archive}.sig`, "utf8");
const latest = latestJson({ version, notes: readFileSync(notes, "utf8"), date: new Date(), signature, url });
verifyLatest({ latest, version, url, archive: readFileSync(archive), signature, publicKey });
writeFileSync(out, JSON.stringify(latest, null, 2) + "\n");
console.log(`Update feed for Satchel ${version}: signature verified with Satchel's update key, version ${version} signed.`);
