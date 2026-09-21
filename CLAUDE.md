# Gazelle: notes for agents

A control server and web UI for two Antelope Audio interfaces (Rust workspace under `crates/`,
pnpm web app under `web/`, release helper in `xtask/`). Windows only.

The project's working notes (status, decisions, specs and reference) live in `.agent/`, which is
a separate private repository that may be checked out at that path; this repository ignores it.
When it is present, read `.agent/README.md` (the map of the working notes) and then
`.agent/STATUS.md` (where things are now, how to run things, what is next). Nothing in this
repository may depend on it or link into it: code comments, docs and workflows must stand on
their own. Public developer docs are in `docs/`: `docs/protocol.md`,
`docs/reverse-engineering.md` and `docs/releasing.md`.

## Hard rules

- **No em dashes (U+2014) or en dashes (U+2013) in product text**: the UI, tray, messages, logs,
  CLI help, docs, `CHANGELOG.md`, and comments in product source. Not as escapes or entities
  either. `xtask/tests/no_dashes.rs` and `pnpm -C web docs:check` enforce it.
- **Tests never touch hardware.** Every server a test or script starts runs with
  `--backend loopback` and `GAZELLE_NO_HARDWARE=1`; the default backend is `usb`, which opens the
  real devices. Stop a server you started by its own PID, never by name. `.cargo/config.toml`
  also sets `GAZELLE_NO_HARDWARE=1` for everything cargo starts, as a backstop after two harnesses
  forgot the flag in one week; do not remove it. Driving hardware from source takes an explicit
  `GAZELLE_NO_HARDWARE=0`, and only with the owner's go-ahead.
- **Never commit credentials**: no keys, tokens or `.env` files. The release signing key lives
  outside the repository (`xtask keygen` refuses to write one inside it) and in the `release`
  environment's secret on GitHub. `.githooks/pre-commit` scans for secrets; do not bypass it.
- **Push, tag or publish only with the user's OK.** Commit freely on a branch; anything that
  leaves the machine is the user's call.

## Changelog

`CHANGELOG.md` is the release notes: CI copies a version's section word for word into its GitHub
release and refuses to release a version whose section is missing or empty
(`xtask release-notes`).

- **A user-visible change adds a line under `## [Unreleased]` in the same commit.** Put it under
  `### Added`, `### Changed`, `### Fixed` or `### Removed` (create the subheading if it is not
  there yet).
- **Write for the person using the app, not for developers.** Say what they can now do or what
  now behaves differently, in their words: "The Mixer remembers which mix was open", not
  "Persist activeMix in the store". No file names, function names, P-numbers or ticket talk.
- **What counts:** anything a user can see, do, or be affected by: features, UI and wording
  changes, fixed bugs they could have hit, changed defaults, command-line flags, the HTTP API,
  install and update behaviour, supported devices, and security fixes.
- **What does not:** refactors, tests, CI, tooling, developer docs and working notes, dependency
  bumps with no visible effect, and fixes to something that was never released.
- **Cutting a release** (in the release commit, before tagging): bump `version` in
  `crates/gazelle-audio-server/Cargo.toml`; rename `## [Unreleased]` to `## [X.Y.Z] - YYYY-MM-DD`
  and add a fresh empty `## [Unreleased]` above it; tidy the notes into a short, readable summary.
  Release tags are the bare version with no `v` (`0.2.0`, `0.2.0-rc.1`), and the heading must
  name the same version (tag `0.2.0` needs `## [0.2.0]`). A heading
  with no date means "not yet released". An
  `xtask` test checks that the version in `Cargo.toml` always has a section, so a version bump
  without notes fails CI. The whole procedure is in `docs/releasing.md`.
