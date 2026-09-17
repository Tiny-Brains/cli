# cli

`tinybrains` is one binary that plays a TinyBrains match on a laptop, with no Compose, no database
and no season. It loads a game's cartridge component through wasmtime, evaluates a manifest's
adapters through **datalogic** and its graph through **tract** — the two libraries an Orion node
itself links — and writes the same replay envelope Kalam writes.

## Install

```sh
brew tap tiny-brains/cli https://github.com/Tiny-Brains/cli
brew install tiny-brains/cli/tinybrains
```

Or take the archive for your platform from the
[latest release](https://github.com/Tiny-Brains/cli/releases/latest), check it against that
release's `SHA256SUMS`, and put `tinybrains` on your `PATH`:

```sh
curl -fsSLO https://github.com/Tiny-Brains/cli/releases/latest/download/tinybrains-aarch64-apple-darwin.tar.gz
tar -xzf tinybrains-aarch64-apple-darwin.tar.gz tinybrains && sudo mv tinybrains /usr/local/bin/
```

| Archive | Platform |
|---|---|
| `tinybrains-aarch64-apple-darwin.tar.gz` | macOS, Apple silicon |
| `tinybrains-x86_64-apple-darwin.tar.gz` | macOS, Intel |
| `tinybrains-aarch64-unknown-linux-gnu.tar.gz` | Linux arm64, glibc 2.35 or newer |
| `tinybrains-x86_64-unknown-linux-gnu.tar.gz` | Linux x86-64, glibc 2.35 or newer |

A file fetched by a **browser** on macOS is quarantined and refused as unsigned; `curl` and Homebrew
are not affected, and `xattr -d com.apple.quarantine tinybrains` clears it. From source, with a Rust
toolchain: `cargo install --locked --git https://github.com/Tiny-Brains/cli`.

Then run it from a game's starter kit, which carries the `games.toml` it reads:

```sh
git clone https://github.com/Tiny-Brains/ants-starter && cd ants-starter
tinybrains games                               # what is registered, at which engine digest
tinybrains check model.onnx manifest.json      # what admission will say
tinybrains matches/self-play.json              # play it; one replay per row
tinybrains view replays/self-play.json         # watch it
```

## Scope

**It owns**

- The `tinybrains` binary: playing a match file, `check`, `adapt`, `conform`, `view`, `maps`, and
  `env`, the cartridge as a training environment.
- The one Rust copy of Kalam's wave loop (`src/wave.rs`), and `conform`, which proves it agrees.
- The match-file parser (`src/matchfile.rs`), which is the format's only specification.
- Resolving a game from a registry: a `path` to an artifact set on disk, or a pinned `release`.
- Its release: the binaries, the Homebrew formula in `Formula/`, and the artifact image.

**It does not**

- Know any game. It knows five function names, `cartridge.json` and the replay envelope; every
  board, preset, seat count and limit is read from the manifest.
- Decide anything the ladder decides. `check` is necessary and not sufficient: there is no download
  allowlist here, and the size class is reported, never assigned — that table is Soma's.
- Carry a registry. A project carries its own `games.toml`; a
  [starter kit](https://github.com/Tiny-Brains/ants-starter) is where one comes from.

## Where it sits

```text
competitor's repository                          GitHub releases
  games.toml ─────── pins ──────────────────▶  Tiny-Brains/ants  engine-<12 hex>
  matches/*.json                                  ants-artifacts.tar.gz
  model.onnx, manifest.json                            │ fetched once, both digests checked
        │                                              ▼
        └──────────▶  tinybrains  ◀────── ~/.cache/tinybrains/cartridges/<digest>/
                          │
                          ▼
                  replays/*.json  (Kalam's envelope; `view` draws it, `conform` diffs it)
```

| Direction | Party | Over | What moves |
|---|---|---|---|
| reads | A project's `games.toml` | the working directory, or `TINYBRAINS_REGISTRY` | Which games exist and where their artifacts are |
| fetches | [Ants](https://github.com/Tiny-Brains/ants) releases | HTTPS | The cartridge's artifact set, pinned by two digests |
| fetches | A match file's seats | a path or HTTPS | Weights and manifests, stored under their sha256 |
| writes | the working directory | files | One replay per row |

Who runs it: [ants-starter](https://github.com/Tiny-Brains/ants-starter)'s CI and README, the
book's quickstart and *Testing* chapter in [web/docs](https://github.com/Tiny-Brains/web/tree/main/docs),
`ants/baselines` (`env` for training, `adapt` for the conformance test), and `web/docs`' image
build, which takes the binary from this repository's image (`CLI_REF`) to play the lesson replays.
[DevOps](https://github.com/Tiny-Brains/devops) builds that image from a checkout of this
repository (`CLI_DIR`) and runs nothing else of it.

## Interface

```text
tinybrains <match.json> [--out DIR] [-v]   play a wave; write one replay per row
tinybrains games                           what is registered, and at which digest
tinybrains maps [GAME]                     the boards a game is played on
tinybrains maps export [GAME] [DIR]        write those boards out as files
tinybrains view <replay.json>              watch it in a browser
tinybrains check <model.onnx> <manifest>   would this be admitted?  [--json]
tinybrains adapt <model.onnx> <manifest>   dump the tensors an adapter produces
tinybrains conform <replay.json>           replay a recorded match here, and diff
tinybrains env [...]                       the cartridge as a training environment
tinybrains --version
```

**The registry** is the first of `$TINYBRAINS_REGISTRY`, `./games.toml`, `./tinybrains.toml` and
`~/.cache/tinybrains/registry.toml` that exists. An entry is either

```toml
[games.ants]
name = "Ants"
path = "../ants/dist"            # an artifact set on disk: a checkout's dist/, or an image's /artifacts/
```

— the digest is whatever the component hashes to, and is reported — or a pinned release, as
`ants/tools/release.sh` prints it and every starter kit carries it:

```toml
[games.ants]
name = "Ants"
repo = "Tiny-Brains/ants"
release = "engine-<12 hex>"
artifacts = { file = "ants-artifacts.tar.gz", sha256 = "sha256:<64 hex>" }
engine = "sha256:<64 hex>"       # == games.active_engine_digest
```

**The match file** is `K_WAVE`'s rows plus the Orion `[vars]` they run under, so a real claim can be
dumped to a file and replayed here; a seat may also name `weights`/`manifest` as a path or a URL, or
be scripted (`"script": ["E", "E", "-"]`). The book's *Testing* §Match files is the competitor-facing
copy of `src/matchfile.rs`.

`TINYBRAINS_HOME` moves the cache (models and cartridges) off `~/.cache/tinybrains`.

**What is identical to production**, and this is the point: the component (same file, same digest),
the evaluator (datalogic, the version `tinybrains games` prints), the runtime (tract), the head
decode, `cartridge.json`, the boards, and the envelope. What differs is config — a directory model
store instead of S3, a file instead of a presigned PUT, and rows from a file instead of a claim
under a lease. **Where it is a copy and not the thing** is the wave loop: Kalam expresses it as an
Orion workflow and `src/wave.rs` as Rust, which is why `conform` exists.

## Run it, test it

```sh
cargo build --release                              # rust-toolchain.toml pins the compiler
cargo fmt --check && cargo clippy --locked --release -- -D warnings
cd ../ants-starter && ../cli/target/release/tinybrains matches/self-play.json   # a registry to run against
docker build -t tinybrains/cli:dev .               # the artifact image: /artifacts/bin/tinybrains
```

There are **no unit tests**. `.github/workflows/check.yml` runs format, lint and a locked build on
every push, then plays `ants-starter` with the binary it just built — `games`, `check`, and its
self-play match — which also exercises the release fetch. The check that matters for `src/wave.rs`
is `tinybrains conform <replay>` against a replay the ladder wrote: it rebuilds the match from the
envelope alone, plays it here, and diffs every field and every turn of the action stream.

To play against a local cartridge build, point `TINYBRAINS_REGISTRY` at a registry whose entry is a
`path` to that checkout's `dist/`. `tinybrains games` always says which way it resolved.

### Releasing

```sh
# bump `version` in Cargo.toml, commit to main, push, then:
git tag v0.2.0 && git push origin v0.2.0
```

`.github/workflows/release.yml` refuses a tag that disagrees with `Cargo.toml` or is not on `main`;
builds the four targets (the Intel macOS one cross-compiled on Apple silicon) and plays the starter
kit with each binary its runner can execute; publishes the archives and `SHA256SUMS` on a GitHub
release; renders `Formula/tinybrains.rb` from the checksums **it downloads from that release** with
`scripts/formula.sh` and commits it to `main`; then installs through Homebrew from the tap and runs
the formula's test.

## What a deployment owes it

- **Its `datalogic-rs` tracks what orion-server links.** An adapter is priced by datalogic on a
  node and here; a version skew is a local pass and a remote refusal. `tinybrains games` prints the
  version, and `DATALOGIC_VERSION` in `src/model.rs` is the string it prints.
- **A cartridge release is never re-cut.** A registry pins the archive's digest, so a replaced file
  is a refusal on every clone that pinned it.
- **Kalam's replay envelope and wave rules move with `src/wave.rs`.** A change to Kalam's claim,
  strike, forfeit or rank rules is a change here in the same batch, and `conform` is what shows
  whether it was made.

## Layout

```text
src/main.rs            dispatch and usage
src/cmd/               one file per verb
src/cartridge.rs       the component, hosted through wasmtime; knows no game
src/wave.rs            the wave loop: the one allowed copy of Kalam's
src/env.rs             the training environment: a pool of waves, deliberately no referee
src/matchfile.rs       the match-file format's only specification
src/model.rs, onnx.rs  datalogic adapters, tract graphs, the head decode
src/registry.rs        games.toml: a path or a pinned release
src/store.rs           the content-addressed cache under ~/.cache/tinybrains
src/serve.rs           `view`'s local server for the cartridge's own viewer
wit/                   the plugin ABI the component exports
scripts/formula.sh     the Homebrew formula, rendered from a release's SHA256SUMS
Formula/               the tap: written by the release workflow, never by hand
Dockerfile             the artifact image web/docs builds from
.github/workflows/     check.yml on every push; release.yml on a v* tag
```

## What must stay true

- **The CLI knows no game.** It never links an engine crate and never names a cartridge's types; a
  second game is a registry entry. If that stops being true the seam has quietly moved.
- **A local result and a ladder result are the same match.** `tinybrains conform` is the check, and
  `src/wave.rs` is the only place a copy of Kalam's loop is allowed to live.
- **`env` is not a referee.** No deadline, no strike ceiling, no forfeits, no adapter evaluation —
  which is why its actions can be positional, and why a result from it is not a result.
- **The binary carries no path of the machine that built it.** No built-in registry, no
  `CARGO_MANIFEST_DIR` lookups: a released binary runs on someone else's laptop.
- **The archive names are an interface.** `tinybrains-<target>.tar.gz`, flat, with `tinybrains` at
  the top: the formula, `releases/latest/download/` links and the book's install lines name them.
- **A tag is a version and is never re-cut.** `v<version>` equals `Cargo.toml`, and a bad release is
  a new version, because the formula and every pinned download name the archive's sha256.

## Status

**17 September 2026 — the CLI is a repository of its own, and ships binaries.** It moved out of
`devops/cli` with its history (`git filter-repo`, 25 commits), because nothing in it was deployment:
the stack never ran it, its one devops coupling was a built-in fallback to `../games/registry.toml`
that baked the build machine's path into every binary, and a competitor needed a Rust toolchain to
install it. That fallback is gone — a registry is the project's, and `devops/games/registry.toml`
is now read only where it is named. `tinybrains --version` is new. A `v*` tag now publishes macOS and Linux binaries for arm64 and
x86-64 and a Homebrew formula in this repository's own tap; ants-starter's CI downloads the release
instead of compiling. The artifact image is unchanged and still what `web/docs` builds from; DevOps
builds it from `CLI_DIR`. Earlier history is in this repository's log and in DevOps' README Status.

## More

- [DevOps](https://github.com/Tiny-Brains/devops) — `docs/decisions.md` N23 is why this repository exists; 47 is why `env` is a verb.
- [ants-starter](https://github.com/Tiny-Brains/ants-starter) — where to run it from.
- Apache-2.0: see [LICENSE](LICENSE).
