// The Workspace page's Phones section: allow phones on this network, pair one with a code and a QR
// code, and see and revoke the phones paired. The server answers all of it only to the computer
// Gazelle runs on (crates/gazelle-audio-server/src/remote), so on a phone the section just says
// where phones are managed.

import { h } from "../core/dom.ts";
import { ago, countdown, FIREWALL_NOTE, PLAIN_HTTP_NOTE, qrPath, reachText, type RemoteQr } from "../store/phones.ts";
import type { Store } from "../store/store.ts";
import { bindConfirm } from "./controls.ts";

export const PHONES_STYLES = `
  .phones { display: grid; gap: 8px; padding: 8px 10px; }
  .phones .switch[aria-pressed="true"] { border-color: var(--ga-accent); color: var(--ga-accent); }
  .phones .reach { margin: 0; overflow-wrap: anywhere; }
  .phones .reach[data-problem] { color: var(--ga-notice-warning, var(--ga-text-primary)); }
  .pairing { display: flex; flex-wrap: wrap; align-items: flex-start; gap: 12px 16px; padding: 8px; border: 1px solid var(--ga-border-strong); border-radius: 4px; background: var(--ga-surface-inset); }
  /* A QR code is read by a camera: dark on light, whatever the theme, with its quiet zone kept. */
  .qr { width: 196px; height: 196px; flex: none; background: #fff; border-radius: 2px; }
  .qr path { fill: #000; }
  .pairing-text { display: grid; gap: 4px; flex: 1 1 220px; min-width: 0; }
  .pairing-text p { margin: 0; }
  .pair-code { font-family: "Josefin Sans Variable", system-ui, sans-serif; font-size: 28px; letter-spacing: 0.12em; font-variant-numeric: tabular-nums; }
  .pair-url { font-family: var(--ga-font-mono, monospace); font-size: 11px; overflow-wrap: anywhere; user-select: all; }
  .phone-list { display: grid; gap: 0; margin: 0; padding: 0; list-style: none; }
  .phone { display: flex; flex-wrap: wrap; align-items: center; gap: 4px 10px; padding: 4px 0; border-top: 1px solid var(--ga-border-subtle); }
  .phone:first-child { border-top: 0; }
  .phone-name { flex: 0 1 180px; min-width: 100px; font-weight: 600; overflow-wrap: anywhere; }
  .phone-seen { flex: 1 1 220px; min-width: 0; font-size: 11px; color: var(--ga-text-secondary); }
`;

const SVG = "http://www.w3.org/2000/svg";

/** The QR code as an SVG the page can scale, dark modules on white. */
function qrSvg(qr: RemoteQr): SVGSVGElement {
  const { d, size } = qrPath(qr);
  const svg = document.createElementNS(SVG, "svg");
  svg.setAttribute("viewBox", `0 0 ${size} ${size}`);
  svg.setAttribute("class", "qr");
  svg.setAttribute("role", "img");
  svg.setAttribute("aria-label", "QR code of the pairing address");
  svg.setAttribute("shape-rendering", "crispEdges");
  svg.setAttribute("data-testid", "phones-qr");
  svg.setAttribute("data-explain", "workspace.phones-qr");
  const path = document.createElementNS(SVG, "path");
  path.setAttribute("d", d);
  svg.append(path);
  return svg;
}

/** Builds the section and keeps it current through the page's own `watch`. */
export function phonesSection(store: Store, watch: (fn: () => void) => void, onDisconnect: (fn: () => void) => void): HTMLElement {
  const phones = store.phones;
  onDisconnect(phones.follow());

  const allow = h("button", { type: "button", class: "switch", "aria-pressed": "false", "data-testid": "phones-allow", "data-explain": "workspace.phones-allow" });
  const allowLabel = () => `Allow phones on this network: ${allow.getAttribute("aria-pressed") === "true" ? "On" : "Off"}`;
  // Turning it on opens Gazelle to the network, so it asks for a second click; turning it off does
  // not, since off is the safe way round.
  const disarmAllow = bindConfirm(
    allow,
    allowLabel,
    () => void phones.setAllowPhones(allow.getAttribute("aria-pressed") !== "true"),
    () => allow.getAttribute("aria-pressed") !== "true",
  );
  onDisconnect(disarmAllow);

  const reach = h("p", { class: "note reach", role: "status", "data-testid": "phones-reach", "data-explain": "workspace.phones-reach" });
  const firewall = h("p", { class: "note", "data-testid": "phones-firewall" }, FIREWALL_NOTE);
  const plain = h("p", { class: "note" }, PLAIN_HTTP_NOTE);

  const pair = h("button", { type: "button", "data-testid": "phones-pair", "data-explain": "workspace.phones-pair", "on:click": () => void phones.startPairing() }, "Pair a phone");
  const cancel = h("button", { type: "button", hidden: true, "data-testid": "phones-cancel", "data-explain": "workspace.phones-cancel", "on:click": () => void phones.cancelPairing() }, "Stop pairing");

  const qrHolder = h("div");
  const code = h("p", { class: "pair-code readout", "data-testid": "phones-code", "data-explain": "workspace.phones-code" });
  const expires = h("p", { class: "note", role: "timer", "data-testid": "phones-expires" });
  const urls = h("div", { "data-testid": "phones-pair-urls" });
  const pairing = h(
    "div",
    { class: "pairing", hidden: true, "data-testid": "phones-pairing" },
    qrHolder,
    h(
      "div",
      { class: "pairing-text" },
      h("p", {}, "Scan the QR code with the phone's camera, or open the address below on the phone and enter this code:"),
      code,
      expires,
      urls,
      h("p", { class: "note" }, "The code works once. The phone is then asked for a name, and opens Gazelle."),
    ),
  );

  const justPaired = h("p", { class: "done", role: "status", hidden: true, "data-testid": "phones-just-paired" });
  const problem = h("p", { class: "problem", role: "alert", hidden: true, "data-testid": "phones-problem" });
  const list = h("ul", { class: "phone-list", "data-testid": "phones-list" });
  const none = h("p", { class: "note", "data-testid": "phones-none" }, "No phones are paired.");
  const elsewhere = h("p", { class: "note", hidden: true, "data-testid": "phones-elsewhere" }, "Phones are allowed, paired and revoked on the computer Gazelle runs on, from this section there.");

  const controls = h("div", { class: "phones-controls" }, h("div", { class: "actions" }, allow), reach, firewall, plain, h("div", { class: "actions" }, pair, cancel), pairing, justPaired, problem, list, none);
  const section = h("div", { class: "phones", "data-testid": "phones" }, controls, elsewhere);

  // The countdown runs on this page's clock, set against the server's at each answer.
  let offset = 0;
  let expiresMs: number | undefined;
  const tick = () => {
    if (expiresMs === undefined) return;
    const now = Date.now() + offset;
    expires.textContent = now >= expiresMs ? "This code has expired. Choose Pair a phone for a new one." : `The code expires in ${countdown(expiresMs, now)}.`;
  };
  const ticker = setInterval(tick, 1000);
  onDisconnect(() => clearInterval(ticker));

  // Revoke buttons are rebuilt with the list; each one's armed confirm goes with it.
  let disarms: (() => void)[] = [];
  onDisconnect(() => disarms.forEach((d) => d()));
  let shownCode: string | undefined;

  watch(() => {
    const state = phones.state.value;
    const busy = phones.busy.value;
    const connected = store.connected.value;
    controls.hidden = state.state === "elsewhere";
    elsewhere.hidden = state.state !== "elsewhere";
    if (state.state !== "ready") {
      reach.textContent = state.state === "failed" ? `Could not read the phones setting: ${state.message}` : state.state === "loading" ? "Reading..." : "";
      reach.toggleAttribute("data-problem", state.state === "failed");
      for (const button of [allow, pair]) button.disabled = true;
      pairing.hidden = true;
      cancel.hidden = true;
      return;
    }
    const status = state.status;
    offset = status.now_ms - Date.now();
    const on = status.allow_phones || status.fixed_by_bind !== null;
    allow.setAttribute("aria-pressed", String(on));
    if (!allow.hasAttribute("data-armed")) allow.textContent = allowLabel();
    allow.disabled = busy || !connected || status.fixed_by_bind !== null;
    const line = reachText(status);
    reach.textContent = line.text;
    reach.toggleAttribute("data-problem", line.problem);
    pair.disabled = busy || !connected || !status.listening;
    pair.textContent = status.pairing === null ? "Pair a phone" : "New code";

    const current = status.pairing;
    pairing.hidden = current === null;
    cancel.hidden = current === null;
    cancel.disabled = busy || !connected;
    expiresMs = current?.expires_ms;
    if (current !== null && current.code !== shownCode) {
      qrHolder.replaceChildren(current.qr === null ? h("p", { class: "note" }, "No network address was found for a QR code.") : qrSvg(current.qr));
      code.textContent = current.code;
      urls.replaceChildren(...current.pair_urls.map((url) => h("p", { class: "pair-url" }, url.replace(/#code=.*$/, ""))));
    }
    shownCode = current?.code;
    tick();

    disarms.forEach((d) => d());
    disarms = [];
    const now = status.now_ms;
    list.replaceChildren(
      ...status.phones.map((phone) => {
        const revoke = h("button", { type: "button", "aria-label": `Revoke ${phone.name}`, "data-testid": "phone-revoke", "data-explain": "workspace.phones-revoke" }, "Revoke");
        revoke.disabled = busy || !connected;
        disarms.push(bindConfirm(revoke, "Revoke", () => void phones.revoke(phone.id)));
        const seen = phone.last_seen_ms === null ? "not seen yet" : `last seen ${ago(phone.last_seen_ms, now)}${phone.last_address === null ? "" : ` from ${phone.last_address}`}`;
        return h(
          "li",
          { class: "phone", "data-testid": "phone", "data-phone-id": phone.id },
          h("span", { class: "phone-name" }, phone.name),
          h("span", { class: "phone-seen" }, `Paired ${new Date(phone.paired_ms).toLocaleDateString()}, ${seen}`),
          revoke,
        );
      }),
    );
    none.hidden = status.phones.length > 0;
  });
  watch(() => {
    const name = phones.justPaired.value;
    justPaired.hidden = name === undefined;
    justPaired.textContent = name === undefined ? "" : `Paired "${name}".`;
  });
  watch(() => {
    const text = phones.problem.value;
    problem.hidden = text === undefined;
    problem.textContent = text ?? "";
  });
  return section;
}
