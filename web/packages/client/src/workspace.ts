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
  members: ChannelRef[];
  /** Nested sub-groups. */
  children: Group[];
}

/** A stereo or multi-channel link, possibly spanning devices. */
export interface Link {
  id: string;
  members: ChannelRef[];
}

export interface Workspace {
  /** Schema version; 1 today. */
  version: number;
  groups: Group[];
  links: Link[];
  /** Device id → the name a user gave it. */
  aliases: Record<string, string>;
}
