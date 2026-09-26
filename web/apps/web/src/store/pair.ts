// The pair page's logic, apart from its DOM (elements/pair-page.ts) so it can be tested without a
// browser.
//
// A phone arrives at `/pair#code=XXXX-XXXX` by scanning the QR code on the computer, or at `/pair`
// by typing the address. The code travels in the fragment, which a browser never sends to a
// server, so it stays out of every log on the way. The page takes it, drops it from the address
// bar (and so from the history), asks for a name for the phone, and exchanges the code for a
// token. The server answers with an HttpOnly cookie that the phone's browser then sends with every
// request to the same address, which is how the app that follows just works.

import { GazelleError, pairPhone } from "gazelle-audio-client";

/** The code in a `#code=...` fragment, trimmed, or undefined when there is none. */
export function codeFromFragment(hash: string): string | undefined {
  const params = new URLSearchParams(hash.replace(/^#/, ""));
  const code = params.get("code")?.trim();
  return code === undefined || code === "" ? undefined : code;
}

/** A name for the phone from its browser's user agent, for the person to keep or change. */
export function defaultPhoneName(userAgent: string): string {
  if (/iPhone/.test(userAgent)) return "iPhone";
  if (/iPad/.test(userAgent)) return "iPad";
  const android = /Android[^;)]*;\s*([^;)]+?)(?:\s+Build\/[^;)]*)?\s*[;)]/.exec(userAgent);
  const model = android?.[1]?.trim();
  // Chrome now reports every Android model as "K", which names nothing.
  if (model !== undefined && model !== "" && model !== "K" && !/^wv$/i.test(model)) return model;
  if (/Android/.test(userAgent)) return "Android phone";
  return "Phone";
}

/** What to say when pairing did not work, by the server's code. */
export function pairFailureText(error: unknown): string {
  const code = error instanceof GazelleError ? error.code : undefined;
  switch (code) {
    case "pairing_refused":
      return "That code did not work. It may be mistyped, already used or expired (a code lasts 5 minutes). On the computer, choose Pair a phone again for a new one.";
    case "too_many_attempts":
      return "Too many wrong codes lately. Wait a minute, then try again.";
    case "too_many_phones":
      return "Gazelle already has as many paired phones as it keeps. Revoke one on the computer first.";
    case "remote_off":
      return "Gazelle is not allowing phones right now. Turn on Allow phones on this network on the computer.";
    case "not_connected":
      return "Gazelle could not be reached. Check that this phone is on the same network as the computer, and that Gazelle is running.";
    default:
      return `Pairing did not work: ${error instanceof Error ? error.message : String(error)}`;
  }
}

/** What the page says to a device that opened the app without being paired. */
export const UNPAIRED_TITLE = "This device is not paired with Gazelle";
export const UNPAIRED_TEXT =
  "Gazelle only answers devices it has paired with. On the computer running Gazelle, open the Workspace page, and under Phones choose Pair a phone. Then scan its QR code with this phone, or open the pairing address it shows.";

/** What the app says in its own place once the computer has revoked this phone. */
export const REVOKED_TITLE = "This phone is no longer paired with Gazelle";
export const REVOKED_TEXT = "Pair it again from the computer: Workspace, Phones.";

export interface PairOutcome {
  ok: boolean;
  message: string;
}

/**
 * Pair with `code` under `name`, using `pair` (the client's `pairPhone`, bound to this origin).
 * An empty code is refused here, without asking the server.
 */
export async function submitPair(code: string, name: string, pair: (code: string, name: string) => Promise<unknown>): Promise<PairOutcome> {
  const trimmed = code.trim();
  if (trimmed === "") return { ok: false, message: "Enter the code shown on the computer." };
  try {
    await pair(trimmed, name.trim());
    return { ok: true, message: "Paired. Opening Gazelle..." };
  } catch (error) {
    return { ok: false, message: pairFailureText(error) };
  }
}

/** Pair with the Gazelle that served this page. */
export function pairHere(origin: string): (code: string, name: string) => Promise<unknown> {
  return (code, name) => pairPhone(origin, code, name);
}
