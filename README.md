# Koru

Koru is a private, local-first workspace for thinking, writing and AI.
Documents, files and terminal sessions — your shells and your coding agents —
open as tabs in one native window. Your work lives on your own devices, works
offline, and syncs directly between them without a server in the middle.

Under the surface, a workspace is **a tree of typed nodes** — documents, tasks,
contacts, images — whose attributes are data rather than text, and **views are
projections over that model**. State is stored as [Loro](https://loro.dev)
snapshots. Because Loro is a CRDT, two devices editing the same workspace
converge without a server; a single user never exchanges an op and pays nothing
for the option.

The desktop app is written in Rust: [parley](https://github.com/linebender/parley)
for text layout, [vello](https://github.com/linebender/vello) for GPU rendering,
winit for windowing, and [iroh](https://iroh.computer) for device-to-device
sync. The phone apps are native Kotlin and Swift over the same contracts.

## Status

Pre-1.0 and moving quickly. The first public release shows **documents, files
and terminal/shell sessions**. More is built than is shown: tasks, the Koru AI
chat, canvases, reminders and Koru Cloud sign-in sit behind product flags that
are off unless a build turns them on
([product feature flags](guide/contributing/FEATURE-FLAGS.md)). The
[user guide](guide/user/USERGUIDE.md) describes the available behavior and
experimental features.

| Platform | Status |
| --- | --- |
| **Windows** (x86-64) | Supported. Builds, runs and packages from this repository |
| **macOS** | Builds and runs from source for development. Public macOS builds are held until a Mac verifies that they link the voice runtime's text-to-speech-free archive, which the tree now uses ([licensing/THIRD-PARTY.md](licensing/THIRD-PARTY.md#espeak-ng-gpl-30-in-three-shipped-binaries)) |
| **Android** | The native app builds with Gradle and is in active development; see [Android setup and verification](apps/android/README.md) |
| **iOS** | Held. The project in `apps/ios` is not released yet; the first release waits for it to be built and verified on a Mac |
| **Linux** | Not a target. CI runs only the platform-independent checks there; nothing claims the app builds or runs on Linux, and there are no packages |

## Build and run from a fresh clone

You need:

- **Rust**, from [rustup](https://rustup.rs). The checked-in
  `rust-toolchain.toml` selects Rust 1.99.0 for development and release builds;
  rustup installs it automatically. The minimum supported version is 1.92,
  declared in `Cargo.toml` and tested in CI. Windows also needs the MSVC build
  tools.
- **sccache**, which is not optional: `.cargo/config.toml` sets it as the
  compiler wrapper. `winget install Mozilla.sccache`, or `cargo install sccache`.
- **A GPU with DX12, Vulkan or Metal.** vello renders with compute shaders and
  has no software fallback.
- **Python 3**, for the repository's checks under `scripts/checks/`.

Then, on Windows:

```bat
git clone https://github.com/koruworks/mono.git koru
cd koru
koru.bat setup   REM sets up whatever is not set up yet, and lists what it could not
koru.bat run
```

`./koru.sh setup` and `./koru.sh run` are the same on macOS.

`setup` installs sccache; sets its cache directory, cache size and the cargo job
count for this machine (user environment variables on Windows, a marked block in
your shell profile elsewhere); and fetches fonts and third-party code. What is
already set up it leaves alone, and whatever it cannot do it lists with the
command that does it. Run it again at any time: it does only what is still
missing, and retries what failed. `setup --check` reports without changing
anything, including when combined with the optional flags below.

Two setup options are available separately:

- **`koru.bat setup --defender-exclusions`**: recommended for faster builds
  in a trusted local checkout after reviewing the trade-off. Windows asks for
  elevation. It disables Defender scanning for the repository, worktrees,
  Cargo dependency sources, and compiler cache. Leave it off when reviewing
  untrusted contributions. `scripts/setup/defender-exclusions.ps1 -Remove`
  reverses the exclusions.
- **`koru.bat setup --agent-skills`** (or `./koru.sh setup --agent-skills`):
  install coding-agent skills and the Git hook that installs them in new
  worktrees. This requires Node.js and runs the skills CLI. Its anonymous
  install telemetry can be disabled with `DISABLE_TELEMETRY=1`.

Ordinary build, test, and run commands do not install agent tooling or change
Git hooks. Both optional flags can be combined on Windows.

`run` downloads the bundled fonts on first use (`koru.bat fonts` does just that
step; the faces are pinned by hash in `scripts/setup/fonts.manifest`), compiles, and
launches. The first compile takes several minutes and everything after it is
incremental. **Fetch the fonts before running the tests on a fresh clone**, or
the typography tests fail.

Every launcher verb also fetches three crates Koru builds from patched source
(`onenote_parser`, `sherpa-onnx`, `sherpa-onnx-sys`) from crates.io and applies
Koru's patches, which are all of them this repository holds
([third-party/](third-party/README.md)). **To run `cargo` directly on a fresh
clone, run `scripts\setup\fetch-third-party.ps1` (or `bash
scripts/setup/fetch-third-party.sh`) once first**. Without them cargo stops with
`patch location ... third-party/crates/sherpa-onnx does not contain packages
matching`.

**Coding agents get Koru's skills from
[koruworks/skills](https://github.com/koruworks/skills)** (the document-driven
workflow: draft, review, plan, execute, hand off), installed with `npx skills
add` into `.agents/skills/`, which `.claude/skills` links to; none of it is
committed. `setup --agent-skills` installs them and sets the Git hook that
installs them in every `git worktree add` after it. It needs Node.js for `npx`,
and nothing else needs the skills: without Node the build is unaffected. To take
a newer set, run `sh scripts/setup/setup-skills.sh --update`.

**Koru Cloud is off in a build from source, and that is expected.** With no
`.env.production` at the repository root, `run` starts with Koru Cloud not
configured and says so in one line; the app works in full on its own device and
syncs directly between your devices. To point a build at a Koru Cloud of your
own, see [CONTRIBUTING.md](CONTRIBUTING.md#koru-cloud-running-without-one-or-with-your-own).

### Usage telemetry is opt-in

A build from source has the same telemetry destination as an official one,
Koru's metrics service at `https://m.koru.do`, and the same rule: nothing is
sent until you agree. Koru asks once on each device. What it sends is which
features are used, how well the app runs, and basic facts about the device such
as its operating system and screen size; never your documents, their names or
anything you type. The *Share usage data* switch in Settings changes the answer
at any time. `KORU_TELEMETRY_ENABLED=0` removes the destination altogether, and
`KORU_TELEMETRY_BASE_URL` points it at a service of your own
([reference/ENV.md](reference/ENV.md)).

### Voice and agents need CMake and libclang

`koru.bat run` launches the app alone. Koru Voice, the agent host,
`koru.bat run-all` and packaging also link [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx)
for speech recognition, and whisper.cpp, which is compiled from source:

```bat
winget install Kitware.CMake LLVM.LLVM     REM CMake, and libclang for the bindings
koru.bat run-all
```

The launcher downloads sherpa-onnx's prebuilt archive the first time (about
117 MB) into `third-party\sherpa-onnx\`, and refuses it unless it matches the
hash Koru pins; a new worktree copies it from the main checkout instead. It is
upstream's build **without text-to-speech**: the ordinary one links eSpeak NG,
which is GPL-3.0, and Koru does not use it. Why, and what ships:
[licensing/](licensing/README.md).

### Tests, packages and phones

```bat
koru.bat test                  REM cargo test --workspace, the whole suite
koru.bat dist-local beta       REM an unsigned local package under dist\ (needs the voice archive)
```

Android needs JDK 17 and Android SDK 36. The Gradle wrapper is fetched, not
committed, so on a fresh clone run `scripts\setup\fetch-third-party.ps1` (or
`bash scripts/setup/fetch-third-party.sh`) once, then `cd apps/android` and
`./gradlew testDebugUnitTest assembleDebug`. See the
[contributor guides](guide/contributing/README.md) for testing and implementation
guidance, and the [Android README](apps/android/README.md) for platform setup.

The launchers are thin: what they run lives under `scripts/`, in four folders.
`setup/` fetches and builds what a fresh clone lacks (fonts, dictionaries, the
third-party code, the voice runtime, the skills) and checks the machine;
`dev/` starts, stops, captures and probes the app while you work on it;
`checks/` holds the repository's own checks (links, the publication scan, SPDX
headers, sign-off), which CI runs too; and `icons/` regenerates the app and
mobile icons from the logo.

## Where to go next

The user and contributor guides live in [guide/](guide/README.md), alongside
the source, and are available in every clone. Internal plans, ADRs, company
notes, and hosted-service operations remain in the private `docs/` submodule.

| If you want to know | Read |
| --- | --- |
| How to work on the code — for people and coding agents alike | [AGENTS.md](AGENTS.md) |
| Where to start with the public documentation | [guide/](guide/README.md) |
| How to contribute, and the sign-off every commit needs | [CONTRIBUTING.md](CONTRIBUTING.md) |
| How to use it | [guide/user/USERGUIDE.md](guide/user/USERGUIDE.md) |
| Building, testing, Rust, and library behavior | [Contributor guides](guide/contributing/README.md) |
| The local API and wire schemas | [Generated references](reference/README.md) |
| The document model and MCP server | [MCP model and tools](crates/koru-mcp-server/MCP.md) |
| Environment variables, events, and shortcuts | [reference/](reference/README.md) |
| Licensing and third-party attribution | [licensing/](licensing/README.md) |

## Koru Cloud is not in this repository

Koru Cloud is the hosted service official builds can sign in to: accounts and
sync through the cloud. It is run by Koru
Works, Inc., and its server code, infrastructure and credentials are not part
of this repository. Everything the apps link is here, including the client side
of Cloud and the wire contracts it speaks (`contracts/`), so a build from this
source can talk to any server that implements them — or to none.

Official builds are this source at a tag, built with configuration only we
hold: the Cloud origin, the update-signing key and the Google Docs importer's
registration. A build without them turns those features off and says so.

## License

Koru is free software under the [Mozilla Public License 2.0](LICENSE). Change a
Koru file and distribute it, and that file stays open; add your own files beside
ours under whatever license you like. Every commit is signed off under the
[Developer Certificate of Origin](DCO); [CONTRIBUTING.md](CONTRIBUTING.md) says
how. Third-party components and what they require are listed in
[licensing/](licensing/README.md).

The Koru name, logo and icons are trademarks of Koru Works, Inc. and are not
covered by the license; see [TRADEMARKS.md](TRADEMARKS.md).
