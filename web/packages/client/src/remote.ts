// Phones on the network, as `GET /api/v1/remote` and its routes answer
// (crates/gazelle-audio-server/src/remote). Every route but pairing answers only a caller on the
// server's own machine; a phone gets `not_local` (403). Keys stay snake_case as on the wire.

import { GazelleError } from "./errors.ts";
import { isObject } from "./bytes.ts";
import { API_PATH } from "./client.ts";

/** An address a phone could use. `primary` is the adapter the PC routes the internet through. */
export interface RemoteAddress {
  ip: string;
  primary: boolean;
}

/** A paired phone. Times are milliseconds since the Unix epoch. */
export interface RemotePhone {
  id: string;
  name: string;
  paired_ms: number;
  last_seen_ms: number | null;
  /** The address it was last seen from. */
  last_address: string | null;
}

/** A QR code as rows of modules, `1` dark and `0` light. */
export interface RemoteQr {
  size: number;
  rows: string[];
}

/** The pairing running now. */
export interface RemotePairing {
  /** `XXXX-XXXX`. */
  code: string;
  expires_ms: number;
  /** `http://<address>:<port>/pair#code=<code>`, the likeliest address first. */
  pair_urls: string[];
  /** The first pairing address as a QR code; null when no address was found. */
  qr: RemoteQr | null;
}

export interface RemoteStatus {
  allow_phones: boolean;
  /** The `--bind` address, when it decides who can reach the server rather than the setting. */
  fixed_by_bind: string | null;
  /** Whether a phone can reach the server now. */
  listening: boolean;
  /** Why the server could not listen for phones, although they are allowed. */
  error: string | null;
  port: number;
  addresses: RemoteAddress[];
  /** `http://<address>:<port>/` for each address. */
  urls: string[];
  phones: RemotePhone[];
  pairing: RemotePairing | null;
  /** The server's clock, for counting down to `pairing.expires_ms`. */
  now_ms: number;
}

/** What pairing answers: the token (also set as an HttpOnly cookie) and the phone as listed. */
export interface RemotePaired {
  token: string;
  phone: RemotePhone;
}

export type PairFetch = (url: string, init: { method: string; headers: Record<string, string>; body: string; credentials: "same-origin" }) => Promise<{ ok: boolean; status: number; json(): Promise<unknown> }>;

/**
 * Exchange a pairing code for a token, from the phone. Needs no connection first: until it is
 * paired, the phone may ask nothing else. In a browser the answer also sets the cookie that every
 * later request on the same origin carries; a native app keeps `token` and sends it as
 * `Authorization: Bearer`. Rejects with the server's code: `pairing_refused` (a wrong, used or
 * expired code), `too_many_attempts`, `too_many_phones` or `remote_off`.
 */
export async function pairPhone(baseUrl: string, code: string, name: string, fetch: PairFetch = (url, init) => globalThis.fetch(url, init)): Promise<RemotePaired> {
  const url = new URL(`${API_PATH}/remote/pair`, baseUrl).toString();
  let response: Awaited<ReturnType<PairFetch>>;
  try {
    response = await fetch(url, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ code, name }), credentials: "same-origin" });
  } catch (e) {
    throw new GazelleError("not_connected", `POST ${url} failed: ${String(e)}`);
  }
  let payload: unknown;
  try {
    payload = await response.json();
  } catch {
    payload = undefined;
  }
  if (!response.ok) {
    const error = isObject(payload) && isObject(payload["error"]) ? payload["error"] : {};
    const code = typeof error["code"] === "string" ? error["code"] : `http_${response.status}`;
    const message = typeof error["message"] === "string" ? error["message"] : `POST ${url} returned HTTP ${response.status}`;
    throw new GazelleError(code, message);
  }
  return payload as RemotePaired;
}
