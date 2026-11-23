import { defineConfig } from "vitest/config";
import { sveltekit } from "@sveltejs/kit/vite";
// @ts-expect-error type error without @types/node package
import process from "node:process";
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(() => ({
  plugins: [sveltekit()],

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || "127.0.0.1",
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`, the Rust crates and build output
      ignored: ["**/src-tauri/**", "**/crates/**", "**/target/**"],
    },
  },

  build: {
    // WebKit on macOS 12+ / WebView2 / WebKitGTK all support ES2022.
    target: "es2022",
    // The app is local-only; no source maps in the shipped bundle.
    sourcemap: false,
  },

  test: {
    include: ["src/**/*.test.ts"],
    environment: "node",
  },
}));
