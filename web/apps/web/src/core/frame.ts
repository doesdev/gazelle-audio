// High-rate writes (cyclic reports arrive tens of times a second) are applied once per animation
// frame, last value wins, in one batch so each affected effect runs at most once (spec §6.2).

import { batch, type Signal } from "./signal.ts";

export type RequestFrame = (callback: () => void) => unknown;

export interface FrameWriter {
  /** Queues `value` for `target`, replacing anything already queued for it this frame. */
  write<T>(target: Signal<T>, value: T): void;
  /** Applies queued writes now. */
  flush(): void;
  readonly pending: number;
}

export function frameWriter(requestFrame: RequestFrame = (callback) => requestAnimationFrame(callback)): FrameWriter {
  const queued = new Map<Signal<unknown>, unknown>();
  let scheduled = false;

  const flush = () => {
    scheduled = false;
    const writes = [...queued];
    queued.clear();
    batch(() => {
      for (const [target, value] of writes) target.value = value;
    });
  };

  return {
    write<T>(target: Signal<T>, value: T) {
      queued.set(target as Signal<unknown>, value);
      if (!scheduled) {
        scheduled = true;
        requestFrame(flush);
      }
    },
    flush,
    get pending() {
      return queued.size;
    },
  };
}
