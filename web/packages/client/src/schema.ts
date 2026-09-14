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

export interface FamilySchema {
  readonly commands: { readonly [name: string]: CommandDescriptor };
  /** Keyed by report id as the server sends it in `cyclic` events, e.g. `0x73`. */
  readonly cyclic: { readonly [reportId: string]: readonly FieldDescriptor[] };
}
