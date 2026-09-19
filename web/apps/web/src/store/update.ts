// What the header says about updating, worked out from the server's update status alone.
//
// The rule is that silence is the normal state. Up to date, checking, and a server that does not
// do updates at all say nothing: the header's line is already full of things that must always be
// readable (the backend, dry run, the connection), and an update notice that is on screen every
// day is one nobody reads on the day it matters. What does show is the three things a person can
// act on or is waiting for: a version ready to restart into, one on its way, and one that failed.

import type { UpdateStatus } from "gazelle-audio-client";

/**
 * What the header shows beside the version, or `undefined` for nothing at all.
 *
 * `act` names what a click does, and `undefined` means the prompt is a readout rather than a
 * button. `confirm` marks the one that interrupts the devices, so the element arms it first.
 */
export interface UpdatePrompt {
  kind: "ready" | "downloading" | "available" | "failed";
  /** What the button or readout says. Short: this line is shared with the safety badges. */
  label: string;
  /** The longer version, for the tooltip. */
  title: string;
  act: "restart" | "download" | "check" | undefined;
  confirm: boolean;
}

export function updatePrompt(status: UpdateStatus | undefined): UpdatePrompt | undefined {
  if (status === undefined) return undefined;
  const state = status.state;
  switch (state.state) {
    // Verified and sitting beside the running binary. The one thing left to ask for.
    case "staged":
      return {
        kind: "ready",
        label: `${state.version} ready, restart`,
        title: `Gazelle ${state.version} is ready. Restarting takes a few seconds, and the devices are let go and picked up again while it happens.`,
        act: "restart",
        confirm: true,
      };
    // The user asked for silence about updates and got none of it: they complained the app said
    // nothing while it worked. This is the line that fixes that.
    case "downloading":
      return { kind: "downloading", label: `${state.version} downloading`, title: `Gazelle ${state.version} is being downloaded and checked.`, act: undefined, confirm: false };
    // Only an install whose update.json says auto_download: false ever rests here.
    case "available":
      return { kind: "available", label: `Get ${state.version}`, title: `Gazelle ${state.version} has been released. This install does not download by itself.`, act: "download", confirm: false };
    // Never silent: a check or a download that failed is exactly what the user was not told about.
    case "failed":
      return { kind: "failed", label: "Update failed, try again", title: `${capitalise(state.message)}. ${state.detail}`, act: "check", confirm: false };
    case "unknown":
    case "checking":
    case "up_to_date":
      return undefined;
  }
}

const capitalise = (text: string): string => (text === "" ? text : text[0]?.toUpperCase() + text.slice(1));
