// The Devices page's Driver section: the audio driver's buffer size, latency and Safe Mode for this
// device, as the driver on the server's PC reports them. Read only: there is nothing here to change
// them with, and the note says so.

import { h } from "../core/dom.ts";
import { driverView } from "../store/driver.ts";
import type { Store } from "../store/store.ts";

/** Builds the section and keeps it current through the page's own `watch`. */
export function driverSection(store: Store, deviceId: string, watch: (fn: () => void) => void): HTMLElement {
  const fields = h("dl", { class: "fields", "data-testid": "driver-fields" });
  const message = h("p", { class: "muted", "data-testid": "driver-message" }, "Reading the driver...");
  const readAt = h("span", { "data-testid": "driver-read-at" });
  const again = h(
    "button",
    { type: "button", "data-testid": "driver-refresh", title: "Ask the driver again", "on:click": () => void store.loadDriver(deviceId, true) },
    "Read again",
  );
  const section = h(
    "ga-section",
    { heading: "Driver" },
    message,
    fields,
    h("p", { class: "note-inline", "data-testid": "driver-note" }, "These are the audio driver's settings on this PC, not the device's. They are shown here only; changing them from Gazelle is a later step."),
    h("p", { class: "note-inline" }, readAt, " ", again),
  );

  watch(() => {
    const report = store.driver(deviceId).value;
    if (report === undefined) return;
    const view = driverView(report);
    message.hidden = view.message === undefined;
    message.textContent = view.message ?? "";
    fields.hidden = view.rows.length === 0;
    fields.replaceChildren(
      ...view.rows.flatMap((row) => [h("dt", {}, row.label), h("dd", {}, h("span", { class: row.unread ? "muted" : "readout", "data-testid": `driver-${row.field}` }, row.value))]),
    );
    readAt.textContent = report.state === "no_driver" ? "" : `Read at ${new Date(report.read_at_ms).toLocaleTimeString()}.`;
    again.hidden = report.state === "no_driver";
  });
  void store.loadDriver(deviceId);
  return section;
}
