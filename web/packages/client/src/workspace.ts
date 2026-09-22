// The server's workspace document (crates/gazelle-audio-server/src/workspace/model.rs): layout
// state that spans devices, persisted server-side. Keys stay snake_case as on the wire.

/** One channel on one device. */
export interface ChannelRef {
  device_id: string;
  channel: number;
}

export interface Group {
  id: string;
  name: string;
  collapsed: boolean;
  hidden: boolean;
  /** `#rrggbb` when the group is colour-coded; the server omits it when unset. */
  color?: string;
  members: ChannelRef[];
  /** Nested sub-groups. */
  children: Group[];
}

/** What a link joins: preamps, a digital input kind, or mixer channels (by mixer input slot). */
export type LinkKind = "preamp" | "line" | "adat" | "spdif" | "mixer";

/**
 * Channels of one kind, on any devices, that change together. `absolute` members
 * take the same value; `relative` members keep their offsets.
 */
export interface Link {
  id: string;
  kind: LinkKind;
  mode: "absolute" | "relative";
  members: ChannelRef[];
}

/** A routing source: a group's position in the topology `inputs` and a channel in it. */
export interface RouteSource {
  group: number;
  channel: number;
}

/** One mixer channel the user made. It occupies one mixer input slot in every mix. */
export interface MixerChannel {
  id: string;
  name: string;
  /** A `MixerGroup` id. */
  group?: string;
  color?: string;
  /** Mixer input slot, 0..31: the strip in every mix. */
  slot: number;
  /** Unset until the user picks an input; the channel is inactive until then. */
  source?: RouteSource;
  /** The mix its fader controls; unset until chosen. */
  main_mix?: number;
  /** Other mixes it is also routed to. */
  sends: number[];
}

export interface MixerGroup {
  id: string;
  name: string;
  collapsed: boolean;
  color?: string;
}

export interface MixConfig {
  name?: string;
  /**
   * Present while the mix is summed to mono: its channels are centred, and these are the pans to
   * restore, by mixer input slot, with the mix master's level just before mono lowered it
   * (`master_level`, dB of attenuation). A workspace written before mono compensated the level has
   * no `master_level`, and one written now still loads without it.
   */
  mono?: { pans: Record<string, number>; master_level?: number };
}

/** A device's mixer as the user laid it out; the device keeps routing and levels. */
export interface DeviceMixer {
  /** Indexed by device mixer. */
  mixes: MixConfig[];
  groups: MixerGroup[];
  /** In display order. */
  channels: MixerChannel[];
}

export interface Workspace {
  /** Schema version; 1 today. */
  version: number;
  groups: Group[];
  links: Link[];
  /** Device id → the name a user gave it. */
  aliases: Record<string, string>;
  /** Device id → its mixer layout. */
  mixers: Record<string, DeviceMixer>;
  /** Mixer layouts the user saved, per device model; older servers omit it. */
  layouts?: SavedLayout[];
  /** Device id → its badge colour, `#rrggbb`; older servers omit it. */
  device_colors?: Record<string, string>;
  /** Cross-device mix surfaces; older servers omit it. */
  surfaces?: Surface[];
  /** Digital connections between devices, as declared; older servers omit it. */
  cables?: Cable[];
  /** Device id → what its Control Room panel shows; a device without one shows Monitor, HP1 and HP2. Older servers omit it. */
  control_room?: Record<string, ControlRoom>;
  /** The aggregate audio driver's setup, when there is one; older servers omit it. */
  aggregate?: Aggregate;
}

/**
 * The aggregate audio driver's setup: which interfaces it opens, in what order, which one drives
 * the callback and how their streams line up. It mirrors the file the driver reads, which Gazelle
 * exports from here whenever this changes, so the setup travels with a workspace backup.
 *
 * Everything is optional, as it is in that file: a section with no devices means the driver opens
 * every Antelope driver it finds. A field neither side knows is kept as it came.
 */
export interface Aggregate {
  /** The sub-devices, in the order their channels appear to a DAW. */
  devices?: AggregateDevice[];
  /**
   * Which device drives the callback. Gazelle writes the device's registry key (or class id), which
   * a rename cannot change, and the driver is given the device's name in its place. An older setup's
   * own name for a device is still understood. The first device when unset.
   */
  callback_master?: string;
  alignment?: "aligned" | "lowest_latency";
  /** The rate to put every device at, in Hz. */
  rate?: number;
  /** The buffer size to offer a DAW as preferred, in samples. */
  buffer_size?: number;
  [field: string]: unknown;
}

/** What the server last knew about the interface one aggregate entry turned out to be. */
export interface AggregateKnown {
  device_id?: string;
  family?: string;
  model?: string;
  /** What its routing sends to each of its USB record channels, as `[source group, channel]`. */
  record_routing?: [number, number][];
}

/** One interface the aggregate opens. It needs a `key` or a `clsid`. */
export interface AggregateDevice {
  /** The name the vendor driver registers itself under, matched without case, whole or as part. */
  key?: string;
  /** The vendor driver's class id, which is the sure way to name one. */
  clsid?: string;
  /**
   * What an older setup called this interface. Nothing is called by it any more: an interface is
   * called by Gazelle's name for its device. It is kept only so a `callback_master` written by it
   * still finds its device.
   */
  name?: string;
  /** Samples to add to this device's input latency. A device that records late takes a positive trim. */
  input_trim?: number;
  output_trim?: number;
  /** Which of its inputs to expose, by the device's own numbering from zero; all of them when unset. */
  inputs?: number[];
  outputs?: number[];
  /**
   * The names the person has typed for this device's inputs, by the device's own channel numbering
   * from zero, which is the numbering `inputs` uses. Only typed names are here: the name each
   * channel takes from Gazelle's routing is worked out by the server when it writes the driver's
   * file, and a typed name wins over it. A channel with no name here is left out rather than
   * written as an empty string.
   */
  input_names?: Record<string, string>;
  output_names?: Record<string, string>;
  /** Which Gazelle device this is, which is how its clock, rate and buffer are read. Not in the driver's file. */
  device_id?: string;
  /**
   * What Gazelle last knew about the interface this entry turned out to be. The server's alone: it
   * works it out again on every save and ignores what a client sends for it.
   */
  known?: AggregateKnown;
  /**
   * Where the driver measures this interface's capture phase at the start of every session, over
   * the digital cable from the callback master. Absent means not measured. The driver refuses it on
   * the callback master itself.
   */
  phase?: AggregatePhaseSetting;
  [field: string]: unknown;
}

/**
 * One interface's phase measurement path, and the phase it is lined up to.
 *
 * Both channels are the devices' own numbering from zero, the numbering `inputs` and `outputs` use.
 * `reference` is the phase measured in the session the input trim was measured in, and is written
 * with that trim and never on its own: it is not a trim, and it is not something to type in.
 */
export interface AggregatePhaseSetting {
  /** A channel of the callback master's own outputs, which the cable leaves from. */
  master_output: number;
  /** A channel of this interface's own inputs, which the cable arrives on. */
  input: number;
  /** The phase measured beside the trim, in samples. Absent until a measurement gives it one. */
  reference?: number;
  [field: string]: unknown;
}

/** What one device's Control Room panel shows. */
export interface ControlRoom {
  /** Output ids as `set_volume` numbers them; the panel shows them in the device's order. */
  outputs: number[];
}

/** A device's digital port, by its topology type. */
export type DigitalPort = "SPDIF_OUT" | "ADAT_OUT" | "SPDIF_IN" | "ADAT_IN";

/** One end of a cable: a device's port and the first channel of it the cable carries. */
export interface CableEnd {
  device_id: string;
  port: DigitalPort;
  first: number;
}

/**
 * A cable the user says joins one device's S/PDIF or ADAT output to another's input of the same
 * kind. It routes nothing; it tells the app where a digital input's signal comes from.
 */
export interface Cable {
  id: string;
  from: CableEnd;
  to: CableEnd;
  channels: number;
}

/** What a surface strip shows. */
export type SurfaceStripKind = "channel" | "master" | "input" | "output" | "port" | "label";

/** A hardware input: its kind and a channel within it, from 0. */
export interface InputRef {
  kind: "preamp" | "line" | "adat" | "spdif";
  channel: number;
}

/**
 * One strip on a surface. A `channel` names a mixer channel of its device's layout and may pin a
 * `mix`; a `master` names a mix, or follows the surface's mix for its device without one; an `input`
 * a hardware input; an `output` an output id as `set_volume` numbers it; a `label` only `text`.
 */
export interface SurfaceStrip {
  /** Unique within its surface. */
  id: string;
  kind: SurfaceStripKind;
  device_id?: string;
  /** A `MixerChannel` id. */
  channel?: string;
  mix?: number;
  input?: InputRef;
  output?: number;
  text?: string;
  /** A `port` strip's digital output. */
  port?: "SPDIF_OUT" | "ADAT_OUT";
  /** A port strip's first channel: 0, or 8 for a second ADAT port. */
  first?: number;
}

/** A user-built row of strips from any devices, each strip naming its device. */
export interface Surface {
  id: string;
  name: string;
  /** Device id → the mix its channel strips show. */
  mixes: Record<string, number>;
  strips: SurfaceStrip[];
}

/** A mixer layout saved by name, which any device of `family` can start from. */
export interface SavedLayout {
  id: string;
  name: string;
  family: "quadro" | "studio";
  mixer: DeviceMixer;
}
