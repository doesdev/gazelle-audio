// The in-app updater as `GET /api/v1/update` and the three POSTs answer it
// (crates/gazelle-audio-server/src/update). The routes exist only on a loopback bind: a server
// reachable from the network does not offer them, and every call here then fails `http_404`.
// Keys stay snake_case as on the wire.

/** Where the updater has got to. `state` names the variant; its own fields sit beside it. */
export type UpdateState =
  /** Nothing has been asked yet: a fresh start, before its first check. */
  | { state: "unknown" }
  | { state: "checking" }
  | { state: "up_to_date" }
  /** Found, and not fetched: only an install with `auto_download: false` stops here. */
  | { state: "available"; version: string; page: string }
  | { state: "downloading"; version: string }
  /** Verified and in place beside the running binary; it runs after a restart. */
  | { state: "staged"; version: string }
  /** `message` is the short reason a person reads; `detail` is the URL, status or error behind it. */
  | { state: "failed"; message: string; detail: string };

export interface UpdateStatus {
  /** The version running now, not the one that may be waiting. */
  version: string;
  target: string;
  channel: "stable" | "prerelease";
  /** Whether unattended checks happen at all. */
  check: boolean;
  /** Whether a found release is fetched without being asked. True unless the settings say not to. */
  auto_download: boolean;
  /** Whether this build can verify a download. False means it will not fetch one. */
  can_verify: boolean;
  /** When the source was last asked, in milliseconds since the Unix epoch, or null if never. */
  last_check_ms: number | null;
  state: UpdateState;
}

/**
 * What a restart answers. It comes back **before** the server stops, which it then does a
 * fraction of a second later, so the page should expect the connection to drop and come back on
 * the named version. A restart with nothing staged is a `GazelleError` with code `nothing_staged`
 * instead, and nothing is stopped.
 */
export interface UpdateRestart {
  restarting: boolean;
  /** The version being restarted into. */
  version: string;
}
