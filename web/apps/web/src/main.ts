// Entry point. Fonts are bundled from Fontsource (SIL OFL), so the UI works offline and makes no
// third-party requests: Josefin Sans for headings, titles and large values; Inter for labels.
// Built-in themes come from apps/web/themes and community themes from web/themes; user themes
// are fetched from the server by the store.

import "@fontsource-variable/inter";
import "@fontsource-variable/josefin-sans";

import { provideStore } from "./elements/index.ts";
import { openStore } from "./store/store.ts";
import type { ThemeSource } from "./themes/theme.ts";

const builtIn = import.meta.glob<unknown>(["../themes/*.json", "!../themes/theme.schema.json"], { eager: true, import: "default" });
const community = import.meta.glob<unknown>("../../../themes/*.json", { eager: true, import: "default" });

const stem = (path: string) => path.slice(path.lastIndexOf("/") + 1, -".json".length);

const themeSources: ThemeSource[] = [
  ...Object.entries(builtIn).map(([path, data]) => ({ id: stem(path), origin: "built-in" as const, data })),
  ...Object.entries(community).map(([path, data]) => ({ id: `community:${stem(path)}`, origin: "community" as const, data })),
];

async function boot(): Promise<void> {
  try {
    const store = await openStore(location.origin, { themeSources });
    provideStore(store);
    document.body.replaceChildren(document.createElement("ga-app"));
  } catch (error) {
    const box = document.createElement("div");
    box.className = "boot-error";
    const heading = document.createElement("h1");
    heading.textContent = "Gazelle cannot reach its server";
    const detail = document.createElement("p");
    detail.textContent = `${location.origin}: ${error instanceof Error ? error.message : String(error)}`;
    const retry = document.createElement("button");
    retry.textContent = "Try again";
    retry.addEventListener("click", () => location.reload());
    box.append(heading, detail, retry);
    document.body.replaceChildren(box);
  }
}

void boot();
