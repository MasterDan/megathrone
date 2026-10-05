import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";
import solid from "vite-plugin-solid";
import tailwindcss from "@tailwindcss/vite";

// The Tauri shell's frontend. Everything UI lives in @megathrone/core
// (source-imported); the `@` alias always points at the core source tree,
// including for core-internal imports compiled by this build.
export default defineConfig({
  plugins: [solid(), tailwindcss()],
  resolve: {
    alias: {
      "@": fileURLToPath(new URL("../core/src", import.meta.url)),
      // @formkit/drag-and-drop imports solid-js without declaring it (a
      // phantom dependency); pin every resolution to this package's copy
      // so the pnpm store's undeclared walk-up cannot miss it
      "solid-js": fileURLToPath(new URL("node_modules/solid-js", import.meta.url)),
    },
  },
  clearScreen: false,
  // solid-icons ships raw JSX (its "module" entry is a .jsx file) — the dev
  // optimizer would pre-bundle it with esbuild's React JSX factory
  // ("React is not defined"); excluded, the file goes through the regular
  // transform pipeline where vite-plugin-solid compiles it instead
  optimizeDeps: {
    exclude: ["solid-icons"],
  },
  server: {
    port: 1420,
    strictPort: true,
  },
  envPrefix: ["VITE_", "TAURI_ENV_"],
  build: {
    target: process.env.TAURI_ENV_PLATFORM === "windows" ? "chrome105" : "safari13",
    minify: process.env.TAURI_ENV_DEBUG ? false : "esbuild",
    sourcemap: Boolean(process.env.TAURI_ENV_DEBUG),
  },
});
