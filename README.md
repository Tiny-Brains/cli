# cli

`tinybrains` plays a TinyBrains match on your own machine, with no server, no database and no
season. It hosts a game's cartridge through wasmtime and runs a model's adapters through
**datalogic**, on an engine built through **dataflow-rs** the way an Orion node builds it, and its
graph through **tract**. So `check` and a local match report what the ladder will, and every replay
uses the ladder's format.

## Install

On macOS (Apple silicon) and Linux, with Homebrew:

```sh
brew tap tiny-brains/cli https://github.com/Tiny-Brains/cli
brew install tiny-brains/cli/tinybrains
```

Or take the archive for your platform from the
[latest release](https://github.com/Tiny-Brains/cli/releases/latest), check it against that
release's `SHA256SUMS`, and put the binary on your `PATH`:

```sh
base=https://github.com/Tiny-Brains/cli/releases/latest/download
archive=tinybrains-x86_64-unknown-linux-gnu.tar.gz
curl -fsSLO "$base/$archive"
curl -fsSL "$base/SHA256SUMS" | grep " $archive\$" | sha256sum -c -
tar -xzf "$archive" tinybrains && sudo mv tinybrains /usr/local/bin/
```

```powershell
Invoke-WebRequest https://github.com/Tiny-Brains/cli/releases/latest/download/tinybrains-x86_64-pc-windows-msvc.zip -OutFile tinybrains.zip
Expand-Archive tinybrains.zip -DestinationPath "$env:LOCALAPPDATA\tinybrains"   # then add that folder to PATH
```

| Archive | Platform |
|---|---|
| `tinybrains-aarch64-apple-darwin.tar.gz` | macOS 26 or newer, Apple silicon |
| `tinybrains-aarch64-unknown-linux-gnu.tar.gz` | Linux arm64, glibc 2.35 or newer |
| `tinybrains-x86_64-unknown-linux-gnu.tar.gz` | Linux x86-64, glibc 2.35 or newer |
| `tinybrains-aarch64-pc-windows-msvc.zip` | Windows on Arm, 64-bit |
| `tinybrains-x86_64-pc-windows-msvc.zip` | Windows x86-64 |

- Every archive holds the binary, `LICENSE` and this README.
- Every build is 64-bit. There is no Intel macOS build.
- The binaries are not signed.
  - On macOS, a file downloaded by a browser is quarantined. To clear it, run
    `xattr -d com.apple.quarantine tinybrains`.
  - On Windows, SmartScreen may ask before the first run.
  - Downloads through `curl`, `Invoke-WebRequest` and Homebrew are not affected.
- To build from source you need a Rust toolchain. Run
  `cargo install --locked --git https://github.com/Tiny-Brains/cli`.

## Quick start

Run it from a game's starter kit. The kit carries the `games.toml` that `tinybrains` reads:

```sh
git clone https://github.com/Tiny-Brains/ants-starter && cd ants-starter
tinybrains games                            # what is registered, at which engine digest
tinybrains check model.onnx manifest.json   # what admission will say
tinybrains matches/self-play.json           # play it: one replay per row, in replays/
tinybrains view replays/self-play.json      # watch it in a browser
```

The first command that needs the game downloads the cartridge release that `games.toml` pins. It
does this once, and refuses the download unless both digests match.

**What is the same as the ladder:** the component, the evaluator, the runtime, the head decode,
`cartridge.json`, the boards and the replay envelope.

**What is different:**
- models are stored in a local directory, not S3
- a replay is written to a file, not uploaded to a presigned URL
- rows come from a file, not from a claim under a lease

The wave loop is a Rust copy of the ladder's, and `tinybrains conform` checks that the two agree.

## Commands

| Command | What it does |
|---|---|
| `tinybrains <match.json>` or `tinybrains run <match.json>` | Plays every row of a match file as one wave, and writes one replay per row to `replays/<id>.json`. It prints ranks, scores and strikes, the mean operations and inference per seat-turn, and the share of the turn deadline the worst seat-turn used. |
| `tinybrains games` | Prints the registry in use, the datalogic version, and each game's engine digest, source and boards. |
| `tinybrains maps [GAME]` | Lists the boards the release ships, and the limits a season's board must fit. |
| `tinybrains maps export [GAME] [DIR]` | Writes those boards out as files, checked against the release's catalogue. The default DIR is `./maps`; to give a DIR you must give GAME first. |
| `tinybrains maps check <board.json>...` | Checks boards the way a season's upload will: the header, the name, the game's `limits.boards`, then the engine's own `worldgen`. A season board is named `size-terrain-Np-Hh`, where `N` is the file's `players` and its `hills` array holds N × H entries. A name that breaks the rule is refused with Soma's own code: `map_name_pattern`, `map_name_players` or `map_name_hills`. For a `basic-*` board it is only a warning, because those ship in the release and are never uploaded. |
| `tinybrains check <model.onnx> <manifest.json>` | Does admission's checks: the graph's opset and operators against the deployment's allowlist (an operator off it is `OP_NOT_ALLOWED` there whatever this binary's runtime can execute, so it is refused here first), its parameters and nodes, and the size metric (artifact bytes + manifest bytes). It then runs every adapter over the game's reference observations under the adapter budget, runs the graph, and reads the head. Last it runs admission's probe as a node does: five inferences over zero-filled inputs at the manifest's `probe_dims`, whose median must fit the game's `turn_ms`, the admitting runner's `models.max_probe_ms`. A model with memory is priced and played in a chain (see [Memory](#memory)). It exits non-zero on a failure. |
| `tinybrains adapt <model.onnx> <manifest.json>` | Writes each tensor the manifest's adapters build as a numpy `.npy` file, next to the observation that produced it. Use this to diff your trainer's encoder against the ladder's. An observation given with `--obs` may carry `memory` or `ant_memory`, in the tensor wire form (`{"tensor": {"dtype", "shape", "data"}}`) or as nested arrays. Nested arrays are decoded into the dtype the manifest declares for the output of that name. Either way the adapter gets a live tensor, as on a node. |
| `tinybrains conform <replay.json>` | Rebuilds a recorded match from its envelope, plays it here, and diffs every field and every turn of the action stream. It exits non-zero on a difference. It refuses a replay played on another engine digest. |
| `tinybrains view <replay.json>` | Serves the game's viewer and the replay on `127.0.0.1`, and opens a browser. The replay is mounted on the viewer's Stage tier, with the viewer's graph under it when the viewer ships `mountGraph`. An older viewer plays the replay without the graph. |
| `tinybrains env` | Runs the cartridge as a training environment over JSON Lines (see [`env`](#env-the-training-environment)). |
| `tinybrains --version`, `--help` | |

| Flag | Commands | Meaning |
|---|---|---|
| `--game SLUG` | match, `check`, `adapt`, `conform`, `view`, `env`, `maps check` | Which registry entry to use. The default for a match is the file's `game`. For `view` and `conform` it is the replay's `game`, falling back to `ants`. Otherwise it is the first game in the registry. |
| `--out DIR` | match, `adapt` | Where the output goes. The default is `./replays` for a match and `./tensors` for `adapt`. `adapt` writes one `case-<i>/` per observation and an `index.json`. |
| `-v`, `--verbose` | match | Prints every strike and forfeit as it happens, and every seat-turn that ran over the turn deadline. |
| `--timings` | match | Shows where the wall clock went, phase by phase (see below). |
| `--json` | `check` | Prints the whole report as one JSON object, for scripts. |
| `--memory-flat-bytes N`, `--memory-cell-bytes N` | `check` | The weight class's two memory numbers (0 to 262144, and 0 to 16). With either one, the model's memory is judged against that class; a number left out is 0. |
| `--obs FILE` | `adapt` | Uses these observations instead of the reference set: one observation, an array of them, or `{"observations": [...]}`. |
| `--no-open` | `view` | Prints the URL without opening a browser. |

**`--timings`** splits a match's wall clock into phases:
- registry, cartridge compile, model load, worldgen, observe, adapter, graph, head decode, step,
  finish and replay write
- `unaccounted`, whatever is left of the measured wall clock

A second table splits the cartridge calls into instantiate, JSON encode, the wasm call and JSON
decode. Those calls also appear in the first table. Recording is always on, and only the printing
waits for the flag.

`infer_us` measures `plan.run` only, not the adapter that fed it. That is the number in a replay's
`seats` and in `check --json`.

## Memory

A manifest may declare an output named `memory`, one named `ant_memory`, both or neither. The
runner keeps each seat's last value of each, and puts it on that seat's next view under the same
key. An adapter reads it with `{"tensor": [{"var": "memory"}]}`.

| When | The seat's `memory` (and `ant_memory`) |
|---|---|
| Turn 0 | Absent from the view |
| After an answered call | That call's output of the same name |
| After a struck call (the call failed) | Unchanged: the last value the model wrote |
| A new match | Absent again |

- Each seat keeps its own memory, even two seats of one model.
- The referee reads `policy` alone, and the replay carries no memory.
- A call that answers with a head the platform cannot read is still a strike. Its memory is kept,
  because the call answered. Kalam does the same.
- The adapter gets the output tensor itself, here as on a node: the view goes into the evaluator
  with each memory as a live tensor. `{"var": "memory"}` is that tensor, never an object, so an
  adapter cannot tell a laptop from the ladder by looking inside it.

**`check` prices a memory** the way the admit clock does, from the manifest alone. Each dtype has a
width in bytes: 1 for `bool`, `i8` and `u8`; 2 for `f16`, `i16` and `u16`; 4 for `f32`, `i32` and
`u32`; 8 for `f64`, `i64` and `u64`. An output whose shape names a dimension costs that much per
cell: the product of its numeric dimensions, times the width, times the board's cells. An output
that names no dimension is a fixed cost. `memory` may name at most two dimensions and `ant_memory`
at most one, and no name may appear twice in one shape. `check` prints the fixed bytes, the bytes
per cell, and the total at the smallest board (the smallest side squared) and the largest
(`limits.boards.cells_max`).

With `--memory-flat-bytes` and `--memory-cell-bytes`, the class's cap is
`flat + cell × cells`, and it is checked at both boards. The verdicts are the admit clock's:

| Code | When |
|---|---|
| `MEMORY_SHAPE` | A memory output names too many dimensions, names one twice, or lacks a dtype with a width or a shape |
| `MEMORY_NOT_ALLOWED` | The manifest declares a memory output and the class allows 0 and 0 |
| `MEMORY_TOO_LARGE` | The memory is over the cap at the smallest or the largest board |
| `MEMORY_ROUND_TRIP` | A call that was fed the model's own memory failed |

**The round trip.** For a model with memory, `check` chains the reference observations the way the
admitting runner does. Observation i gets the memory outputs of call i-1 when observation i-1 was on
a board of the same size and that call answered. Otherwise the keys are absent. No observation is
played twice. `check` reports how many calls were fed a memory and how many of those failed.

## Registry and match files

### The registry

`tinybrains` uses the first of these that exists. The binary carries no registry of its own.

1. `$TINYBRAINS_REGISTRY`
2. `./games.toml`
3. `./tinybrains.toml`
4. `registry.toml` in the cache directory (see [Environment variables](#environment-variables))

An entry names either an artifact set on disk or a pinned release:

```toml
[games.ants]
name = "Ants"
path = "../ants/dist"            # a checkout's dist/, or an unpacked release; relative to this file
```

```toml
[games.ants]
name = "Ants"
repo = "Tiny-Brains/ants"
release = "engine-<12 hex>"
artifacts = { file = "ants-artifacts.tar.gz", sha256 = "sha256:<64 hex>" }
engine = "sha256:<64 hex>"       # the component's digest: the one the ladder plays
```

**A `path` entry** wins over a release. Its digest is whatever the component hashes to, and
`tinybrains games` reports it.

**A release entry** needs all four fields: `repo`, `release`, `artifacts` and `engine`. The archive
is downloaded from the GitHub release once and unpacked under `cartridges/` in the cache. It is
refused unless the archive hashes to `artifacts.sha256` and the component inside hashes to `engine`.
An ants release's notes print this block.

### Match files

A match file holds the rows a ladder runner claims, plus the vars they run under:

```json
{
  "game": "ants",
  "vars": { "max_turns": 300 },
  "rows": [
    { "id": "self-play", "seed": 42, "map": "basic-tiny-2p", "seat_count": 2,
      "seats": [
        { "seat": 0, "weights": "../model.onnx", "manifest": "../manifest.json", "label": "mine" },
        { "seat": 1, "weights": "../model.onnx", "manifest": "../manifest.json", "label": "mine-again" }
      ] }
  ]
}
```

**Seats.** A seat names its model in one of three ways:
- `weights` + `manifest`: a path relative to the match file, or a URL
- `weights_hash` + `manifest_hash`: digests already in the local store
- `script`: written orders instead of a model

**Boards.** Every row needs a `map`, given in one of three ways:
- the id of a board the release ships (`tinybrains maps`)
- a path ending in `.json`, relative to the match file. This is how you play a season's board.
- the board itself, inline

A row that names a `preset` is refused. `seat_count` must equal the board's `players`, and every
row in a file must seat the same number.

**Vars.** `vars` override the game's own numbers, and only the ones you write. The keys are
`max_turns`, `turn_ms`, `budget_ops` and `strike_ceiling` (default 5).

**Engine digest.** If a top-level `engine_digest` differs from the resolved game's, the run prints
a warning.

The full field reference is the book's
[Testing › Match files](https://github.com/Tiny-Brains/web/blob/main/docs/src/models/testing.md#match-files).

## Environment variables

| Variable | Effect |
|---|---|
| `TINYBRAINS_REGISTRY` | The registry file to use. The lookup above is skipped. |
| `TINYBRAINS_HOME` | The cache directory. The default is `~/Library/Caches/tinybrains` on macOS, `$XDG_CACHE_HOME/tinybrains` or `~/.cache/tinybrains` on Linux, and `%LOCALAPPDATA%\tinybrains` on Windows. |
| `TINYBRAINS_PROFILE_NODES` | When set to any value, the graph runs node by node through tract's state machine instead of `plan.run`, and `--timings` lists the slowest nodes. Inference gets a little slower. |
| `TINYBRAINS_TIMINGS_CSV` | A file path. With `--timings` on a match, the time spent in each phase on each turn is also written there as CSV. |

The cache holds:
- `models/weights/` and `models/manifests/`: every model a match or `check` read, stored under its
  sha256
- `cartridges/<digest>/`: unpacked releases
- optionally, a `registry.toml`

## `env`: the training environment

```sh
tinybrains env --maps basic-tiny-2p,basic-small-3p --waves 4 --matches-per-wave 16
```

`env` puts the real cartridge behind JSON Lines on stdin and stdout, one object per line. The first
line out is `hello`, which carries:
- the engine digest and the evaluator
- the boards
- the game's limits and budgets
- the run's settings

The requests are:
- `{"op":"observe"}` returns every live seat's observation and, by default, every live match's
  score.
- `{"op":"step","actions":[...]}` advances every wave one turn. It answers like `observe`, and
  also lists in `ended` the matches that just finished.
- `{"op":"close"}` stops.

**Actions** are positional, one per seat in the order the last `observe` returned. Each action is
either an array of order strings, or a string with one character per ant in `mine` order. Any
character except `N`, `E`, `S` and `W` holds.

**Errors are fatal.** An error prints one `{"ok":false,"error":...}` line and exits 1. Diagnostics
go to stderr.

| Flag | Default | Meaning |
|---|---|---|
| `--game SLUG` | first registry entry | |
| `--maps IDS\|DIR` (or `--map`) | every board the release ships | Comma-separated board ids or `.json` paths, or a directory of boards. Each wave plays one board, and waves take the boards in turn from where `--seed` starts them. |
| `--waves N` | 4 | Independent waves, played in parallel across cores. A wave is refilled when all its matches end. |
| `--matches-per-wave N` | 16 | |
| `--max-turns N` | the game's `max_turns` | |
| `--seed N` | 1 | The root seed. Every match seed and board choice descends from it. |
| `--scores every\|end` | `every` | `every` reports each live match's score on every turn. That costs one extra `finish` call per wave per turn, about 29% of throughput. `end` reports scores only when a match ends. |

**`env` is not the referee.** It has no turn deadline, no strike ceiling, no forfeits and no
adapter evaluation, so a result from it is not a result. `tinybrains check` and a played match are
the gates. Scores are there for the trainer's reward, and they never reach a model's input.

## Building from source

```sh
cargo build --release                               # rust-toolchain.toml pins rustc exactly
cargo fmt --check
cargo clippy --locked --release -- -D warnings
```

`cargo test` runs the unit tests: the memory carry, its pricing and verdicts, and the season-board
name rule. On every push to `main` and on every pull request, `.github/workflows/check.yml`:
- runs format, lint, the unit tests and a locked build
- clones [ants-starter](https://github.com/Tiny-Brains/ants-starter) and runs `games`, `check`
  and the starter's self-play match with the new binary, which also exercises the release fetch
- plays `fixtures/memflaky/match.json` and runs `conform` over `fixtures/memflaky/replay.json`,
  the envelope Kalam's own match loop wrote for that match on the release the starter pins

For `src/wave.rs`, the check that matters is `tinybrains conform` on a replay Kalam wrote, and the
fixture is one: a seat with a memory, struck from turn 3 and forfeited at the ceiling. A new ants
release in the starter's `games.toml`, or a byte changed in the fixture's graphs or manifests, is
a re-record with `scripts/record-replay.py` (its header says what it needs).

To play against a local cartridge build, write a registry whose entry is
`path = "<ants checkout>/dist"`, and point `TINYBRAINS_REGISTRY` at it.

## Releasing

```sh
# bump `version` in Cargo.toml, commit to main, push, then:
gh workflow run release.yml                  # rehearsal: every target built and played, nothing published
git tag v0.2.0 && git push origin v0.2.0     # the release
```

A `v*` tag runs `.github/workflows/release.yml`, which:

1. refuses a tag that disagrees with `Cargo.toml` or is not on `main`
2. builds each target natively on its own runner and plays the starter kit with each binary
3. publishes the archives and `SHA256SUMS` on a GitHub release
4. renders `Formula/tinybrains.rb` with `scripts/formula.sh` from the `SHA256SUMS` it downloads
   from that release, and commits it to `main`
5. installs from the tap on macOS and runs the formula's test

Things to know before you release:
- **Never re-cut a tag.** The formula and every pinned download name the archive's sha256. A bad
  release is a new version. Rehearse first.
- **This repository is the Homebrew tap.** `Formula/` is written by the workflow only, and a hand
  edit is overwritten by the next release.
- **The formula commit starts no workflow**, because it is pushed with `GITHUB_TOKEN`. If branch
  protection refuses that push, re-run the `formula` job, not the tag.
- **The book pins its own CLI version.** `web/docs/Dockerfile` downloads the Linux archive at
  `CLI_VERSION`. Bump it there when the lessons need a newer CLI.

## Layout

```text
src/main.rs            dispatch and usage
src/cmd/               one file per command
src/cartridge.rs       the component, hosted through wasmtime; knows no game
src/wave.rs            the wave loop: the one copy of Kalam's
src/env.rs             the training environment: a pool of waves, no referee
src/matchfile.rs       the match-file format's specification
src/memory.rs          a seat's memory: the carry as live tensors, the admit clock's pricing
src/model.rs, onnx.rs  datalogic adapters, tract graphs, the head decode, graph stats
src/registry.rs        games.toml: a path or a pinned release
src/store.rs           the content-addressed cache
src/timing.rs          --timings
src/serve.rs           the local server `view` uses
wit/                   the plugin ABI the component exports
fixtures/memflaky/     two hand-built graphs, their match file, and the replay Kalam's loop wrote for it
scripts/record-replay.py  re-records that replay through kalam's workflow on a cartridge release
scripts/formula.sh     renders the Homebrew formula from a release's SHA256SUMS
Formula/               the tap, written by the release workflow
.github/workflows/     check.yml on every push; release.yml on a v* tag
```

## Invariants

- **The CLI knows no game.** It never links an engine crate or names a cartridge's types. A second
  game is a registry entry, not Rust.
- **A local result and a ladder result are the same match.** `src/wave.rs` is the only copy of
  Kalam's wave loop. A change to Kalam's claim, strike, forfeit, rank or memory rules is a change
  here in the same batch, and `conform` shows whether it was made: CI runs it over the replay in
  `fixtures/memflaky/`, and a replay from the ladder is the same check.
- **The memory rules are Soma's and Kalam's.** The verdict codes, the dtype widths, the named
  dimension limits and the bounds on the class numbers (`FLAT_BYTES_MAX`, `CELL_BYTES_MAX`) in
  `src/memory.rs` copy the admit clock and `weight_classes_ok()`. The name rule in `maps check`
  copies `season_map_name_problem()`. Change them together.
- **The binary carries no path of the machine that built it.** There is no built-in registry and
  no `CARGO_MANIFEST_DIR` lookup.
- **The archive names are an interface.** Archives are named `tinybrains-<target>.tar.gz`
  (`.zip` on Windows), with no version in the name and the binary at the top. The formula,
  `releases/latest/download/` links, ants-starter's CI, `web/docs/Dockerfile` and the book's
  install lines all depend on these names.
- **`dataflow-rs` tracks what orion-server links.** A node evaluates an adapter on the datalogic
  engine its dataflow-rs engine holds, and this binary borrows its evaluator the same way, so the
  templating mode, the `$` key escape and the operator families are dataflow-rs's settings on both
  sides. A version or feature skew means a local pass and a remote refusal, or two different
  tensors. `Cargo.toml` carries orion-server's exact line, and `DATALOGIC_VERSION` in
  `src/model.rs` is what `games` prints; bump them together, and only with Orion.

## Known gaps

- **Orion's own operators are refused, not run.** A node registers ten operators of Orion's (the
  `base64`, `base64url`, `hex` and `url` codecs, `join` and `random`) and screens `secret`, `now` and
  `random` out of every adapter. They live in orion-server, which this binary does not link, so
  `src/model.rs` lists them and refuses an adapter that names one: in templating mode a missing
  operator reads as data, which would be a different tensor rather than an error. The lists change
  with an Orion upgrade until Orion shares the operators and the screen.

- **The seats of a turn are inferred one after another.** `Model` is `Send + Sync`, so a worker per
  seat only needs the run's model cache (`Models`, which uses `Rc`/`RefCell`) made thread-safe.
- **The release profile is `opt-level = 2`.** `opt-level = 3` with fat LTO measured about 5% faster
  graph time, but it added about 12 minutes to the build.
- **The compiled component is not cached between runs.** Every process compiles the cartridge
  again, which takes about 120 ms on an M2 Pro.

## License

Apache-2.0: see [LICENSE](LICENSE).
