// Hash routes (spec §6.2): #/devices[/<device id>], #/workspace, #/inputs[/<device id>],
// #/mixer[/<device id>[/<mixer>]], #/routing.

import { signal } from "../core/signal.ts";

export type Page = "devices" | "workspace" | "inputs" | "mixer" | "routing";

export interface Route {
  page: Page;
  id?: string;
  /** A further segment, such as the mixer index. */
  sub?: string;
}

export const PAGES: readonly { page: Page; label: string }[] = [
  { page: "devices", label: "Devices" },
  { page: "workspace", label: "Workspace" },
  { page: "inputs", label: "Inputs" },
  { page: "mixer", label: "Mixer" },
  { page: "routing", label: "Routing" },
];

export function parseRoute(hash: string): Route {
  const [, page, id, sub] = hash.replace(/^#/, "").split("/");
  const known = PAGES.find((p) => p.page === page);
  if (known === undefined) return { page: "devices" };
  if (!id) return { page: known.page };
  return sub ? { page: known.page, id: decodeURIComponent(id), sub: decodeURIComponent(sub) } : { page: known.page, id: decodeURIComponent(id) };
}

export function href(route: Route): string {
  let path = `#/${route.page}`;
  if (route.id !== undefined) path += `/${encodeURIComponent(route.id)}`;
  if (route.id !== undefined && route.sub !== undefined) path += `/${encodeURIComponent(route.sub)}`;
  return path;
}

export const route = signal<Route>({ page: "devices" }, (a, b) => a.page === b.page && a.id === b.id && a.sub === b.sub);

/** Keeps `route` in step with the address bar until the returned function is called. */
export function followHash(): () => void {
  const update = () => {
    route.value = parseRoute(location.hash);
  };
  addEventListener("hashchange", update);
  update();
  return () => removeEventListener("hashchange", update);
}
