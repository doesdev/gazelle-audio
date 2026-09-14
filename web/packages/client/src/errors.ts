/** Codes the client raises itself (spec §4.4, plus `superseded` from coalescing). */
export type ClientErrorCode = "not_connected" | "timeout" | "closed" | "superseded";

/** Codes the control server sends, passed through unchanged. */
export type ServerErrorCode =
  | "unknown_device"
  | "no_registry"
  | "unknown_command"
  | "bad_value"
  | "timeout"
  | "device_gone"
  | "protocol_error"
  | "storage_error"
  | "unsupported"
  | "bad_request"
  | "not_found";

// `string & {}` keeps editor completion for the known codes while accepting ones a newer server adds.
export type ErrorCode = ClientErrorCode | ServerErrorCode | (string & {});

/** The one error type the client rejects with. */
export class GazelleError extends Error {
  override name = "GazelleError";
  readonly code: ErrorCode;
  /** Whatever the server sent as `detail`; `undefined` for client-raised errors. */
  readonly detail: unknown;

  constructor(code: ErrorCode, message: string, detail?: unknown) {
    super(message);
    this.code = code;
    this.detail = detail;
  }
}
