// Registers every element. Import once, before adding <ga-app> to the page.

import { GaApp } from "./app.ts";
import { GaChannel } from "./channel.ts";
import { GaChannelGroup } from "./channel-group.ts";
import { GaMixMaster } from "./mix-master.ts";
import { GaRouting } from "./routing-page.ts";
import { GaDeviceList } from "./device-list.ts";
import { GaDeviceStatus } from "./device-status.ts";
import { GaEffects } from "./effects-page.ts";
import { GaHeader } from "./header.ts";
import { GaInputs } from "./inputs-page.ts";
import { GaOutputs } from "./outputs-page.ts";
import { GaControlRoom, GaMonitor } from "./control-room.ts";
import { GaOutputMeters } from "./output-meters.ts";
import { GaMixer } from "./mixer-page.ts";
import { GaMixerDock } from "./mixer-dock.ts";
import { GaStrip } from "./strip.ts";
import { GaNotices } from "./notices.ts";
import { GaSection } from "./section.ts";
import { GaWorkspace } from "./workspace.ts";

import { installSelectWheel } from "./select-wheel.ts";

export { provideStore } from "./element.ts";

const elements: [string, CustomElementConstructor][] = [
  ["ga-section", GaSection],
  ["ga-header", GaHeader],
  ["ga-device-list", GaDeviceList],
  ["ga-device-status", GaDeviceStatus],
  ["ga-workspace", GaWorkspace],
  ["ga-inputs", GaInputs],
  ["ga-outputs", GaOutputs],
  ["ga-control-room", GaControlRoom],
  ["ga-output-meters", GaOutputMeters],
  ["ga-monitor", GaMonitor],
  ["ga-strip", GaStrip],
  ["ga-channel", GaChannel],
  ["ga-channel-group", GaChannelGroup],
  ["ga-mix-master", GaMixMaster],
  ["ga-routing", GaRouting],
  ["ga-effects", GaEffects],
  ["ga-mixer", GaMixer],
  ["ga-mixer-dock", GaMixerDock],
  ["ga-notices", GaNotices],
  ["ga-app", GaApp],
];

for (const [name, element] of elements) {
  if (customElements.get(name) === undefined) customElements.define(name, element);
}

// The wheel steps any select in the app, from one listener rather than each page's own.
installSelectWheel();
