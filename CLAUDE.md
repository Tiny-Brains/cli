# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

`cli` is the `tinybrains` binary, one Rust crate. It:
- plays a TinyBrains match on a laptop
- makes admission's own measurements (`check`, `adapt`)
- replays a ladder match to prove the two agree (`conform`)
- judges season boards (`maps check`)
- hosts a cartridge as a training environment (`env`)

It builds, checks and releases from this repository alone. It links no sibling and reads no path
outside itself. `README.md` covers commands, the registry, environment variables and releasing.
`../CLAUDE.md` is the platform map.

## Checks

```sh
cargo fmt --check                                      # rustfmt.toml: max_width 100
cargo clippy --locked --release -- -D warnings         # CI gates on this
cargo build --release                                  # rust-toolchain.toml pins rustc exactly

# then run it where a registry is: the starter kit carries one
cd ../ants-starter
../cli/target/release/tinybrains games
../cli/target/release/tinybrains check model.onnx manifest.json
../cli/target/release/tinybrains matches/self-play.json
../cli/target/release/tinybrains conform replays/self-play.json
```

There are no unit tests. For `src/wave.rs` the check that matters is `tinybrains conform` on a
replay the ladder wrote. After a dependency change, run `cargo tree -e normal,build --target all -d`
to look for new duplicates.

## Rules

- **It knows no game.** It never links an engine crate and never names a cartridge's types. What
  it knows:
  - the cartridge's function names
  - `cartridge.json`
  - the replay envelope

  It finds the component by its `.wasm` extension. It reads every board, seat count and limit from
  the manifest or the board. A second game is a registry entry and no Rust.
- **There is no built-in registry, and there must not be one.** Nothing the binary looks up may
  come from where it was built (`CARGO_MANIFEST_DIR` or any `env!` path). In a released binary,
  that path is a CI runner's directory on someone else's laptop.
- **Every `<game>-starter` pins a release and must work with no sibling checkout.** A registry
  feature that only works from a `path` entry is one competitors cannot use.
- **`src/wave.rs` is the one allowed second implementation** of Kalam's wave loop, and its module
  doc names the behaviours it copies. Kalam's wave rules (`kalam/scripts/gen-kalam.py`) and
  `wave.rs` change together. `conform` diffs every field and every turn. A difference in `deltas` is
  the one that matters, because ranks and scores can agree while the matches that produced them
  differed.
- **`src/env.rs` is not a third match loop.** It has no deadline, strikes, forfeits, model call or
  replay envelope.
  - Its positional actions are correct only because it never forfeits a seat. If it ever needs
    Kalam's explicit `{m, seat, action}` form, that means it has grown a referee rule.
  - The live scores it reads out of `finish` are the trainer's reward. They must never reach a
    model's input.
  - stdout is its protocol, so every diagnostic goes to stderr.
- **`src/matchfile.rs` is the match-file format's only specification.** The book's *Testing* page,
  section Match files (`web/docs/src/models/testing.md`), is its competitor-facing copy, and nothing
  compares the two. A field added or derived differently here is an edit there too.
- **Every board goes to `worldgen` whole.** The component carries no boards. `MatchFile::resolve_boards`
  and `env::boards_of` turn an id, a `.json` path or an inline board into the board. A row that
  names a `preset` is refused, never ignored.
- **`src/timing.rs` has two groups, and they are not one list.**
  - `Registry`..`Write` are disjoint and sum to the run. A phase added there must be taken out of
    another, or the breakdown sums to more than the wall clock.
  - `Instantiate`..`DecodeOut` double-count on purpose.
  - `infer_us` measures `plan.run` alone.
- **`DATALOGIC_VERSION` in `src/model.rs` tracks what orion-server links.** It is the string that
  `games` and `env`'s hello print. `Cargo.toml`'s `datalogic-rs` is what actually runs. Bump them
  together, and only with Orion.
- **Dependencies are the minimum, with features gated** (`default-features = false`). Before adding
  a crate:
  - look for it in `cargo tree -i`
  - look for a re-export (datalogic re-exports `bumpalo` and `datavalue`)
  - never take a command-line or tooling crate for one helper
- **Other repos consume this binary.**
  - `registry.rs`, `serve.rs` and `cmd/` read the cartridge's artifact-set layout: the component at
    the root, `cartridge.json`, `maps/`, `reference/` and `viz/`. A rename inside `ants/dist` is a
    change here too, and it reaches competitors only through an ants release and a CLI release
    together.
  - ants-starter's `check.yml` downloads the latest Linux archive.
  - `web/docs/Dockerfile` downloads it at a pinned `CLI_VERSION`.
  - `ants/baselines` finds the binary on `PATH` or through `$TINYBRAINS`.
  - The book (`quickstart.md`, `models/testing.md`) and web's `/start` page carry the install
    lines.

## Gotchas

- **This repository is the Homebrew tap.** `brew tap` needs the URL, because the repository is not
  named `homebrew-cli`. `Formula/` is written only by the release workflow. A wrong sha256 there is
  a checksum error on every install.
- **Rehearse before tagging** (`gh workflow run release.yml`). A tag is never re-cut, so a tag that
  fails half-way is a version number spent.
- **The archive name and shape are an interface.** Archives are named `tinybrains-<target>.tar.gz`
  or `.zip`, with no version in the name and the binary at the top level.
- **Windows is a target, so paths are not strings.**
  - `view` refuses `:` and `\` in a request path, because on Windows `Path::join` lets either
    escape the viewer's directory.
  - The model store names files `sha256-<hex>`, because `:` is not legal in a Windows filename.
  - Windows is exercised only by the release workflow.
- **The formula commit starts no workflow.** It is pushed with `GITHUB_TOKEN`, so `check.yml` does
  not run on it. If branch protection refuses the push, re-run the `formula` job, not the tag.
- **There is no Intel macOS build, no 32-bit build and no Docker image.** The formula refuses other
  Macs with `depends_on arch: :arm64` and `depends_on macos: :tahoe`.
- **The cache is not `~/.cache` on macOS.** It is `~/Library/Caches/tinybrains`, and
  `TINYBRAINS_HOME` overrides it.
