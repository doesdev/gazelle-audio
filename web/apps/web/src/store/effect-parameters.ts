// The effect parameter catalogue, fetched when the Effects page opens rather than with the app.
// `effect-parameters-data.ts` is generated from both panels' bytecode and is about 116 kB of the
// bundle, which only the Effects page and its editor need, so it is reached through this module:
// the types are re-exported (erased at build time, so they cost nothing), and the data arrives
// through a dynamic import, which the bundler puts in a chunk of its own.
//
// `catalogue` is undefined until it has arrived. It is a signal, so a page that reads it is built
// again when it does; the Effects page starts the fetch as it opens and shows an effect's editor
// once it is there. Nothing else in the app reads an effect's parameters.

import { signal, type ReadonlySignal } from "../core/signal.ts";

export type { EffectControlLayout, EffectDescription, EffectFamily, EffectLayouts, EffectParameter } from "./effect-parameters-data.ts";

import type { EffectDescription, EffectFamily } from "./effect-parameters-data.ts";

export interface EffectCatalogue {
  /** Every effect type whose parameters the editor knows, by family. */
  readonly parameters: Readonly<Record<EffectFamily, ReadonlyMap<number, EffectDescription>>>;
  /** Effect types the editor leaves out, with the reason. */
  readonly unsupported: Readonly<Record<EffectFamily, ReadonlyMap<number, string>>>;
}

const state = signal<EffectCatalogue | undefined>(undefined);

/** The catalogue once it has been fetched, and undefined before that. */
export const catalogue: ReadonlySignal<EffectCatalogue | undefined> = state;

let arriving: Promise<EffectCatalogue> | undefined;

/** Fetches the catalogue, or resolves with the one already here. Concurrent calls share the fetch. */
export function loadCatalogue(): Promise<EffectCatalogue> {
  const here = state.peek();
  if (here !== undefined) return Promise.resolve(here);
  arriving ??= import("./effect-parameters-data.ts").then((module) => {
    const loaded: EffectCatalogue = { parameters: module.EFFECT_PARAMETERS, unsupported: module.UNSUPPORTED_EFFECTS };
    state.value = loaded;
    return loaded;
  });
  return arriving;
}
