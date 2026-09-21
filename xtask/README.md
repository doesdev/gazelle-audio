# xtask

The release helper: the steps of cutting a Gazelle release that are more than one command, written
as code so that the release workflow (`.github/workflows/release.yml`) and a person cutting a
release by hand run the same thing, and so that thing is under test. It is never shipped.

The procedure it serves, and when each step runs, is in
[`docs/releasing.md`](../docs/releasing.md). The module docs in [`src/main.rs`](src/main.rs) and
each subcommand's module say exactly what it checks.

```
cargo run -p xtask -- help
```

| Subcommand | What it does |
| --- | --- |
| `keygen --out <file>` | Makes an ed25519 signing pair, writes the private half to a file and prints the public half. Refuses a path inside a git working tree. |
| `check-version [<tag>]` | The tag must be the server's version, bare (`1.0.0`, no `v`). |
| `release-notes <version>` | Prints that version's section of `CHANGELOG.md`, and fails when it is missing or empty. |
| `smoke` | Starts both built binaries and checks what they answer: the version, that the updater is compiled in with a signing key (with `--pubkey`, that key), and that `/` is the web app rather than the "not built" notice. |
| `dist --out <dir>` | Collects the release directory under the names the updater asks for, with the setup file, the zip and the PDFs, and nothing else. |
| `pubkey` | Prints the public half of the signing key, and fails when it is not the one expected. |
| `sign --dir <dir>` | Hashes every file into `SHA256SUMS` and signs it. |
| `verify --dir <dir> --pubkey <hex>` | Checks a signed directory the way an installed copy will, and is strict about what is in it. |

## Where it sits

It depends on [`gazelle-audio-server`](../crates/gazelle-audio-server/README.md) with default
features off, to use the updater's own code: `verify` checks a release with exactly the functions
an installed copy checks it with, `dist` names assets exactly as the updater asks for them, and
`check-version` reads the version the binary reports. Nothing depends on it.

## The key

**The private key never goes in the repository.** It lives in a file outside it and, for CI, in the
`release` environment's secret on GitHub. It reaches `sign` and `pubkey` by `--key <file>` or by
`GAZELLE_RELEASE_KEY` naming that file, never as a value on the command line. `keygen` refuses to
write one inside a git working tree, and `sign` refuses a key file inside the directory it signs,
and anything there that looks like a key, because everything in that directory is uploaded.

## Testing

```bash
cargo test -p xtask
cargo clippy -p xtask --all-targets
```

- `tests/release_dry_run.rs` cuts a whole release and consumes it without publishing anything: a
  key made outside the repository, a directory signed by the real `xtask` binary, served from disk
  over local HTTP to a real updater, which checks, downloads, verifies and stages it, and the staged
  file then runs. The binaries in that release are stand-ins (copies of `xtask` itself), so no
  server is started.
- `tests/no_dashes.rs` fails on any em dash (U+2014) or en dash (U+2013), or an escape or entity
  for one, in the product's source: the web app and client, every crate's sources, build script
  and manifest, this helper, the themes, the embedded schemas, `CHANGELOG.md` and `CLAUDE.md`.
  `docs/` and the root `README.md` are checked by `corepack pnpm -C web docs:check` instead. Crate
  READMEs are covered by neither, so check them by hand.
- A test also checks that the server's current version has a section in `CHANGELOG.md`, so a
  version bump without notes fails.

`smoke` is the one subcommand that starts the server, and it starts it only on the loopback backend
with `GAZELLE_NO_HARDWARE=1`, `--no-tray` and `--no-persist`, on a spare localhost port with a
scratch config directory: it never opens a device, touches your own settings or asks GitHub
anything.
