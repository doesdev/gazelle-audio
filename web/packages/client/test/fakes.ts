// Test doubles: a WebSocket the test drives by hand and timers that only move when told to.

import type { SocketFactory, SocketLike, Timers } from "../src/index.ts";

export interface FakeSocket extends SocketLike {
  readonly url: string;
  /** Frames the client sent, parsed. */
  readonly sent: Record<string, unknown>[];
  closed: boolean;
  receive(frame: object): void;
  /** The server side goes away. */
  drop(): void;
}

export function fakeNetwork(): { sockets: FakeSocket[]; WebSocket: SocketFactory } {
  const sockets: FakeSocket[] = [];
  class Socket implements FakeSocket {
    readonly url: string;
    readonly sent: Record<string, unknown>[] = [];
    closed = false;
    onopen: SocketLike["onopen"] = null;
    onmessage: SocketLike["onmessage"] = null;
    onclose: SocketLike["onclose"] = null;
    onerror: SocketLike["onerror"] = null;

    constructor(url: string) {
      this.url = url;
      sockets.push(this);
    }

    send(data: string): void {
      if (this.closed) throw new Error("socket is closed");
      this.sent.push(JSON.parse(data) as Record<string, unknown>);
    }

    close(): void {
      this.closed = true;
    }

    receive(frame: object): void {
      this.onmessage?.({ data: JSON.stringify(frame) });
    }

    drop(): void {
      this.closed = true;
      this.onclose?.({});
    }
  }
  return { sockets, WebSocket: Socket };
}

export class ManualTimers implements Timers {
  now = 0;
  #next = 1;
  readonly tasks = new Map<number, { at: number; callback: () => void }>();

  setTimeout(callback: () => void, ms: number): number {
    const id = this.#next++;
    this.tasks.set(id, { at: this.now + ms, callback });
    return id;
  }

  clearTimeout(handle: unknown): void {
    if (typeof handle === "number") this.tasks.delete(handle);
  }

  /** Delays from now of every scheduled task, soonest first. */
  pending(): number[] {
    return [...this.tasks.values()].map((t) => t.at - this.now).sort((a, b) => a - b);
  }

  /** Runs every task due within `ms`, in order, then sets the clock to the end. */
  advance(ms: number): void {
    const end = this.now + ms;
    for (;;) {
      const due = [...this.tasks].filter(([, t]) => t.at <= end).sort(([ia, a], [ib, b]) => a.at - b.at || ia - ib)[0];
      if (due === undefined) break;
      this.tasks.delete(due[0]);
      this.now = due[1].at;
      due[1].callback();
    }
    this.now = end;
  }
}

/** Settles a promise into a value so a rejection is handled from the moment it is created. */
export function outcome<T>(promise: Promise<T>): Promise<{ ok: true; value: T } | { ok: false; error: unknown }> {
  return promise.then(
    (value) => ({ ok: true as const, value }),
    (error: unknown) => ({ ok: false as const, error }),
  );
}
