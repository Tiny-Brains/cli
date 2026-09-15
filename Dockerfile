# syntax=docker/dockerfile:1

# The `tinybrains` CLI as an ARTIFACT IMAGE: one binary that plays a match on a laptop.
#
# WHY THIS EXISTS. Three repositories need this binary and none of them should need a Rust
# toolchain to get it: `docs` regenerates the book's lesson replays with it, and `drill` and
# `ants-baselines` are competitor-facing and run it to play matches.
#
# IT LINKS NO SIBLING ANY MORE. It used to need a named build context for `axon`, because
# `Cargo.toml` said `path = "../../axon"` and that is outside this build context. With axon deleted
# the crate links the two libraries a node itself links -- datalogic for the adapters, tract for the
# graph -- both from crates.io, so this is an ordinary build and `cargo install --git` works for
# anyone.

ARG RUST_VERSION=1.98.1
ARG BUSYBOX_VERSION=1.37-musl

# ---- build -------------------------------------------------------------------
#
# Trixie rather than bookworm. `ort` is gone with axon, so the old reason (its prebuilt aarch64
# onnxruntime wanting a newer libstdc++) is gone with it -- tract is pure Rust and would run on
# anything. Kept because the carrier's consumers are trixie-based and moving the base is a change
# they would have to make in step.
FROM rust:${RUST_VERSION}-trixie AS build

WORKDIR /src/cli
COPY . .

RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/src/cli/target,sharing=locked \
    cargo build --release --locked --bin tinybrains \
 && cp target/release/tinybrains /usr/local/bin/tinybrains

# ---- the carrier -------------------------------------------------------------
#
# The binary alone, so a consumer does `COPY --from=cli /artifacts/bin/tinybrains /usr/local/bin/`
# and needs nothing else.
FROM busybox:${BUSYBOX_VERSION}
LABEL org.opencontainers.image.title="tinybrains CLI" \
      org.opencontainers.image.source="https://github.com/Tiny-Brains/devops" \
      org.opencontainers.image.description="one binary that plays a match on a laptop: the cartridge through wasmtime, the manifest's adapters through datalogic, the graph through tract"

COPY --from=build /usr/local/bin/tinybrains /artifacts/bin/tinybrains

CMD ["sh", "-c", "cp -a /artifacts/. /out/"]
