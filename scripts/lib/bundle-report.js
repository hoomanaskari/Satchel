// Vite plugin for `pnpm notices` (scripts/notices.ts): with SATCHEL_BUNDLE_REPORT set to a file
// path, the client build writes there, as a JSON array, the id of every module whose code ends up
// in the shipped web assets (tree-shaken modules with no rendered code are left out). Without the
// variable it does nothing, so normal builds are unchanged.
// @ts-expect-error no @types/node in this project (like vite.config.js)
import { writeFileSync } from "node:fs";
// @ts-expect-error no @types/node in this project
import process from "node:process";

/** @returns {import("vite").Plugin} */
export function bundleReport() {
  const out = process.env.SATCHEL_BUNDLE_REPORT;
  return {
    name: "satchel-bundle-report",
    apply: "build",
    generateBundle(_options, bundle) {
      // SvelteKit also builds a server bundle to prerender the pages; none of it ships.
      if (!out || this.environment?.name !== "client") return;
      const ids = new Set();
      for (const chunk of Object.values(bundle)) {
        if (chunk.type !== "chunk") continue;
        for (const [id, mod] of Object.entries(chunk.modules)) if (mod.renderedLength > 0) ids.add(id);
      }
      writeFileSync(out, JSON.stringify([...ids].sort()));
    },
  };
}
