// Hash routes (spec §6.2): #/devices[/<device id>], #/workspace, #/mixer, #/routing.

import { signal } from "../core/signal.ts";

export type Page = "devices" | "workspace" | "mixer" | "routing";

export interface Route {
  page: Page;
  id?: string;
}

export const PAGES: readonly { page: Page; label: string }[] = [
  { page: "devices", label: "Devices" },
  { page: "workspace", label: "Workspace" },
  { page: "mixer", label: "Mixer" },
  { page: "routing", label: "Routing" },
];

export function parseRoute(hash: string): Route {
  const [, page, id] = hash.replace(/^#/, "").split("/");
  const known = PAGES.find((p) => p.page === page);
  if (known === undefined) return { page: "devices" };
  return id ? { page: known.page, id: decodeURIComponent(id) } : { page: known.page };
}

export function href(route: Route): string {
  return route.id === undefined ? `#/${route.page}` : `#/${route.page}/${encodeURIComponent(route.id)}`;
}

export const route = signal<Route>({ page: "devices" }, (a, b) => a.page === b.page && a.id === b.id);

/** Keeps `route` in step with the address bar until the returned function is called. */
export function followHash(): () => void {
  const update = () => {
    route.value = parseRoute(location.hash);
  };
  addEventListener("hashchange", update);
  update();
  return () => removeEventListener("hashchange", update);
}
