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
 * Channels of one kind, on any devices, that change together (decision P51). `absolute` members
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
  /** Present while the mix is summed to mono: its channels are centred, and these are the pans to restore, by mixer input slot. */
  mono?: { pans: Record<string, number> };
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
}

/** A mixer layout saved by name, which any device of `family` can start from. */
export interface SavedLayout {
  id: string;
  name: string;
  family: "quadro" | "studio";
  mixer: DeviceMixer;
}
