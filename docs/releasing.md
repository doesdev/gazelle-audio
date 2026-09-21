# Releasing Gazelle

How a version of Gazelle is built, signed and published. This is a developer document, not part
of the user manual.

A release is one GitHub release per version, drafted by a workflow and published by a person.
Everything the app's updater trusts is signed with an ed25519 key that never enters the
repository.

## What a release carries

The tag is the **bare version**: `X.Y.Z`, or `X.Y.Z-rc.N` for a release candidate, with no `v`
in front. The updater tolerates a leading `v`, but the release workflow and
`xtask check-version` accept only the bare form. The release carries, for each platform:

| File | What it is |
|---|---|
| `gazelle-audio-server-<target>.exe` | The console build: the asset the updater fetches |
| `gazelle-audio-serverw-<target>.exe` | The windowless build, fetched too when it is installed |
| `Gazelle-Setup.exe` | Windows only: the windowless build byte for byte, under the name a person downloads to install. Double-clicked, it offers to install itself. The updater ignores it |
| `gazelle-audio-<target>.zip` | Both binaries, for a person downloading by hand; the updater ignores it |
| `gazelle-manual.pdf`, `gazelle-cheat-sheet.pdf` | The docs; once per release, not per target |
| `SHA256SUMS` | One `<digest>  <name>` line per asset |
| `SHA256SUMS.sig` | 64 raw bytes: the detached ed25519 signature over `SHA256SUMS` |

`<target>` is the Rust target triple, such as `x86_64-pc-windows-msvc`, recorded at build time
by the server's `build.rs`. Non-Windows targets drop the `.exe`, and carry no setup file.

The setup file has no target in its name because it is the link a person follows, and Windows
on x86_64 is the only build there is. Its name is what puts the windowless build into setup mode
(the server's `install/setup.rs`); both binaries embed an application manifest saying they run
as the invoker, so Windows never takes a file called "Setup" for an installer that needs
administrator rights.

The asset the updater fetches is the executable itself, not an archive: unpacking an archive
from the network would be code running on bytes nothing has verified yet, while a bare binary
makes applying an update two renames and no parser.

A release is **not** signed for SmartScreen (Windows warns on a first download), and it is not
an update for any platform it carries no binary for: the updater ignores a release with nothing
for its own target.

### The aggregate driver travels inside the executables

Gazelle Aggregate (`gazelle_aggregate.dll`, built from `crates/gazelle-audio-aggregate`) is not a
release asset of its own. On Windows both binaries carry it byte for byte, and the server writes
it out beside itself: `--install` writes it into the install folder, and every start writes it
beside the running executable when it is missing or differs. That keeps the one file setup and
the updater exactly as they are, and since `SHA256SUMS` covers the executables, its signature
covers the driver too. An update therefore brings a new driver with the new executable, and the
updated Gazelle's first start puts it in place.

A DAW may have the driver loaded when that happens. Windows will not overwrite a loaded DLL but
will rename it, so the old file is renamed to `gazelle_aggregate.dll.<n>.old`, the new one takes
its place (the registration names the path, so the next DAW to open the driver gets the new one),
and a later start deletes the renamed copies once nothing holds them. Failing even that never
fails the install or the start: `--install` prints why, the Aggregate page shows it, and the next
start tries again. Registering the driver is still the Aggregate page's, with Windows'
administrator prompt; nothing registers it unasked.

Which file is carried is decided when the server is built, by `GAZELLE_AGGREGATE_DLL` (the
server's `build.rs`): the path of the DLL to carry, taken from the workspace root when it is
relative. Unset, which it is for every developer build and every test, the server carries
nothing, and the Aggregate page says that this build does not carry the driver. Set to a file that
is missing, is not a DLL, or is built for another machine, **the build fails** with the reason, so
a release can never quietly ship without it. The driver is built with the same static C runtime as
the binaries (`.cargo/config.toml` applies to every crate built for the Windows targets), and
`tests/runtime.rs` in the server reads its import table to prove it.

## Cutting a release

In the release commit, before tagging:

1. Bump `version` in `crates/gazelle-audio-server/Cargo.toml`.
2. In `CHANGELOG.md`, rename `## [Unreleased]` to `## [X.Y.Z] - YYYY-MM-DD` and add a fresh,
   empty `## [Unreleased]` above it. Tidy the notes into a short, readable summary. The heading
   must name the same version as the tag (tag `1.1.0` needs `## [1.1.0]`); a heading with no date
   means "not yet released".
3. Push `main` and wait for CI to be green.

CI refuses a version whose changelog section is missing or empty (`xtask release-notes`), and an
`xtask` test checks that the version in `Cargo.toml` always has a section, so a version bump
without notes fails.

### The automated path

Pushing a bare version tag runs `.github/workflows/release.yml` on a Windows runner:

```
git tag -a X.Y.Z -m "Gazelle X.Y.Z"
git push origin X.Y.Z
```

| Job | What it does | Holds the signing key |
|---|---|---|
| `build` | `xtask check-version <tag>`; `xtask release-notes <version>`; checks the `GAZELLE_UPDATE_PUBKEY` variable is 64 hex digits; `pnpm install --frozen-lockfile` and `pnpm build`; the aggregate driver's release build; every suite (cargo test, clippy with `-D warnings`, the window check, pnpm test, e2e, docs:check); the release build with `--features window`, the public key and `GAZELLE_AGGREGATE_DLL` naming the driver just built; the import table test (`--test runtime`) on the release binaries, the driver beside them and the copy they carry; `xtask smoke` on both binaries; `pnpm docs:pdf` against the release binary; `xtask dist`; a release build of `xtask` itself. Uploads the unsigned directory, the helper and the notes as workflow artifacts. No build cache is restored: a release is built from nothing. | No |
| `sign` (tag pushes only) | Runs in the protected `release` environment, so it waits for approval. A fresh machine that checks nothing out and builds nothing: it runs the helper the build job made. Writes the secret to the runner's temporary directory, checks its public half is the one the binaries carry (`xtask pubkey --expect`), signs (`xtask sign`), deletes the key, and verifies with the updater's own code (`xtask verify`), whose output is the upload list. Then `gh release create <tag> --draft --verify-tag`, with `--prerelease` for a semver pre-release, uploading exactly the verified files. The only job with `contents: write`. | Yes, only after the build is finished |
| `dry-run` (manual runs only) | The same signing and verification with a throwaway key made on the spot, and a summary of the release that would have been drafted. No secret, no environment, nothing created. | No |

Approve the `release` deployment on the run's page when it asks. A draft release appears under
Releases with the changelog section as its notes; read it, then publish it. Finally prove the
update (step 9 below).

The public key is checked against the secret in the sign job, before signing, rather than before
building: doing it earlier would need the secret in an earlier job, and every job in the
`release` environment asks for its own approval. A mismatch still fails before anything is signed
or drafted, and the smoke test has already proved the binaries carry the variable's key.

Every step that is more than one command is an `xtask` subcommand, so the workflow and a person
at the keyboard run the same code, and it is under test:

```
xtask check-version [<tag>]      the tag must be the server's version, bare; prints version=, tag=, prerelease=
xtask release-notes <version>    that version's CHANGELOG.md section; fails if missing or empty
xtask smoke [--pubkey <hex>]     both built binaries on the loopback backend: health version, can_verify,
                                 the target, the web app at /, (with --pubkey) that exact key inside, and
                                 on Windows the gazelle_aggregate.dll beside them, byte for byte, inside
xtask dist --out <dir>           the release directory, named as the updater asks; new or empty dirs only
xtask pubkey --key <file> [--expect <hex>]
                                 the key's public half; fails hard on a mismatch
xtask sign --dir <dir> --key <file>
                                 refuses a key inside <dir> and any file there that looks like a key
xtask verify --dir <dir> --pubkey <hex>
                                 the updater's own checks, plus: every file listed, every listed file
                                 there, both binaries present, and on Windows the setup file present
                                 and identical to the windowless build; prints the upload list
```

### The manual fallback

The same steps by hand, for when GitHub Actions is not available. Run everything from the
repository root, on Windows, with the tree clean. `<outside-the-repo>/gazelle-release.key` stands
for wherever the signing key lives.

```bash
# 1. The version: bumped and noted as above, then
cargo run -p xtask -- check-version X.Y.Z               # the tag you are about to push
cargo run -p xtask -- release-notes X.Y.Z               # the notes the release will carry

# 2. The web UI, which the binary embeds. Skipping this ships a "not built" notice page.
corepack pnpm -C web install --frozen-lockfile
corepack pnpm -C web build

# 2b. The aggregate driver, which the release build carries. Built first.
cargo build --release -p gazelle-audio-aggregate

# 3. The suites, all green.
cargo test -q --workspace
cargo clippy -q --workspace --all-targets -- -D warnings
cargo check -q -p gazelle-audio-server --features window
corepack pnpm -C web test && corepack pnpm -C web e2e && corepack pnpm -C web docs:check

# 4. The shipped build: release, with the window, with the public key, carrying the driver.
#    Repeat per target; the asset names carry the triple, which build.rs records. A driver
#    path that is missing or not a DLL for this machine fails the build.
GAZELLE_UPDATE_PUBKEY=<64 hex digits> GAZELLE_AGGREGATE_DLL=target/release/gazelle_aggregate.dll \
  cargo build --release -p gazelle-audio-server --features window

#    The import tables of what ships: no Visual C++ runtime DLL in either binary, the driver
#    beside them or the copy they carry. The same variables, so nothing is rebuilt.
GAZELLE_UPDATE_PUBKEY=<64 hex digits> GAZELLE_AGGREGATE_DLL=target/release/gazelle_aggregate.dll \
  cargo test --release -p gazelle-audio-server --features window --test runtime

# 4a. Start both built binaries (loopback backend, a spare port, a scratch config) and check the
#     version, can_verify, the target, the web app at /, that they carry exactly this key, and
#     that they carry target/release/gazelle_aggregate.dll byte for byte.
GAZELLE_NO_HARDWARE=1 cargo run -p xtask -- smoke --version X.Y.Z --pubkey <64 hex digits>

# 4b. The docs PDFs, checked against the binary just built. They are release assets, never
#     commits (docs/dist/ is ignored).
GAZELLE_BIN=target/release/gazelle-audio-server.exe corepack pnpm -C web docs:pdf

# 5. Collect the release directory under the names the updater asks for, with the setup file,
#    the zip and the PDFs. It refuses a directory that already holds anything.
cargo run -p xtask -- dist --out dist

# 6. Sign, LAST, after every file is in the directory. Everything in it is hashed, so the key
#    must not be in it (sign refuses), and a binary rebuilt after this step fails its own hash.
cargo run -p xtask -- pubkey --key <outside-the-repo>/gazelle-release.key --expect <64 hex digits>
cargo run -p xtask -- sign --dir dist --key <outside-the-repo>/gazelle-release.key
#    (or set GAZELLE_RELEASE_KEY to that path and leave --key off)
cargo run -p xtask -- verify --dir dist --pubkey <64 hex digits>
```

`verify` prints exactly the files to upload. Then:

7. Tag and push the tag, as above.
8. Create the GitHub release on the tag, titled `Gazelle X.Y.Z`, and upload exactly the files
   `verify` listed and nothing else, with `xtask release-notes X.Y.Z` as the notes. Tick
   "pre-release" for a release candidate; the updater honours both that flag and a semver
   pre-release in the tag (its `channel: "prerelease"` setting opts in).
9. Prove the update from a copy that is one version behind, before announcing it: with the old
   binary running, `POST /api/v1/update/check` then `POST /api/v1/update/download`, confirm the
   state becomes `staged`, restart, and confirm `GET /api/v1/health` reports the new version and
   the `.exe.old` files are gone. Beside the executable, `gazelle_aggregate.dll` is now the new
   release's driver (`GET /api/v1/aggregate` reports `registration.bundled` as `in_place`), and
   with a DAW holding the old one open it is instead a `gazelle_aggregate.dll.1.old` beside it,
   gone at the first start after the DAW is closed.

## The release checklist

Before tagging, all of these must be true:

| | What | How it is checked |
|---|---|---|
| 1 | **The version is the one to ship**, has never been released, and has a non-empty `CHANGELOG.md` section. The tag is that version exactly, bare. | `xtask check-version <tag>` and `xtask release-notes <version>`; `xtask smoke` checks `GET /api/v1/health` on the built binaries reports the same |
| 2 | **The signing key exists and is backed up**, outside the repository and outside the release directory. Losing it means no installed copy can ever be updated again. | `xtask keygen` refuses to write inside a git working tree |
| 3 | **The public half is built into the binaries being uploaded.** A build without it will not download an update at all. | `xtask smoke --pubkey <hex>`: `GET /api/v1/update` on both binaries reports `"can_verify": true`, and each binary holds that key; `xtask pubkey --expect` before signing |
| 4 | **The updater's default repository is this one.** It is set in `crates/gazelle-audio-server/src/update/settings.rs`. | By reading it |
| 5 | **Both binaries carry the icon.** | `cargo test -p gazelle-audio-server --test icon`, and by eye in Explorer |
| 5a | **Both binaries carry the aggregate driver**, the one just built, and neither it nor they need the Visual C++ runtime. | The release build fails when `GAZELLE_AGGREGATE_DLL` names no usable DLL; `xtask smoke` refuses a binary that does not hold `target/release/gazelle_aggregate.dll` byte for byte; `cargo test --release -p gazelle-audio-server --features window --test runtime` with the same variables |
| 6 | **The workspace is green**, with the window feature compiling. | `cargo test --workspace`, `cargo clippy --workspace --all-targets`, `cargo check -p gazelle-audio-server --features window` |
| 7 | **The web UI is built**, or the binary embeds a "not built" notice page instead of the app. | `pnpm -C web build` before the release build; `xtask smoke` checks `GET /` is the app |
| 8 | **The installer works by hand**: `Gazelle-Setup.exe` downloaded from the draft release and double-clicked (no administrator prompt; SmartScreen's "More info", "Run anyway"), run again over the install, a real `--install`, the Start Menu entry, the entry in Settings, Apps, an uninstall from that list, and an uninstall of a relocated copy removing its own binary. `gazelle_aggregate.dll` is in the install folder after the install; with the driver registered from there, an uninstall offers to unregister it (and a quiet one, `--uninstall --yes`, leaves it and says how). | By hand; `cargo test -p gazelle-audio-server --test icon` checks the manifest that keeps the setup file from asking for elevation, and `--test install` the driver's install, replacement and uninstall against temporary folders |
| 9 | **A staged update restarts into the new version**, including from the tray's "Restart to update" item. | Step 9 above |
| 10 | **The docs are current and print.** The manual and the cheat sheet describe this version and their PDFs build. | `corepack pnpm -C web docs:pdf` passes its checks (no em or en dashes, links and images resolve, the command-line chapter matches the release binary's `--help`) and writes both PDFs; the manual's title page names the version |

## One-time setup

Once per repository, before the first release:

1. **Make the signing key**, outside the repository (the tool refuses to write one inside a git
   working tree), and back the file up offline: losing it means no installed copy can ever be
   updated again.

   ```
   cargo run -p xtask -- keygen --out <outside-the-repo>/gazelle-release.key
   ```

   It prints the public half: `GAZELLE_UPDATE_PUBKEY=<64 hex digits>`. The file holds the
   private half as 64 hex digits and a newline, which is exactly what the secret must contain.

2. **Create the `release` environment**: the repository's Settings, Environments, New
   environment, named `release`. Under Deployment protection rules, tick **Required reviewers**
   and add whoever approves releases. Under **Deployment branches and tags**, choose "Selected
   branches and tags" and add a **tag** rule with the pattern `[0-9]*.[0-9]*.[0-9]*`, so only a
   bare version tag can reach the key.

3. **Add the secret to that environment**, not to the repository: on the environment's page,
   Environment secrets, `GAZELLE_RELEASE_KEY`, pasting the key file's contents. Or:

   ```
   gh secret set GAZELLE_RELEASE_KEY --env release --repo <owner>/<repo> < <outside-the-repo>/gazelle-release.key
   ```

4. **Add the public half as a repository variable** (Settings, Secrets and variables, Actions,
   Variables), `GAZELLE_UPDATE_PUBKEY`, the 64 hex digits keygen printed:

   ```
   gh variable set GAZELLE_UPDATE_PUBKEY --repo <owner>/<repo> --body <64 hex digits>
   ```

5. **Optionally, a tag ruleset** (Settings, Rules, Rulesets, a new tag ruleset targeting
   `[0-9]*.[0-9]*.[0-9]*` that restricts creation, update and deletion), so a release tag cannot
   be moved after it is pushed.

6. **A dry run**, once `release.yml` is on the default branch (a manual run needs the file
   there): Actions, Release, Run workflow, or

   ```
   gh workflow run release.yml --repo <owner>/<repo> --ref main
   gh run watch --repo <owner>/<repo>
   ```

   It builds and checks everything, signs with a throwaway key, and writes the release it would
   have drafted into the run's summary. Nothing is created, and its artifacts are signed with the
   throwaway key, so they must never be uploaded.
