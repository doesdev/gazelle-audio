// Registers every element the app loads with it. Import once, before adding <ga-app> to the page.
//
// These are what the shell shows at once: the header, the sidebar's device list, meter and Control
// Room, the mixer and its dock, and the notices. A page built by one route only is not here: it
// comes with its route, from `lazy.ts`.

import { GaApp } from "./app.ts";
import { GaChannel } from "./channel.ts";
import { GaChannelGroup } from "./channel-group.ts";
import { GaMixMaster } from "./mix-master.ts";
import { GaDeviceList } from "./device-list.ts";
import { GaHeader } from "./header.ts";
import { GaControlRoom, GaMonitor } from "./control-room.ts";
import { GaOutputMeters } from "./output-meters.ts";
import { GaMixer } from "./mixer-page.ts";
import { GaMixerDock } from "./mixer-dock.ts";
import { GaStrip } from "./strip.ts";
import { GaNotices } from "./notices.ts";
import { GaSection } from "./section.ts";

import { installSelectWheel } from "./select-wheel.ts";

export { provideStore } from "./element.ts";

const elements: [string, CustomElementConstructor][] = [
  ["ga-section", GaSection],
  ["ga-header", GaHeader],
  ["ga-device-list", GaDeviceList],
  ["ga-control-room", GaControlRoom],
  ["ga-output-meters", GaOutputMeters],
  ["ga-monitor", GaMonitor],
  ["ga-strip", GaStrip],
  ["ga-channel", GaChannel],
  ["ga-channel-group", GaChannelGroup],
  ["ga-mix-master", GaMixMaster],
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
