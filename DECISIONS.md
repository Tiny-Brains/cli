# Decisions — the `tinybrains` CLI

Why the `tinybrains` CLI is shaped the way it is: the part of TinyBrains' decision record about this
repository. One line per decision, with the reasoning kept and the cost of flipping it named where
that was worked out.

> **The record was one file until 17 September 2026**, `devops/docs/decisions.md`. When devops
> stopped running anything (N25) it was split, so each decision lives in the repository it is
> about. **The numbers are the record's, not this file's**: they were assigned once across the
> platform and are never reused, so a citation of `41` or `N24` names one decision wherever it now
> lives, and a section number below is the one the whole record gave it.
>
> **Four numbering series.** The **A-series** is the twenty-one architectural decisions taken
> before anything was built. The **plain series** is the build decisions the layers took, numbering
> from 1 again, so `A5` and `5` are different decisions and a bare number in a code comment means
> the plain series. The **R-series** is the Orion 1.8.1 rebuild and the **N-series** the runner, the
> submission path and where each repository's artifacts come from.

## Where the rest of the record is

| Decisions | Where they live |
|---|---|
| **A1–A21**, §2's review findings and the Orion changes asked for | [soma](https://github.com/Tiny-Brains/soma/blob/main/docs/decisions.md) |
| Plain series: the match table (2, 3, 7, 7c, 7d, 18, 21, 22), the clocks (1, 7–13, 23, 24, 28, 51–59), admission (20, 35–40), the retired loader (6, 34, 46, the adapter cap) and deployment 43, 44, 48 | [soma](https://github.com/Tiny-Brains/soma/blob/main/docs/decisions.md) |
| Plain series: the wave (19, 33) and deployment 5, 25, 41, 42, 45 | [kalam](https://github.com/Tiny-Brains/kalam/blob/main/docs/decisions.md) |
| Plain series: the game and the protocol (4, 14–16), the baselines (49, 50 of the loader's) | [ants](https://github.com/Tiny-Brains/ants/blob/main/DECISIONS.md) |
| Plain series: the training environment (47, 48 of the loader's) | [cli](https://github.com/Tiny-Brains/cli/blob/main/DECISIONS.md) |
| Plain series: deployment 47 and 49 (the compose file's) | [web](https://github.com/Tiny-Brains/web/blob/main/DECISIONS.md) |
| **R1, R2, R4, R6, R9, R10, R11** · **R3, R7, R8** · **R5** | soma · kalam · ants |
| **N3, N6–N8, N12, N13, N15–N19** · **N1, N2, N4, N5, N9** · **N20–N22, N24** · **N23** · **N25** | soma · kalam · ants · cli · web |
| Still open | the repository each is forced in: 30, 31, N14 and three unnumbered in soma; N10 and a runner on another network in kalam; 32 in ants; 26, 27, N11 and the orchestrator in web |

The plain series collides with itself once: the retired loader's **47, 48, 49** and deployment's
**47, 48, 49** are different decisions, told apart above by where each lives.

---

## 3. Build decisions

Numbered as the build numbered them. Each names the repository it now lives in.

### The loader — ~~`axon`~~, **superseded 14 September 2026**

> **Every decision in this section was taken about a service that no longer exists**, and the
> checkout is gone from the working tree. Orion 1.8.1's `models` entity replaced it whole — §4's
> R-series is what replaced each one. They are kept because a
> decision log that deletes what it superseded cannot be read backwards: **46** (there is no compute
> cap) and **49** (the baselines are a repository of their own) still stand on their own arguments,
> and the rest are the reasoning the R-series answers. Where an entry below and the R-series
> disagree, the R-series is what runs.

| # | Decision | Taken as |
|---|---|---|
| **47** | **The training environment is a CLI verb, and deliberately not a referee** | `tinybrains env` hosts the cartridge as a pool of waves over JSON Lines. It lives in `devops/cli` because that is the only place that already hosts a component, links axon as a library and resolves games from the registry; a second host in a trainer's repository would be a second wave loop, which is what `conform` exists to prevent. It has no deadline, no strike ceiling, no forfeit and no adapter evaluation — which is exactly why its actions can be *positional*, the protocol's own form, correct here only because a training env never forfeits a seat. Needing Kalam's explicit `{m, seat, action}` form is the symptom of a rule it does not have. Measured on an M2 Pro: 4,900 seat-turns/s with real colonies |
| **48** | **A dense per-turn reward needs no cartridge change** | `f_finish` gates only `map` on `done` and answers `scores`, `ranks` and `turns` for a match still running, so an exact per-turn score costs one extra invocation (29% of env throughput, declinable with `--scores end`). The trainer sees it and the policy never does, which is asymmetric actor-critic rather than a hole in the fog: the value head that consumes it is discarded before export |

---

## 4b. The N-series — a runner leaves the deployment, and GitHub leaves the submission path

Taken and built 16 September 2026, as two tracks decided together because each removed a dependency
that was not earning its place. They shared one thread — the models bucket, which lets a runner read
artifacts without a secret and is the submission path's audit trail — and no file. The proposal and
its phased plan (`docs/design.md`, `docs/design-plan.md`) were deleted once they were all record;
what they argued is here and in [`architecture.md`](https://github.com/Tiny-Brains/soma/blob/main/docs/architecture.md) §3a, the statements and routes
are `soma/docs/schema.md` §3.8a, §4 and §4a, the operator's page is [`deployment.md`](https://github.com/Tiny-Brains/kalam/blob/main/docs/deployment.md)
§11, and what each phase turned up is in the Status blocks of `soma`, `kalam`, `devops` and `web`.
N10, N11 and N14 are still open, in §5.

### The CLI's repository

| # | Question | Decision | What it overturns, and what it cost |
|---|---|---|---|
| N23 | Is the `tinybrains` CLI part of the deployment repository? | **No. `devops/cli` moves to `Tiny-Brains/cli`** with its history (`git filter-repo --subdirectory-filter cli`, 25 commits), and **ships binaries, not an image**: a `v<version>` tag builds five 64-bit targets natively — `aarch64-apple-darwin` (macOS 26 or newer), `{aarch64,x86_64}-unknown-linux-gnu` and `{aarch64,x86_64}-pc-windows-msvc` — plays ants-starter with each, publishes the archives and `SHA256SUMS` on a GitHub release, and commits a Homebrew formula rendered from those checksums into the same repository, **which is the tap** (`brew tap tiny-brains/cli https://github.com/Tiny-Brains/cli`). No Intel macOS and no 32-bit builds. The binary loses its built-in registry — `CARGO_MANIFEST_DIR/../games/registry.toml`, a build-machine path compiled into every binary — so a registry is always the project's. ants-starter's CI downloads the release; the book and web's `/start` say `brew install`; `web/docs/Dockerfile` downloads the Linux release at a pinned `CLI_VERSION`, so the `cli` build service, the `tools` profile, `CLI_DIR` and `CLI_REF` go. `games/registry.toml` stays here, named by `submission-storm.py` | Decision **47**'s "it lives in `devops/cli`", and N21's closing cost, "a competitor still needs a Rust toolchain for `cargo install --git` until the CLI ships binaries". Nothing in the CLI was deployment: the stack never runs it, and its only coupling to this repository was that fallback path, which on a released binary names a CI runner's directory. Its artifact image was kept at first and then dropped: a command-line tool is installed rather than run as a container, and the image's one consumer, the book's build, can take the same binary a person installs. A separate `homebrew-tap` repository was the conventional shape and was not taken: it is a second repository whose one file a release must push to with a token, where a tap inside the CLI's own repository is written by the job that knows the checksums. **Cost:** installing needs the tap's URL, because the repository is not named `homebrew-cli`; the formula is a bot commit on `main`; a tag is a version and is never re-cut, so the release workflow is rehearsed by hand first; the book's lessons play a released CLI, so a CLI change reaches them only through a release and a `CLI_VERSION` bump; and a run from this directory needs `TINYBRAINS_REGISTRY=games/registry.toml`, which it used to find by itself |
