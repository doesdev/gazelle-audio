// `pnpm -C web build`: writes apps/web/dist, which gazelle-audio-server embeds at build time.

import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { build } from "vite";

const app = fileURLToPath(new URL("..", import.meta.url));
await build({ configFile: join(app, "vite.config.ts") });
