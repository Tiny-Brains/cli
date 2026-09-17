# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`cli` is **the `tinybrains` binary**: one Rust crate that plays a TinyBrains match on a laptop,
makes admission's own measurements, replays a ladder match to prove agreement, and hosts a
cartridge as a training environment. It is one of the repos checked out side by side under
`tinybrains/`; see `../CLAUDE.md` for the platform map. **It is self-contained**: it builds, checks
and releases from this repository alone, links no sibling, and reads no path outside itself.

It lived at `devops/cli` until 17 September 2026 and moved here with its history
(`devops/docs/decisions.md` N23). A dated record elsewhere that says `devops/cli` means this
repository. `README.md` is the operator's page and its **Status** block holds the open work.

## Commands

```sh
cargo build --release                                  # rust-toolchain.toml pins rustc exactly
cargo fmt --check                                      # rustfmt.toml: max_width 100
cargo clippy --locked --release -- -D warnings         # CI gates on this
docker build -t tinybrains/cli:dev .                   # the artifact image web/docs builds from

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
— a `path` to an artifact set on disk (a checkout's `dist/`, or an image's `/artifacts/` copied
out; the digest is *reported*), or a `release`: that same tree as one `.tar.gz` on a GitHub release,
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

**`DATALOGIC_VERSION` in `src/model.rs` must track what orion-server links.** An adapter is priced
by datalogic on a node and by datalogic here, so a skew is a local `check` pass and a remote
refusal. The constant is what `games` and `env`'s hello print; `Cargo.toml`'s `datalogic-rs` is what
actually runs. Bump them together, and only with Orion.

## Releasing

A `v*` tag runs `.github/workflows/release.yml`: refuse a tag that disagrees with `Cargo.toml` or is
not on `main` → build four targets (`aarch64`/`x86_64` × `apple-darwin`/`unknown-linux-gnu`; Intel
macOS is cross-compiled on the Apple-silicon runner, Linux is built on 22.04 for glibc 2.35) → play
`ants-starter` with each binary its runner can execute → publish `tinybrains-<target>.tar.gz` and
`SHA256SUMS` on a GitHub release → render `Formula/tinybrains.rb` with `scripts/formula.sh` from the
**downloaded** `SHA256SUMS` and commit it to `main` → `brew tap` this repository, install, and run
the formula's test.

Things that break silently if forgotten:

- **This repository is the Homebrew tap.** `brew tap tiny-brains/cli https://github.com/Tiny-Brains/cli`
  — the URL is required because the repository is not named `homebrew-cli`. `Formula/` is written
  by the workflow only; a hand edit is overwritten by the next release, and a wrong sha256 is a
  checksum error on every install.
- **Never re-cut a tag.** The formula, the release's `SHA256SUMS` and anyone's pinned download name
  the archive's digest. A bad release is a new version.
- **The archive name and shape are an interface.** `tinybrains-<target>.tar.gz`, no version in the
  name, `tinybrains` at the top level. The formula's `bin.install`, `releases/latest/download/`
  links, ants-starter's CI and the book's install lines all rely on it.
- **`rust-toolchain.toml` and the Dockerfile's `RUST_VERSION` are one pin written twice.** Bump both.
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
- **Consumers of the binary**: ants-starter's `check.yml` downloads the release archive for
  `x86_64-unknown-linux-gnu`; `web/docs/Dockerfile` copies `/artifacts/bin/tinybrains` out of
  `CLI_REF`, which devops' compose builds from `CLI_DIR` (default `../cli`); `ants/baselines` finds
  it on `PATH` or through `TINYBRAINS`. The book (`web/docs/src/quickstart.md`, `models/testing.md`)
  and web's `/start` page carry the install lines.
