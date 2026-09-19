// Starting layouts per device model: named sets of channels a mixer with no channel set up can
// start from, beside the default of one inactive channel (the user's request, 2026-09-16). They are
// drafts meant to be edited or replaced: each channel names an input by topology
// type and channel, a main mix and sends. ChannelsModel.applyProfile places them on the first free
// slots and routes them.

export interface ProfileChannel {
  name: string;
  /** Topology input group type, such as "PREAMP". */
  input: string;
  channel: number;
  main_mix: number;
  sends: readonly number[];
}

export interface LayoutProfile {
  id: string;
  name: string;
  description: string;
  /** Names for mixes 1, 2, …; later mixes keep their default names. */
  mixes: readonly string[];
  channels: readonly ProfileChannel[];
}

const channel = (name: string, input: string, index: number, main_mix: number, sends: readonly number[] = []): ProfileChannel => ({ name, input, channel: index, main_mix, sends });

const daw = (input: string, sends: readonly number[] = []) => [channel("DAW L", input, 0, 0, sends), channel("DAW R", input, 1, 0, sends)];

export const PROFILES: Readonly<Record<"quadro" | "studio", readonly LayoutProfile[]>> = {
  quadro: [
    {
      id: "tracking",
      name: "Tracking",
      description: "All four preamps and the DAW return, in the monitors and a performer's cue mix.",
      mixes: ["Monitors", "Cue"],
      channels: [0, 1, 2, 3].map((i) => channel(`Preamp ${i + 1}`, "PREAMP", i, 0, [1])).concat(daw("COM_PLAY", [1])),
    },
    {
      id: "podcast",
      name: "Podcast",
      description: "Two microphones and the DAW return; the guest hears their own mix on HP2.",
      mixes: ["Host", "Guest"],
      channels: [channel("Host", "PREAMP", 0, 0, [1]), channel("Guest", "PREAMP", 1, 0, [1]), ...daw("COM_PLAY", [1])],
    },
    {
      id: "playback",
      name: "Playback",
      description: "Just the DAW return in the monitors.",
      mixes: ["Monitors"],
      channels: daw("COM_PLAY"),
    },
  ],
  studio: [
    {
      id: "tracking",
      name: "Tracking",
      description: "Preamps 1 to 8 and the DAW return, in the monitors and a performer's cue mix.",
      mixes: ["Monitors", "Cue"],
      channels: [0, 1, 2, 3, 4, 5, 6, 7].map((i) => channel(`Preamp ${i + 1}`, "PREAMP", i, 0, [1])).concat(daw("USB_PLAY", [1])),
    },
    {
      id: "drums",
      name: "Drums",
      description: "A drum kit on preamps 1 to 8 with the DAW return, and a drummer's mix.",
      mixes: ["Main", "Drummer"],
      channels: ["Kick", "Snare", "Hat", "Tom 1", "Tom 2", "Floor", "OH L", "OH R"].map((name, i) => channel(name, "PREAMP", i, 0, [1])).concat(daw("USB_PLAY", [1])),
    },
    {
      id: "playback",
      name: "Playback",
      description: "Just the DAW return in the monitors.",
      mixes: ["Monitors"],
      channels: daw("USB_PLAY"),
    },
  ],
};
