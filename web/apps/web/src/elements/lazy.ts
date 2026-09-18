// The elements that arrive with their route rather than with the app.
//
// `index.ts` registers everything the shell shows at once: the header, the sidebar's device list,
// meter and Control Room, the mixer and its dock, and the notices. The pages below are each built
// by one route, so each travels in a chunk of its own, fetched when that route opens
// (P125 did the same for the effect parameter catalogue). A **static** import of one of these
// modules from anywhere the app loads eagerly would put it straight back in the entry chunk, which
// `test/bundle-split.test.ts` guards against by looking at what the build wrote.
//
// A tag's loader defines everything that tag needs before the element itself, so a page whose
// children are lazy too (the surface page's strips) is never inserted half-registered.

/** The definitions a tag's chunk brings, the tag itself last. */
type Definitions = readonly (readonly [string, CustomElementConstructor])[];

const loaders: Readonly<Record<string, () => Promise<Definitions>>> = {
  "ga-device-status": async () => [["ga-device-status", (await import("./device-status.ts")).GaDeviceStatus]],
  "ga-workspace": async () => [["ga-workspace", (await import("./workspace.ts")).GaWorkspace]],
  "ga-inputs": async () => [["ga-inputs", (await import("./inputs-page.ts")).GaInputs]],
  "ga-outputs": async () => [["ga-outputs", (await import("./outputs-page.ts")).GaOutputs]],
  "ga-routing": async () => [["ga-routing", (await import("./routing-page.ts")).GaRouting]],
  "ga-effects": async () => [["ga-effects", (await import("./effects-page.ts")).GaEffects]],
  // The surface page is made of surface strips, which the mixer dock can show as well: one chunk
  // holds both, and whichever asks first defines both.
  "ga-surface": async () => {
    const [page, strip] = await Promise.all([import("./surface-page.ts"), import("./surface-strip.ts")]);
    return [
      ["ga-surface-strip", strip.GaSurfaceStrip],
      ["ga-surface", page.GaSurface],
    ];
  },
  "ga-surface-strip": async () => [["ga-surface-strip", (await import("./surface-strip.ts")).GaSurfaceStrip]],
};

/** Every tag fetched with its route. `index.ts` must register none of them. */
export const LAZY_TAGS: readonly string[] = Object.keys(loaders);

/** Whether `tag` comes with its route rather than with the app, fetched or not. */
export function isLazy(tag: string): boolean {
  return Object.hasOwn(loaders, tag);
}

/** Whether `tag` can be built now: it is not lazy, or its chunk is already here. */
export function isReady(tag: string): boolean {
  return !isLazy(tag) || customElements.get(tag) !== undefined;
}

const loading = new Map<string, Promise<void>>();

/**
 * Fetches `tag`'s chunk and defines what it holds. Callers that ask while a fetch is in flight share
 * it, however many pages or docks are waiting, and one that has arrived is never fetched twice.
 *
 * A fetch that failed stays failed for this document: a browser remembers a module fetch that did
 * not work and refuses the same file again without going near the network, so there is nothing to
 * gain by forgetting it here. The way back is a reload, which is what the message the shell leaves
 * offers.
 */
export function loadElement(tag: string): Promise<void> {
  if (isReady(tag)) return Promise.resolve();
  let started = loading.get(tag);
  if (started === undefined) {
    started = (loaders[tag] as () => Promise<Definitions>)().then((definitions) => {
      for (const [name, element] of definitions) if (customElements.get(name) === undefined) customElements.define(name, element);
    });
    loading.set(tag, started);
  }
  return started;
}
