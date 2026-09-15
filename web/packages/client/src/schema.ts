// Shapes of the generated schema descriptors (src/generated/*.ts), shared with the client's
// value conversion.

/** Bytes as a caller may give them: hex (optionally `0x`-prefixed), byte values, or a `Uint8Array`. */
export type Bytes = Uint8Array | string | readonly number[];

export type Scalar = "u8" | "i8" | "u16" | "i16" | "u32" | "i32";

export type FieldDescriptor =
  | { readonly kind: "scalar"; readonly name: string; readonly scalar: Scalar; readonly bitWidth?: number; readonly default?: number }
  | { readonly kind: "array"; readonly name: string; readonly elem: Scalar; readonly count: number }
  | { readonly kind: "struct_array"; readonly name: string; readonly count: number; readonly fields: readonly FieldDescriptor[] }
  | { readonly kind: "elem_array"; readonly name: string; readonly count: number; readonly elem: FieldDescriptor };

export interface CommandDescriptor {
  /** As the server spells it, e.g. `0x70`. */
  readonly reportId: string;
  readonly params: readonly FieldDescriptor[];
  /** `null` for commands without a response. */
  readonly returns: readonly FieldDescriptor[] | null;
}

/** One routing group, as the vendor panel declares it. */
export interface TopologyGroup {
  /** `<TYPE><n>`: n counts earlier groups of the same type. */
  readonly id: string;
  readonly type: string;
  readonly typeId: number;
  readonly name: string;
  readonly channels: number;
  readonly color: string;
}

/** A family's routing groups and mixers (`refs/schemas/<family>_topology.json`). */
export interface Topology {
  readonly family: string;
  readonly source: {
    readonly tool: string;
    readonly bytecode: string;
    readonly blobs: readonly { readonly path: string; readonly sha256: string; readonly read: readonly string[] }[];
  };
  /** Routing sources. */
  readonly inputs: readonly TopologyGroup[];
  /** Routing destinations; MIXER_IN groups are the mixers' channels. */
  readonly outputs: readonly TopologyGroup[];
  readonly signalPresent: readonly { readonly field: string; readonly type: string; readonly typeId: number; readonly channel: number }[];
  readonly availableChannels: readonly { readonly type: string; readonly typeId: number; readonly field: string }[];
  readonly mixers: {
    readonly count: number;
    readonly channels: number;
    /** The family's mixer command: `set_mixer` or `set_mixer_cfg`. */
    readonly command: string;
    /** Device channel of the master; strip `i` is device channel `i + 1`. */
    readonly masterChannel: number;
    readonly stereoLinkId: number;
    /** MIXER_IN output group ids, one per mixer. */
    readonly inputGroups: readonly string[];
    /** MIXER_OUT input group ids, one per mixer. */
    readonly outputGroups: readonly string[];
  };
  /** What the extraction could not read and so assumes. */
  readonly assumptions: readonly string[];
}

export interface FamilySchema {
  readonly commands: { readonly [name: string]: CommandDescriptor };
  /** Keyed by report id as the server sends it in `cyclic` events, e.g. `0x73`. */
  readonly cyclic: { readonly [reportId: string]: readonly FieldDescriptor[] };
}
