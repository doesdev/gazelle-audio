import { fileURLToPath } from "node:url";

import { defaultClientConditions, defineConfig, type Plugin } from "vite";

/** Where `pnpm -C web dev` runs the control server; Vite proxies the API to it. */
export const SERVER_PORT = 8420;

/**
 * The client's command and report layouts are a chunk of their own, which it fetches as it connects
 * (`loadSchemas` in gazelle-audio-client), and the app connects the moment it starts. So the page
 * names that chunk for the browser to fetch beside the app's own, as Vite does for what the app
 * imports, and it is already here when the socket opens rather than asked for then.
 */
function preloadSchemas(): Plugin {
  return {
    name: "gazelle-preload-schemas",
    apply: "build",
    transformIndexHtml: {
      order: "post",
      handler(_html, context) {
        const chunk = Object.values(context.bundle ?? {}).find((one) => one.type === "chunk" && (one.facadeModuleId ?? "").replace(/\\/g, "/").endsWith("/src/generated/schemas.ts"));
        if (chunk === undefined) throw new Error("the build has no chunk of the client's schemas to preload");
        return [{ tag: "link", attrs: { rel: "modulepreload", crossorigin: true, href: `/${chunk.fileName}` }, injectTo: "head" }];
      },
    },
  };
}

export default defineConfig({
  root: fileURLToPath(new URL(".", import.meta.url)),
  plugins: [preloadSchemas()],
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
