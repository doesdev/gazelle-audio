// What the app calls the devices' inputs and outputs. The devices spell their routing groups in
// capitals ("PREAMP", "USB 1 PLAY", "AFX OUT"); Gazelle shows them in plain case with the acronyms
// kept ("Preamp", "USB 1 Play", "AFX Out"). Only what is shown changes: the topology, the wire and
// the workspace keep the devices' own names. The server names the aggregate's channels and words
// its messages from the same list, and both sides are held to one file of cases
// (refs/fixtures/display_names.json), so the two agree.

import type { TopologyGroup } from "gazelle-audio-client";

/**
 * The devices' words that read better in plain case, by their capitals. A word not here is kept as
 * the device spells it: the acronyms (USB, ADAT, AFX, TB, COM, HP1, the USB groups' A and B) and L/R.
 */
export const DISPLAY_WORDS: Readonly<Record<string, string>> = {
  PREAMP: "Preamp",
  LINE: "Line",
  IN: "In",
  OUT: "Out",
  PLAY: "Play",
  REC: "Rec",
  SPDIF: "S/PDIF",
  MONITOR: "Monitor",
  REAMP: "Reamp",
  MUTE: "Mute",
  OSCILLATOR: "Oscillator",
  LOOPBACK: "Loopback",
  MIX: "Mix",
  MIXER: "Mixer",
  CH: "Ch",
  EMU: "Emu",
  MIC: "Mic",
};

/**
 * A device's name for a group, or a channel of one, as Gazelle shows it: each word from
 * `DISPLAY_WORDS`, a number on the end kept ("MIX3" is "Mix3"), anything else as it was.
 * "USB 1 PLAY 3" is "USB 1 Play 3", "SPDIF IN" is "S/PDIF In", "HP1" stays "HP1".
 */
export function properCase(name: string): string {
  return name.replace(/\b[A-Z]+(?=\d*\b)/g, (word) => DISPLAY_WORDS[word] ?? word);
}

/** What a topology group is called on screen: its name in plain case (`properCase`). */
export function groupName(group: Pick<TopologyGroup, "name">): string {
  return properCase(group.name);
}
