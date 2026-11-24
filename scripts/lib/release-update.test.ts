import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { FEED, PLATFORM, archiveName, assetUrl, latestJson, verifyArchive, verifyLatest } from "./release-update";

// Signed by `tauri signer sign --app-version 0.0.2` with a throwaway key (not Satchel's; its
// private half was never kept).
const dir = join(import.meta.dirname, "fixtures");
const archive = readFileSync(join(dir, "update-archive.bin"));
const signature = readFileSync(join(dir, "update-archive.bin.sig"), "utf8");
const publicKey = readFileSync(join(dir, "update-test-key.pub"), "utf8");
const satchelKey = JSON.parse(readFileSync(join(import.meta.dirname, "../../src-tauri/tauri.conf.json"), "utf8")).plugins.updater.pubkey;

describe("update archives", () => {
  it("accepts an archive signed for its version", () => {
    expect(() => verifyArchive({ archive, signature, publicKey, version: "0.0.2" })).not.toThrow();
  });

  it("rejects a changed archive, another version, or another key", () => {
    const tampered = Buffer.from(archive);
    tampered[0] ^= 1;
    expect(() => verifyArchive({ archive: tampered, signature, publicKey, version: "0.0.2" })).toThrow(/does not match/);
    expect(() => verifyArchive({ archive, signature, publicKey, version: "0.0.3" })).toThrow(/does not bind version 0.0.3/);
    expect(() => verifyArchive({ archive, signature, publicKey: satchelKey, version: "0.0.2" })).toThrow(/another key/);
  });

  it("rejects a forged trusted comment", () => {
    const text = Buffer.from(signature, "base64").toString().replace("version:0.0.2", "version:0.0.9");
    const forged = Buffer.from(text).toString("base64");
    expect(() => verifyArchive({ archive, signature: forged, publicKey, version: "0.0.9" })).toThrow(/trusted comment/);
  });
});

describe("latest.json", () => {
  const url = assetUrl("0.0.2", archiveName("0.0.2"));

  it("points the feed and the archive at the GitHub release", () => {
    expect(FEED).toBe("https://github.com/hoomanaskari/Satchel/releases/latest/download/latest.json");
    expect(url).toBe("https://github.com/hoomanaskari/Satchel/releases/download/v0.0.2/Satchel_0.0.2_aarch64.app.tar.gz");
  });

  it("is what Tauri's updater reads, and is checked against the archive", () => {
    const latest = latestJson({ version: "0.0.2", notes: "  What's new\n", date: new Date(0), signature, url });
    expect(latest).toEqual({
      version: "0.0.2",
      notes: "What's new",
      pub_date: "1970-01-01T00:00:00.000Z",
      platforms: { [PLATFORM]: { signature: signature.trim(), url } },
    });
    expect(() => verifyLatest({ latest, version: "0.0.2", url, archive, signature, publicKey })).not.toThrow();
    expect(() => verifyLatest({ latest: { ...latest, version: "0.0.3" }, version: "0.0.2", url, archive, signature, publicKey })).toThrow();
    expect(() => verifyLatest({ latest, version: "0.0.2", url: assetUrl("0.0.2", "other"), archive, signature, publicKey })).toThrow();
  });
});
