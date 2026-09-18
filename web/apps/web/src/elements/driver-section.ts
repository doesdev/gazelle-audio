// The Devices page's Driver section: the audio driver's buffer size, latency and Safe Mode for this
// device, as the driver on the server's PC reports them, and a menu and a switch to change the buffer
// size and Safe Mode. Each change asks for a confirming click, as the clock does, because a program
// using the driver (a DAW) restarts its audio; afterwards the section shows what the driver reports.

import { h } from "../core/dom.ts";
import { asioInUseText, driverControls, driverView, writeText } from "../store/driver.ts";
import type { Store } from "../store/store.ts";
import { bindConfirm, confirmedChoice } from "./controls.ts";

/** Each row's key for the explain mode. */
const ROW_KEYS: Record<string, string> = {
  driver_version: "devices.driver-version",
  sample_rate: "devices.driver-rate",
  buffer_size: "devices.driver-buffer",
  input_latency: "devices.driver-latency",
  output_latency: "devices.driver-latency",
  safe_mode: "devices.driver-safe-mode",
};

const RESTARTS = "a program using the driver (a DAW) restarts its audio";

/** Builds the section and keeps it current through the page's own `watch`; `onDisconnect` forgets an armed confirm. */
export function driverSection(store: Store, deviceId: string, watch: (fn: () => void) => void, onDisconnect: (fn: () => void) => void): HTMLElement {
  const fields = h("dl", { class: "fields", "data-testid": "driver-fields" });
  const message = h("p", { class: "muted", "data-testid": "driver-message" }, "Reading the driver...");
  const readAt = h("span", { "data-testid": "driver-read-at" });
  const again = h(
    "button",
    { type: "button", "data-testid": "driver-refresh", "data-explain": "devices.driver-refresh", title: "Ask the driver again", "on:click": () => void store.loadDriver(deviceId, true) },
    "Read again",
  );

  // The buffer size: a menu of the sizes the driver offers, sent from a Confirm beside it. Off the
  // wheel, since each step would restart the audio of any program using the driver.
  const bufferMenu = h("select", { "aria-label": "Buffer size", "data-testid": "driver-buffer-menu", "data-no-wheel": true, "data-explain": "devices.driver-buffer-menu" });
  const bufferChoice = confirmedChoice(
    bufferMenu,
    "driver-buffer-confirm",
    "devices.driver-buffer-confirm",
    (size) => `Change the driver's buffer to ${size} samples: ${RESTARTS}`,
    (size) => void store.setDriver(deviceId, { buffer_size: size }),
  );
  onDisconnect(bufferChoice.disarm);

  // Safe Mode: a switch that reads On or Off and takes a confirming second click either way.
  let safeMode = false;
  const safeSwitch = h("button", {
    type: "button",
    class: "driver-safe",
    "aria-label": "Safe Mode",
    "aria-pressed": "false",
    "data-testid": "driver-safe-mode-switch",
    "data-explain": "devices.driver-safe-mode-switch",
  });
  onDisconnect(
    bindConfirm(safeSwitch, () => (safeMode ? "On" : "Off"), () => void store.setDriver(deviceId, { safe_mode: !safeMode })),
  );

  // After a refusal because a program is using ASIO: the same change again, forced.
  const force = h("button", { type: "button", class: "driver-force", "data-testid": "driver-force", "data-explain": "devices.driver-force", hidden: true }, "Change anyway");
  onDisconnect(
    bindConfirm(force, "Change anyway", () => {
      const write = store.driverWrite(deviceId).peek();
      if (write?.state === "refused") void store.setDriver(deviceId, { ...write.change, force: true });
    }),
  );

  const inUse = h("p", { class: "note-inline warning", "data-testid": "driver-in-use", hidden: true });
  const result = h("p", { class: "note-inline", "data-testid": "driver-result", "data-explain": "devices.driver-result", "aria-live": "polite", hidden: true });
  const note = h(
    "p",
    { class: "note-inline", "data-testid": "driver-note" },
    `These are the audio driver's settings on this PC, not the device's. A change to the buffer size or Safe Mode takes a confirming click, and ${RESTARTS}.`,
  );
  const section = h("ga-section", { heading: "Driver", explain: "devices.driver" }, message, fields, inUse, h("p", { class: "note-inline" }, result, " ", force), note, h("p", { class: "note-inline" }, readAt, " ", again));

  watch(() => {
    const report = store.driver(deviceId).value;
    const write = store.driverWrite(deviceId).value;
    if (report === undefined) return;
    const view = driverView(report);
    const controls = driverControls(report);
    const sending = write?.state === "sending";
    message.hidden = view.message === undefined;
    message.textContent = view.message ?? "";

    if (controls !== undefined) {
      const sizes = controls.sizes.map(String);
      if ([...bufferMenu.options].map((option) => option.value).join() !== sizes.join()) {
        bufferMenu.replaceChildren(...controls.sizes.map((size) => h("option", { value: String(size) }, `${size} samples`)));
      }
      bufferChoice.current = controls.buffer;
      if (!bufferChoice.armed()) bufferMenu.value = String(controls.buffer);
      safeMode = controls.safeMode;
      safeSwitch.setAttribute("aria-pressed", String(safeMode));
      if (!safeSwitch.hasAttribute("data-armed")) safeSwitch.textContent = safeMode ? "On" : "Off";
    }
    bufferMenu.disabled = sending;
    safeSwitch.toggleAttribute("disabled", sending);
    force.toggleAttribute("disabled", sending);

    fields.hidden = view.rows.length === 0;
    fields.replaceChildren(
      ...view.rows.flatMap((row) => {
        let value: Node;
        if (controls !== undefined && row.field === "buffer_size") value = h("span", { class: "choice" }, bufferMenu, bufferChoice.confirm);
        else if (controls !== undefined && row.field === "safe_mode") value = safeSwitch;
        else value = h("span", { class: row.unread ? "muted" : "readout", "data-testid": `driver-${row.field}`, "data-explain": ROW_KEYS[row.field] }, row.value);
        return [h("dt", {}, row.label), h("dd", {}, value)];
      }),
    );

    const using = controls === undefined ? undefined : asioInUseText(controls.asioClients);
    inUse.hidden = using === undefined;
    inUse.textContent = using ?? "";

    const shown = write === undefined ? undefined : writeText(write);
    result.hidden = shown === undefined;
    result.textContent = shown?.text ?? "";
    result.classList.toggle("warning", shown?.problem === true);
    force.hidden = !(write?.state === "refused" && write.code === "asio_in_use");

    readAt.textContent = report.state === "no_driver" ? "" : `Read at ${new Date(report.read_at_ms).toLocaleTimeString()}.`;
    again.hidden = report.state === "no_driver";
  });
  void store.loadDriver(deviceId);
  return section;
}
