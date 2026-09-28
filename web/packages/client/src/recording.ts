// The recorder, as `/api/v1/recording` answers and the socket's `recording` frames carry it.

/**
 * Where the recorder is. `arming` and `disarming` last as long as the drivers take to open and close;
 * `counting_in` is Record pressed with a count-in, before the take starts on the downbeat after it.
 */
export type RecordingState = "off" | "arming" | "armed" | "counting_in" | "recording" | "disarming";

/** One recorded channel, named as the aggregate names it, and how loud it is. */
export interface RecordingChannelLevel {
  /** Its name in the aggregate, which a DAW shows and its file is called: "Vocal mic (Quadro 1)". */
  name: string;
  /** The interface it is on. */
  device: string;
  device_index: number;
  /** Its own number on that interface, from zero. */
  channel: number;
  /** The loudest sample since the last reading, in dBFS; absent for silence. */
  peak_dbfs?: number;
}

/** The pre-roll Arm reserved. */
export interface RecordingPreroll {
  /** The free memory it was sized from. */
  available_bytes: number;
  percent_asked: number;
  /** What it actually takes, as a percentage of the free memory. */
  percent: number;
  bytes: number;
  capacity_frames: number;
  preroll_frames: number;
  /** The most pre-roll it holds, in seconds. */
  preroll_seconds: number;
  /** Why it is not what the percentage alone would make it. */
  clamped?: string;
  /** How much is held now: filling after Arm and after each take; while recording, what the take started with. */
  held_seconds: number;
}

/** The take being recorded. */
export interface RecordingTakeLive {
  /** Since Record took effect. */
  elapsed_seconds: number;
  /** The pre-roll it started with. */
  preroll_seconds: number;
  number?: number;
  folder?: string;
  files: string[];
}

export interface RecordingDropouts {
  device: string;
  dropped: number;
  starved: number;
}

/** Everything the Recording page and the Remote page's transport show. */
export interface RecordingStatus {
  state: RecordingState;
  /** The preset it is armed with. */
  preset?: { id: string; name: string; folder: string; format: string };
  rate?: number;
  buffer_size?: number;
  channels: RecordingChannelLevel[];
  preroll?: RecordingPreroll;
  take?: RecordingTakeLive;
  /** Blocks lost since Arm because the writer fell behind; each is silence in its take. */
  overruns: number;
  /** Blocks the aggregate lost since Arm, every interface together. */
  dropouts: number;
  dropouts_by_device: RecordingDropouts[];
  disk_free_bytes?: number;
  disk_seconds_left?: number;
  disk_low: boolean;
  /** A driver asked to be reset while armed; the recorder does not do that under a take. */
  reset_asked: boolean;
  problem?: string;
  /** The preset last armed with, which is offered first. */
  last_preset?: string | null;
  /** Where files go when a preset names no folder. */
  default_folder?: string;
  /** Recording from the loopback's test tones, into the temporary folder. */
  loopback?: boolean;
  /** What auto-arm is doing; older servers omit it. */
  auto_arm?: RecordingAutoArm;
  /** The metronome; older servers omit it. */
  metronome?: MetronomeStatus;
}

export type MetronomeSound = "click" | "beep" | "woodblock" | "cowbell" | "tick";
export type MetronomeSubdivision = "none" | "eighths" | "triplets" | "sixteenths";

/** `/api/v1/metronome/settings`: kept on the computer, in `metronome.json`. */
export interface MetronomeSettings {
  /** Quarter notes a minute, 20 to 400, in steps of 0.1. */
  tempo: number;
  numerator: number;
  /** 2, 4, 8 or 16: the note a beat is, and a click. */
  denominator: number;
  accent: boolean;
  subdivision: MetronomeSubdivision;
  sound: MetronomeSound;
  /** The loudest click's peak, in dBFS; never louder than -6 whatever it says. */
  volume_db: number;
  /** Interface and that interface's own output, from zero. */
  outputs: { device: number; channel: number }[];
  /** Bars counted before a take: 0 to 4. */
  count_in_bars: number;
  /** The click runs whenever a take does. */
  follow_record: boolean;
}

/** `/api/v1/metronome`, and the recorder's state's `metronome`. */
export interface MetronomeStatus {
  running: boolean;
  /** The interfaces are open, by the recorder or the metronome. */
  open: boolean;
  started_by?: "hand" | "count_in" | "follow" | "preview";
  rate?: number;
  /** The beat last played, from one, in its bar, from one. */
  beat: number;
  bar: number;
  beats_per_bar: number;
  beat_seconds: number;
  /** How long ago the last beat was, as of `at_ms`. */
  since_beat_seconds?: number;
  at_ms?: number;
  /** A count-in under way; `bar` is 0 while it waits for the next downbeat. */
  count_in?: { bars: number; bar: number };
  /** The outputs it plays to, while the interfaces are open, named as the aggregate names them. */
  outputs: RecordingChannelLevel[];
  outputs_problem?: string;
  settings: MetronomeSettings;
}

/**
 * Where auto-arm is. `paused`: disarmed by hand, until Gazelle next starts or it is armed by hand.
 * `backing_off`: the last try was refused (`reason`), and the next is at `retry_at_ms`.
 */
export type RecordingAutoArmPhase = "off" | "armed" | "arming" | "paused" | "waiting_for_interfaces" | "waiting_for_measurement" | "backing_off";

/** Auto-arm, as the recorder's state carries it. */
export interface RecordingAutoArm {
  on: boolean;
  /** The preset it arms with, by id; kept while it is off. */
  preset: string | null;
  preset_name: string | null;
  phase: RecordingAutoArmPhase;
  /** Why the last try was refused. */
  reason: string | null;
  failures: number;
  /** When it tries next, in milliseconds since the Unix epoch. */
  retry_at_ms: number | null;
  /** The interfaces went away while it was armed, and it is waiting for them. */
  lost: boolean;
}

/** `/api/v1/recording/settings`: kept on the computer, in `recording.json`. */
export interface RecordingSettings {
  auto_arm: boolean;
  auto_arm_preset: string | null;
  start_in_hub: boolean;
}

/** `/api/v1/window/widget` and `/api/v1/window/hub`: the recording widget and hub windows. */
export interface RecordingWindows {
  /** This Gazelle has windows at all; `reason` says why not. */
  available: boolean;
  widget: boolean;
  hub: boolean;
  hub_full_screen: boolean;
  reason?: string;
}

/** A take that has been written. */
export interface RecordingTake {
  number: number;
  preset: string;
  folder: string;
  files: string[];
  log: string;
  date: string;
  time: string;
  seconds: number;
  preroll_seconds: number;
  overruns: number;
  dropouts: number;
  /** After a count-in: the downbeat, in seconds into the take, which every file marks with a cue. */
  downbeat_seconds?: number;
  /** Why Gazelle ended it, when a person did not. */
  stopped_by?: string;
  problem?: string;
}
