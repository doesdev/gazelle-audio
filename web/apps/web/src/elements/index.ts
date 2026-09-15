// Registers every element. Import once, before adding <ga-app> to the page.

import { GaApp } from "./app.ts";
import { GaDeviceList } from "./device-list.ts";
import { GaDeviceStatus } from "./device-status.ts";
import { GaHeader } from "./header.ts";
import { GaInputs } from "./inputs-page.ts";
import { GaMixer } from "./mixer-page.ts";
import { GaStrip } from "./strip.ts";
import { GaNotices } from "./notices.ts";
import { GaSection } from "./section.ts";
import { GaWorkspace } from "./workspace.ts";

export { provideStore } from "./element.ts";

const elements: [string, CustomElementConstructor][] = [
  ["ga-section", GaSection],
  ["ga-header", GaHeader],
  ["ga-device-list", GaDeviceList],
  ["ga-device-status", GaDeviceStatus],
  ["ga-workspace", GaWorkspace],
  ["ga-inputs", GaInputs],
  ["ga-strip", GaStrip],
  ["ga-mixer", GaMixer],
  ["ga-notices", GaNotices],
  ["ga-app", GaApp],
];

for (const [name, element] of elements) {
  if (customElements.get(name) === undefined) customElements.define(name, element);
}
