// The metronome, as the Recording page, the hub, the widget and the phone show it. Its state rides on
// the recorder's (`status.metronome`), so it arrives with the same `recording` frames and needs no
// socket of its own; what is here is the arithmetic the pages share: tap tempo, where the beat is
// between frames, and the words.
//
// The beat light is worked out in the page, from the last frame's beat and how long ago it was, and
// the frame's clock: the server sends five frames a second while the click runs, and each one puts
// the light right again, so it never drifts from what is heard by more than the network's jitter.

import type { MetronomeSettings, MetronomeSound, MetronomeStatus, MetronomeSubdivision } from "gazelle-audio-client";

export type { MetronomeSettings, MetronomeStatus };

export const TEMPO_MIN = 20;
export const TEMPO_MAX = 400;
/** The volume's range, in dBFS. The server holds it at -6 whatever it is sent. */
export const VOLUME_MIN = -60;
export const VOLUME_MAX = -6;
export const VOLUME_DEFAULT = -18;
export const COUNT_IN_MAX = 4;
export const DENOMINATORS = [2, 4, 8, 16] as const;
export const NUMERATOR_MAX = 16;

export const SOUNDS: readonly (readonly [MetronomeSound, string])[] = [
  ["click", "Click"],
  ["beep", "Beep"],
  ["woodblock", "Woodblock"],
  ["cowbell", "Cowbell"],
  ["tick", "Tick"],
];

export const SUBDIVISIONS: readonly (readonly [MetronomeSubdivision, string])[] = [
  ["none", "None"],
  ["eighths", "2 a beat (eighths)"],
  ["triplets", "3 a beat (triplets)"],
  ["sixteenths", "4 a beat (sixteenths)"],
];

/** Remembered in this browser once the first Start has been explained. */
export const METRONOME_EXPLAINED_KEY = "gazelle.metronome.explained";

/** What running the metronome means for the drivers, said wherever it can be started. */
export const METRONOME_HOLDS =
  "Running the metronome opens the interfaces through Gazelle Aggregate and holds their audio drivers, as arming does, even with nothing armed. A DAW may not be able to use them until you stop it (and disarm).";

/** Where the click goes, and where it does not. */
export const METRONOME_NOT_IN_TAKES = "It plays only to the outputs chosen here. It is never in a take unless you route those outputs back into an input you record.";

/** A tempo as the server keeps it: 20 to 400, in steps of 0.1. */
export function clampTempo(tempo: number): number {
  if (!Number.isFinite(tempo)) return 120;
  return Math.min(TEMPO_MAX, Math.max(TEMPO_MIN, Math.round(tempo * 10) / 10));
}

/** A tempo in words: "120", "97.5". */
export function tempoText(tempo: number): string {
  return Number.isInteger(tempo) ? String(tempo) : tempo.toFixed(1);
}

/** How long a pause starts the taps again, in ms. */
export const TAP_RESET_MS = 2_000;
/** How many of the last intervals are averaged. */
export const TAP_AVERAGE = 4;

/**
 * **Tap tempo**: each tap gives the tempo from the last few taps' average spacing, once there are two.
 * A pause of two seconds starts again. The taps count beats, and the tempo counts quarter notes, so
 * a beat of another length (the denominator) is turned into quarters.
 */
export class TapTempo {
  #taps: number[] = [];

  /** A tap at `now` (ms); the tempo it gives, or undefined for the first. */
  tap(now: number, denominator = 4): number | undefined {
    const last = this.#taps.at(-1);
    if (last === undefined || now - last > TAP_RESET_MS || now <= last) this.#taps = [];
    this.#taps.push(now);
    if (this.#taps.length > TAP_AVERAGE + 1) this.#taps.shift();
    if (this.#taps.length < 2) return undefined;
    const spacing = (now - (this.#taps[0] as number)) / (this.#taps.length - 1);
    return clampTempo((60_000 / spacing) * (4 / denominator));
  }

  get count(): number {
    return this.#taps.length;
  }
}

/** Where the beat is at `now` (ms since the epoch): which beat of the bar, from one, and how far into it, 0 to 1. */
export function beatAt(status: MetronomeStatus | undefined, now: number): { beat: number; into: number } | undefined {
  if (status === undefined || !status.running || status.since_beat_seconds === undefined || status.at_ms === undefined || !(status.beat_seconds > 0)) return undefined;
  const elapsed = status.since_beat_seconds + Math.max(0, now - status.at_ms) / 1000;
  const beats = Math.floor(elapsed / status.beat_seconds);
  const perBar = Math.max(1, status.beats_per_bar);
  return { beat: ((status.beat - 1 + beats) % perBar) + 1, into: elapsed / status.beat_seconds - beats };
}

/** The signature and tempo in a few words: "120 BPM, 4/4". */
export function tempoLine(settings: MetronomeSettings | undefined): string {
  if (settings === undefined) return "";
  return `${tempoText(settings.tempo)} BPM, ${settings.numerator}/${settings.denominator}`;
}

/** What the metronome is doing, in a few words. */
export function metronomeState(status: MetronomeStatus | undefined): string {
  if (status === undefined) return "Stopped";
  if (status.count_in !== undefined) return status.count_in.bar === 0 ? `Count-in of ${status.count_in.bars} from the next bar` : `Count-in, bar ${status.count_in.bar} of ${status.count_in.bars}`;
  if (!status.running) return "Stopped";
  if (status.started_by === "preview") return "Previewing one bar";
  return status.started_by === "follow" ? "Playing with the take" : status.started_by === "count_in" ? "Playing for the take" : "Playing";
}

/** Names joined for a sentence: "A", "A and B", "A, B and C". */
export function listText(names: readonly string[]): string {
  if (names.length <= 1) return names[0] ?? "";
  return `${names.slice(0, -1).join(", ")} and ${names.at(-1) as string}`;
}

/** The volume in words: "-18 dBFS". */
export function volumeText(db: number): string {
  return `${Math.round(Math.min(VOLUME_MAX, db))} dBFS`;
}
