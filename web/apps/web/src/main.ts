// Entry point. Fonts are bundled from Fontsource (SIL OFL), so the UI works offline and makes no
// third-party requests: Josefin Sans for headings, titles and large values; Inter for labels.
// Built-in themes come from apps/web/themes and community themes from web/themes; user themes
// are fetched from the server by the store.

import "@fontsource-variable/inter";
import "@fontsource-variable/josefin-sans";

import { effect } from "./core/signal.ts";
import { DRAWER_MAX_PX } from "./elements/app.ts";
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

/** A device that is not paired gets 401 from everything; it is told how to pair rather than that the server is gone. */
async function unpaired(): Promise<boolean> {
  try {
    return (await fetch("/api/v1/health")).status === 401;
  } catch {
    return false;
  }
}

async function boot(): Promise<void> {
  // A phone opening the QR code's address: the pair page, in a chunk of its own, and not the app,
  // which it may not use until it has paired.
  if (location.pathname === "/pair") {
    const { showPairPage } = await import("./elements/pair-page.ts");
    showPairPage();
    return;
  }
  try {
    // At phone width the open mixer dock would take a quarter of the screen.
    const narrow = matchMedia(`(max-width: ${DRAWER_MAX_PX}px)`).matches;
    const store = await openStore(location.origin, { themeSources, narrow });
    provideStore(store);
    // A phone opening the app's plain address lands on the page laid out for it.
    if (store.phone && location.hash === "") history.replaceState(history.state, "", "#/remote");
    document.body.replaceChildren(document.createElement("ga-app"));
    // Revoked on the computer while open: the app stops, and says so in its own place.
    effect(() => {
      if (store.unpaired.value) void showRevoked();
    });
  } catch (error) {
    const box = document.createElement("div");
    box.className = "boot-error";
    const heading = document.createElement("h1");
    const detail = document.createElement("p");
    if (await unpaired()) {
      const { UNPAIRED_TITLE, UNPAIRED_TEXT } = await import("./store/pair.ts");
      heading.textContent = UNPAIRED_TITLE;
      detail.textContent = UNPAIRED_TEXT;
    } else {
      heading.textContent = "Gazelle cannot reach its server";
      detail.textContent = `${location.origin}: ${error instanceof Error ? error.message : String(error)}`;
    }
    const retry = document.createElement("button");
    retry.textContent = "Try again";
    retry.addEventListener("click", () => location.reload());
    box.append(heading, detail, retry);
    document.body.replaceChildren(box);
  }
}

/**
 * The page a phone is left with once the computer has revoked it: the connection has stopped for
 * good, since every try would be refused, and the way back is to pair again.
 */
async function showRevoked(): Promise<void> {
  const { REVOKED_TITLE, REVOKED_TEXT } = await import("./store/pair.ts");
  const box = document.createElement("div");
  box.className = "boot-error";
  box.dataset["testid"] = "unpaired";
  const heading = document.createElement("h1");
  heading.textContent = REVOKED_TITLE;
  const detail = document.createElement("p");
  detail.textContent = REVOKED_TEXT;
  const again = document.createElement("a");
  again.href = "/pair";
  again.textContent = "Pair again";
  box.append(heading, detail, again);
  document.body.replaceChildren(box);
}

void boot();
