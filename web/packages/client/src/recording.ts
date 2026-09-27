// The recorder, as `/api/v1/recording` answers and the socket's `recording` frames carry it.

/** Where the recorder is. `arming` and `disarming` last as long as the drivers take to open and close. */
export type RecordingState = "off" | "arming" | "armed" | "recording" | "disarming";

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
  /** Why Gazelle ended it, when a person did not. */
  stopped_by?: string;
  problem?: string;
}
