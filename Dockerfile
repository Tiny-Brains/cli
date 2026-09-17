# syntax=docker/dockerfile:1

# The `tinybrains` CLI as an ARTIFACT IMAGE: one binary that plays a match on a laptop.
#
# WHY THIS EXISTS. `web/docs` regenerates the book's lesson replays with this binary inside a Docker
# build, and takes it from this image (`CLI_REF`) rather than compiling Rust there. A person wants
# the release instead -- Homebrew or a GitHub release archive, see README.md -- which carries the
# same binary for macOS and Linux with no Docker at all.
#
# IT LINKS NO SIBLING. The crate links the two libraries a node itself links -- datalogic for the
# adapters, tract for the graph -- both from crates.io, so this is an ordinary build of this
# repository alone, and `cargo install --git` works for anyone.

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
      org.opencontainers.image.source="https://github.com/Tiny-Brains/cli" \
      org.opencontainers.image.description="one binary that plays a match on a laptop: the cartridge through wasmtime, the manifest's adapters through datalogic, the graph through tract"

COPY --from=build /usr/local/bin/tinybrains /artifacts/bin/tinybrains

CMD ["sh", "-c", "cp -a /artifacts/. /out/"]
