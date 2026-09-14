import { fileURLToPath } from "node:url";

import { defaultClientConditions, defineConfig } from "vite";

/** Where `pnpm -C web dev` runs the control server; Vite proxies the API to it. */
export const SERVER_PORT = 8420;

export default defineConfig({
  root: fileURLToPath(new URL(".", import.meta.url)),
  resolve: {
    // Inside the repository gazelle-audio-client resolves to its TypeScript source (the
    // `gazelle-source` export condition), so the app never needs a prebuilt client.
    conditions: ["gazelle-source", ...defaultClientConditions],
  },
  server: {
    proxy: {
      // HTTP and the /api/v1/ws WebSocket share one origin with the page, as when the server
      // serves the built UI itself.
      "/api": { target: `http://127.0.0.1:${SERVER_PORT}`, ws: true },
    },
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
    target: "es2022",
  },
});
