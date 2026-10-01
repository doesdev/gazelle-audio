// The Mixer's guard of a cable dedicated to the phase measurement (`phase-path.ts`, `phaseGuard`),
// for the store the elements share. `phaseGuard` in element.ts fetches it the first time the
// workspace dedicates a cable, as a page's chunk is fetched with its route, so the app's first chunk
// carries none of the phase path's logic.

import { phaseGuard, type PhaseGuard } from "../store/phase-path.ts";
import { useStore } from "./element.ts";

export const guard = (): PhaseGuard => phaseGuard(useStore());
