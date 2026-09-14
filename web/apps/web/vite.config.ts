import { fileURLToPath } from "node:url";

import { defineConfig } from "vite";

/** Where `pnpm -C web dev` runs the control server; Vite proxies the API to it. */
export const SERVER_PORT = 8420;

export default defineConfig({
  root: fileURLToPath(new URL(".", import.meta.url)),
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
