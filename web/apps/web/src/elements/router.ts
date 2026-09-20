// Hash routes: #/devices[/<device id>], #/workspace, #/inputs[/<device id>],
// #/outputs[/<device id>], #/mixer[/<device id>[/<mixer>]], #/routing, #/effects[/<device id>],
// #/aggregate, and #/surface/<surface id>, which has no tab of its own: surfaces are opened from
// the Workspace page.

import { signal } from "../core/signal.ts";

export type Page = "devices" | "workspace" | "inputs" | "outputs" | "mixer" | "routing" | "effects" | "aggregate" | "surface";

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
  { page: "outputs", label: "Outputs" },
  { page: "mixer", label: "Mixer" },
  { page: "routing", label: "Routing" },
  { page: "effects", label: "Effects" },
  { page: "aggregate", label: "Aggregate" },
];

export function parseRoute(hash: string): Route {
  const [, page, id, sub] = hash.replace(/^#/, "").split("/");
  if (page === "surface" && id) return { page: "surface", id: decodeURIComponent(id) };
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

/**
 * Changes the address in place, without a history entry or a `hashchange`: for a choice made on the
 * page that the address should carry (the Mixer page's mix), where the page itself stays as it is.
 */
export function replaceRoute(next: Route): void {
  history.replaceState(history.state, "", href(next));
  route.value = parseRoute(location.hash);
}

/** Keeps `route` in step with the address bar until the returned function is called. */
export function followHash(): () => void {
  const update = () => {
    route.value = parseRoute(location.hash);
  };
  addEventListener("hashchange", update);
  update();
  return () => removeEventListener("hashchange", update);
}
