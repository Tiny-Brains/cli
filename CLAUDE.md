# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`cli` is **the `tinybrains` binary**: one Rust crate that plays a TinyBrains match on a laptop,
makes admission's own measurements, replays a ladder match to prove agreement, and hosts a
cartridge as a training environment. It is one of the repos checked out side by side under
`tinybrains/`; see `../CLAUDE.md` for the platform map, and `DECISIONS.md` for the decisions that
shaped this binary (47, 48, N23). **It is self-contained**: it builds, checks
and releases from this repository alone, links no sibling, and reads no path outside itself.

It lived at `devops/cli` until 17 September 2026 and moved here with its history
(N23, in `DECISIONS.md`). A dated record elsewhere that says `devops/cli` means this
repository. `README.md` is the operator's page and its **Status** block holds the open work.

## Commands

```sh
cargo build --release                                  # rust-toolchain.toml pins rustc exactly
cargo fmt --check                                      # rustfmt.toml: max_width 100
cargo clippy --locked --release -- -D warnings         # CI gates on this

# run it where a registry is: a starter kit carries one
cd ../ants-starter
../cli/target/release/tinybrains games                 # what is registered, at which digest
../cli/target/release/tinybrains check model.onnx manifest.json
../cli/target/release/tinybrains matches/self-play.json
../cli/target/release/tinybrains view replays/self-play.json
../cli/target/release/tinybrains conform replays/<a ladder replay>.json
../cli/target/release/tinybrains adapt model.onnx manifest.json
../cli/target/release/tinybrains env --waves 4 --matches-per-wave 16

# a release: bump `version` in Cargo.toml, commit to main, push, then
gh workflow run release.yml                            # rehearsal: all five targets built + played, nothing published
git tag v0.2.0 && git push origin v0.2.0
scripts/formula.sh 0.2.0 SHA256SUMS                    # what release.yml renders into Formula/
```

There are **no tests** — `tinybrains conform` is the check (see Architecture). The CI pass is
format, lint, a locked build, and `ants-starter`'s own steps run with the binary just built. Run the
binary from a directory holding a `games.toml`, or set `TINYBRAINS_REGISTRY`; `TINYBRAINS_HOME`
moves the cache off `~/.cache/tinybrains`. To play a local cartridge build, write a registry whose
entry is `path = "<ants checkout>/dist"`.

## Architecture

**It knows no game.** It never links an engine crate and never names a cartridge's types: it knows
five function names, `cartridge.json`, and the replay envelope, and resolves a game from a registry
— a `path` to an artifact set on disk (a checkout's `dist/`, or an unpacked ants release; the
digest is *reported*), or a `release`: that same tree as one `.tar.gz` on a GitHub release,
fetched once into the cache and refused unless the archive and the component hash to the registry's
`artifacts.sha256` and `engine`. **Every `<game>-starter` pins a release, and must work with no
sibling checkout at all** (devops N21, N22) — a registry feature that only works from a `path` is
one competitors cannot use. It hosts the cartridge through wasmtime and evaluates a manifest's
adapters through **datalogic** and its graph through **tract** — the same two libraries an Orion
node uses. Adding a second game is a registry entry and no Rust.

**There is no built-in registry, and there must not be one.** Until the move, `Registry::find`
fell back to `env!("CARGO_MANIFEST_DIR")/../games/registry.toml` — devops' copy — which compiled
the build machine's absolute path into every binary. On a released binary that is a CI runner's
directory on a competitor's laptop. A registry belongs to the project that plays the game; nothing
the binary looks up may be derived from where it was built.

**`src/env.rs` is a training environment and deliberately not a third match loop.** It has the
four cartridge functions and a pool that refills them, and none of Kalam's rules: no deadline, no
strike ceiling, no forfeits, no model call, no replay envelope. That is what lets it not be a copy
that drifts — and it is why its actions are *positional*, which is only correct because a training
env never forfeits a seat. Needing the explicit `{m, seat, action}` form is the symptom of a rule it
does not have. It reads live scores out of `finish` every turn, which the cartridge answers for an
unfinished match; that is the trainer's reward channel and never reaches a model's input.

**`src/wave.rs` is the one place a second implementation is allowed.** Kalam expresses the wave
loop as an Orion workflow of JSONLogic; this expresses it as Rust, and its docstring names the
behaviours it must copy exactly (explicit `{m, seat, action}` form, forfeited seats and seats with
no ants omitted entirely, cumulative strikes, forfeit rank = `engine_rank + seat_count`, flat
`refs`). Copies drift, so the agreement is **checked rather than argued**: `tinybrains conform
<replay>` rebuilds a match from its envelope alone, plays it locally, and diffs every field and
every turn of the action stream. A difference in `deltas` is the one that matters — ranks and scores
can agree while the match that produced them differed.

**`src/matchfile.rs` is the match-file format's only specification.** A match file is the row Kalam
claims (`K_WAVE` in `kalam/scripts/gen-kalam.py`) plus the vars it runs under. The book's *Testing*
§Match files (`web/docs/src/models/testing.md`) is its competitor-facing copy, and no check compares
them: a field added or derived differently here is an edit there too.

**`src/timing.rs` is the run's own clock, and its two groups are not one list.** `Registry`..`Write`
are disjoint and sum to the run; `Instantiate`..`DecodeOut` decompose the cartridge calls among them
and double-count on purpose. Adding a phase to the first group without taking it out of another is
how a breakdown starts summing to more than the clock it came from — `--timings` prints an
`unaccounted` row against the measured wall clock, which is what makes that visible. Recording is
unconditional; only the printing is behind the flag. Note that `infer_us`, which goes into every
replay's `seats` and into `check --json`, is `plan.run` alone and not the adapter that fed it.

**`DATALOGIC_VERSION` in `src/model.rs` must track what orion-server links.** An adapter is priced
by datalogic on a node and by datalogic here, so a skew is a local `check` pass and a remote
refusal. The constant is what `games` and `env`'s hello print; `Cargo.toml`'s `datalogic-rs` is what
actually runs. Bump them together, and only with Orion.

**Dependencies are the minimum, with features gated** (README Status, 17 September). Compile time
is wasmtime + cranelift and tract; everything else in `Cargo.toml` is there for a call the binary
makes, with `default-features = false`. Before adding a crate, look for it in `cargo tree -i` and
in a re-export (datalogic re-exports `bumpalo` and `datavalue`), and never take a command-line or
tooling crate for one helper — `tract-libcli` cost some forty crates for a shape parser. Check a
change with `cargo tree -e normal,build --target all -d` for new duplicates.

## Releasing

A `v*` tag runs `.github/workflows/release.yml`: refuse a tag that disagrees with `Cargo.toml` or is
not on `main` → build five 64-bit targets, each natively on its own runner — `aarch64-apple-darwin`
(`macos-26`, `MACOSX_DEPLOYMENT_TARGET=26.0`), `{aarch64,x86_64}-unknown-linux-gnu` (22.04, so glibc
2.35) and `{aarch64,x86_64}-pc-windows-msvc` (`windows-11-arm`, `windows-2025`) → play
`ants-starter` with every binary → publish `tinybrains-<target>.tar.gz` (`.zip` for Windows) and
`SHA256SUMS` on a GitHub release → render `Formula/tinybrains.rb` with `scripts/formula.sh` from the
**downloaded** `SHA256SUMS` and commit it to `main` → `brew tap` this repository, install, and run
the formula's test. `gh workflow run release.yml` runs the build and play half alone and publishes
nothing — **rehearse before tagging**, because a tag that fails half-way is a version number spent.

**There is no Intel macOS build, and no 32-bit build of anything**, by decision. The formula says so
with `depends_on arch: :arm64` and `depends_on macos: :tahoe` rather than failing on a missing URL.
**There is no Docker image either**: a CLI is installed, not run as a container, and the one Docker
build that needs it (`web/docs`) downloads the Linux release by `CLI_VERSION`.

Things that break silently if forgotten:

- **This repository is the Homebrew tap.** `brew tap tiny-brains/cli https://github.com/Tiny-Brains/cli`
  — the URL is required because the repository is not named `homebrew-cli`. `Formula/` is written
  by the workflow only; a hand edit is overwritten by the next release, and a wrong sha256 is a
  checksum error on every install.
- **Never re-cut a tag.** The formula, the release's `SHA256SUMS` and anyone's pinned download name
  the archive's digest. A bad release is a new version.
- **The archive name and shape are an interface.** `tinybrains-<target>.tar.gz` or `.zip`, no
  version in the name, the binary at the top level. The formula's `bin.install`,
  `releases/latest/download/` links, ants-starter's CI, `web/docs/Dockerfile` and the book's install
  lines all rely on it.
- **Windows is a target, so paths are not strings.** `view` refuses `:` and `\` in a request path
  because `Path::join` on Windows lets either escape the viewer's directory; the model store names
  files `sha256-<hex>` because `:` is not a legal filename there. The release plays the starter kit on
  both Windows runners, and that is the only place Windows is exercised.
- **The formula job pushes to `main` with `GITHUB_TOKEN`.** That push starts no workflow, by
  GitHub's rule, so `check.yml` does not run on formula commits. Branch protection that forbids the
  bot pushing would fail that job after the release is already public; re-run the job, not the tag.

## What crosses a repo boundary

- **Kalam's wave rules** (`kalam/scripts/gen-kalam.py`) and `src/wave.rs` change together; `conform`
  against a fresh ladder replay is the proof.
- **The cartridge's artifact-set layout** — component at the root by extension, `cartridge.json`,
  `maps/`, `reference/`, `viz/` — is read by `registry.rs`, `serve.rs` and `cmd/`. A rename inside
  `ants/dist` is a change here too, and reaches competitors only through a new ants release and a
  CLI release together.
- **Consumers of the binary**: ants-starter's `check.yml` downloads the latest Linux release archive
  for its runner; `web/docs/Dockerfile` downloads the Linux archive for its build platform at a pinned
  `CLI_VERSION` and checks it against that release's `SHA256SUMS` — **bump it there** when the lessons
  need a newer CLI; `ants/baselines` finds it on `PATH` or through `TINYBRAINS`. The book (`web/docs/src/quickstart.md`, `models/testing.md`)
  and web's `/start` page carry the install lines.
