// Signals, computeds and effects (spec §6.2). Writes push a "stale" mark down to observers;
// values are pulled lazily and compared by version, so a computed recomputes only when a source
// really changed and an effect never sees a half-updated graph. Effects run once per batch.
// Computeds keep their subscriptions for their lifetime: create them in the store, not per render.

export type Equals<T> = (a: T, b: T) => boolean;

export interface ReadonlySignal<T> {
  readonly value: T;
  /** The value without subscribing the running computed or effect. */
  peek(): T;
}

export interface Signal<T> extends ReadonlySignal<T> {
  value: T;
}

interface Source {
  version: number;
  readonly observers: Set<Observer>;
  refresh(): void;
}

interface Observer {
  sources: Map<Source, number>;
  next: Map<Source, number>;
  markStale(): void;
}

const FLUSH_LIMIT = 100;
let active: Observer | undefined;
let batchDepth = 0;
const pending = new Set<EffectNode>();

function track(source: Source): void {
  if (active !== undefined && !active.next.has(source)) active.next.set(source, source.version);
}

function runTracked<T>(observer: Observer, fn: () => T): T {
  const previous = active;
  active = observer;
  observer.next = new Map();
  try {
    return fn();
  } finally {
    active = previous;
    for (const source of observer.sources.keys()) if (!observer.next.has(source)) source.observers.delete(observer);
    for (const source of observer.next.keys()) source.observers.add(observer);
    observer.sources = observer.next;
  }
}

function changed(sources: Map<Source, number>): boolean {
  for (const [source, seen] of sources) {
    source.refresh();
    if (source.version !== seen) return true;
  }
  return false;
}

class SignalNode<T> implements Source {
  version = 0;
  readonly observers = new Set<Observer>();
  #value: T;
  readonly #equals: Equals<T>;

  constructor(value: T, equals: Equals<T>) {
    this.#value = value;
    this.#equals = equals;
  }

  refresh(): void {}

  get value(): T {
    track(this);
    return this.#value;
  }

  set value(next: T) {
    if (this.#equals(this.#value, next)) return;
    this.#value = next;
    this.version++;
    batch(() => {
      for (const observer of [...this.observers]) observer.markStale();
    });
  }

  peek(): T {
    return this.#value;
  }
}

class ComputedNode<T> implements Source, Observer {
  version = 0;
  readonly observers = new Set<Observer>();
  sources = new Map<Source, number>();
  next = new Map<Source, number>();
  #value: T | undefined;
  #initialised = false;
  #stale = true;
  #computing = false;
  readonly #fn: () => T;
  readonly #equals: Equals<T>;

  constructor(fn: () => T, equals: Equals<T>) {
    this.#fn = fn;
    this.#equals = equals;
  }

  markStale(): void {
    if (this.#stale) return;
    this.#stale = true;
    for (const observer of [...this.observers]) observer.markStale();
  }

  refresh(): void {
    if (!this.#stale) return;
    if (this.#initialised && !changed(this.sources)) {
      this.#stale = false;
      return;
    }
    if (this.#computing) throw new Error("a computed depends on itself");
    this.#computing = true;
    try {
      const next = runTracked(this, this.#fn);
      if (!this.#initialised || !this.#equals(this.#value as T, next)) {
        this.#value = next;
        this.#initialised = true;
        this.version++;
      }
      this.#stale = false;
    } finally {
      this.#computing = false;
    }
  }

  get value(): T {
    this.refresh();
    track(this);
    return this.#value as T;
  }

  peek(): T {
    this.refresh();
    return this.#value as T;
  }
}

class EffectNode implements Observer {
  sources = new Map<Source, number>();
  next = new Map<Source, number>();
  readonly #fn: () => void | (() => void);
  #cleanup: (() => void) | undefined;
  #disposed = false;

  constructor(fn: () => void | (() => void)) {
    this.#fn = fn;
    this.#run();
  }

  markStale(): void {
    if (!this.#disposed) pending.add(this);
  }

  update(): void {
    if (!this.#disposed && changed(this.sources)) this.#run();
  }

  #run(): void {
    this.#cleanup?.();
    this.#cleanup = undefined;
    const cleanup = runTracked(this, this.#fn);
    if (typeof cleanup === "function") this.#cleanup = cleanup;
  }

  dispose(): void {
    if (this.#disposed) return;
    this.#disposed = true;
    pending.delete(this);
    for (const source of this.sources.keys()) source.observers.delete(this);
    this.sources.clear();
    this.#cleanup?.();
    this.#cleanup = undefined;
  }
}

function flush(): void {
  for (let round = 0; pending.size > 0; round++) {
    if (round >= FLUSH_LIMIT) {
      pending.clear();
      throw new Error(`effects did not settle within ${FLUSH_LIMIT} rounds; an effect probably writes a signal it reads`);
    }
    const effects = [...pending];
    pending.clear();
    batchDepth++;
    try {
      for (const node of effects) node.update();
    } finally {
      batchDepth--;
    }
  }
}

export function signal<T>(value: T, equals: Equals<T> = Object.is): Signal<T> {
  return new SignalNode(value, equals);
}

export function computed<T>(fn: () => T, equals: Equals<T> = Object.is): ReadonlySignal<T> {
  return new ComputedNode(fn, equals);
}

/** Runs `fn` now and again whenever what it read changes; `fn` may return a cleanup. Returns a disposer. */
export function effect(fn: () => void | (() => void)): () => void {
  const node = new EffectNode(fn);
  return () => node.dispose();
}

/** Writes inside `fn` notify effects once, after it returns. */
export function batch<T>(fn: () => T): T {
  batchDepth++;
  try {
    return fn();
  } finally {
    batchDepth--;
    if (batchDepth === 0) flush();
  }
}

/** Reads inside `fn` do not subscribe the running computed or effect. */
export function untracked<T>(fn: () => T): T {
  const previous = active;
  active = undefined;
  try {
    return fn();
  } finally {
    active = previous;
  }
}
