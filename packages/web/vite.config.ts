import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";
import solid from "vite-plugin-solid";
import tailwindcss from "@tailwindcss/vite";

// The browser shell's frontend — the same @megathrone/core UI, talking to a
// megathrone-web server instead of the Tauri IPC (see src/webContext.tsx).
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
    port: 1421,
    strictPort: true,
    // the console app (src-tauri/crates/megathrone-web) serves this UI on
    // the origin it runs on; in dev proxy its future API to a local instance
    proxy: {
      "/api": {
        target: "http://127.0.0.1:8420",
        changeOrigin: true,
      },
    },
  },
  envPrefix: ["VITE_"],
});
