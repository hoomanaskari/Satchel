// The update half of a release: Satchel's `latest.json` and a check that the update archive's
// signature is what every installed Satchel will accept. Used by scripts/release.sh and
// scripts/update-rehearsal.sh.
//
// Tauri's updater signs with minisign: the `.sig` file and the public key in tauri.conf.json are
// base64 of minisign's own text files. The archive is signed prehashed ("ED": Ed25519 over the
// BLAKE2b-512 of the file), and a second, global signature covers that signature and the
// trusted comment, which carries `version:<version>` (Satchel sets `requireSignedVersion`, so
// the app refuses an archive whose signed version is not the one announced). Node's crypto has
// both algorithms, so no minisign binary is needed.

import { createHash, createPublicKey, verify } from "node:crypto";

/** Where every installed Satchel looks for updates, and where release assets live. */
export const REPO = "hoomanaskari/Satchel";
export const FEED = `https://github.com/${REPO}/releases/latest/download/latest.json`;
/** Tauri's name for an Apple silicon Mac in `latest.json`. */
export const PLATFORM = "darwin-aarch64";

export const archiveName = (version: string, arch = "aarch64") => `Satchel_${version}_${arch}.app.tar.gz`;
export const assetUrl = (version: string, file: string) => `https://github.com/${REPO}/releases/download/v${version}/${file}`;

export interface LatestJson {
  version: string;
  notes: string;
  pub_date: string;
  platforms: Record<string, { signature: string; url: string }>;
}

/** What Satchel's updater reads: the newest version, its notes, and where its signed archive is. */
export function latestJson(o: { version: string; notes: string; date: Date; signature: string; url: string }): LatestJson {
  return {
    version: o.version,
    notes: o.notes.trim(),
    pub_date: o.date.toISOString(),
    platforms: { [PLATFORM]: { signature: o.signature.trim(), url: o.url } },
  };
}

function strictBase64(value: string, what: string): Buffer {
  const text = value.trim();
  const bytes = Buffer.from(text, "base64");
  if (!text || bytes.toString("base64") !== text) throw new Error(`${what} is not valid base64`);
  return bytes;
}

/** The lines of a minisign text file, given as Tauri stores it (base64 of the file). */
function minisignLines(encoded: string, what: string): string[] {
  const text = new TextDecoder("utf-8", { fatal: true }).decode(strictBase64(encoded, what));
  return text.split(/\r?\n/);
}

function ed25519Key(raw: Buffer) {
  return createPublicKey({ key: { kty: "OKP", crv: "Ed25519", x: raw.toString("base64url") }, format: "jwk" });
}

/**
 * Checks `archive` against `signature` (the `.sig` file's text) with `publicKey` (the `pubkey`
 * in tauri.conf.json), and that the signature binds `version`. Throws with the reason otherwise.
 */
export function verifyArchive(o: { archive: Buffer; signature: string; publicKey: string; version: string }): void {
  const pub = strictBase64(minisignLines(o.publicKey, "The public key")[1] ?? "", "The public key");
  if (pub.length !== 42 || pub.subarray(0, 2).toString() !== "Ed") throw new Error("Not a minisign public key");
  const lines = minisignLines(o.signature, "The signature");
  const sig = strictBase64(lines[1] ?? "", "The signature");
  if (sig.length !== 74 || sig.subarray(0, 2).toString() !== "ED") throw new Error("Not a prehashed minisign signature");
  if (!sig.subarray(2, 10).equals(pub.subarray(2, 10))) throw new Error("Signed with another key");
  const key = ed25519Key(pub.subarray(10));
  const digest = createHash("blake2b512").update(o.archive).digest();
  if (!verify(null, digest, key, sig.subarray(10))) throw new Error("The archive does not match its signature");
  const prefix = "trusted comment: ";
  if (!lines[2]?.startsWith(prefix)) throw new Error("The signature has no trusted comment");
  const comment = lines[2].slice(prefix.length);
  const global = strictBase64(lines[3] ?? "", "The global signature");
  if (!verify(null, Buffer.concat([sig.subarray(10), Buffer.from(comment)]), key, global)) {
    throw new Error("The trusted comment does not match its signature");
  }
  const versions = comment.split("\t").filter((f) => f.startsWith("version:"));
  if (versions.length !== 1 || versions[0] !== `version:${o.version}`) throw new Error(`The signature does not bind version ${o.version}`);
}

/** Checks a `latest.json` against the archive it announces: version, URL and signature. */
export function verifyLatest(o: { latest: LatestJson; version: string; url: string; archive: Buffer; signature: string; publicKey: string }): void {
  const entry = o.latest.platforms?.[PLATFORM];
  if (o.latest.version !== o.version) throw new Error(`latest.json announces ${o.latest.version}, not ${o.version}`);
  if (entry?.url !== o.url) throw new Error("latest.json points at another archive");
  if (entry.signature !== o.signature.trim()) throw new Error("latest.json carries another signature");
  if (!o.latest.notes) throw new Error("latest.json has no release notes");
  if (Number.isNaN(Date.parse(o.latest.pub_date))) throw new Error("latest.json has no valid date");
  verifyArchive({ archive: o.archive, signature: entry.signature, publicKey: o.publicKey, version: o.version });
}
