# syntax=docker/dockerfile:1
#
# Loom Phase-V conformance harness image (VH4) — builds the aogd control-plane
# daemon and the aog-noded edge daemon from the Cargo workspace and ships them on
# a slim runtime. Build context = the mai workspace root (so Cargo.lock resolves);
# target/ is .dockerignore'd. One image, one binary per compose service (VH5).

FROM rust:1-bookworm AS builder
WORKDIR /build
COPY . .
# Cache the crate registry + target dir across builds, then copy the two binaries
# OUT of the cached target into /out — a cache mount is not part of the image
# layer, so the runtime stage can only pick them up from a regular path.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    cargo build --release -p aogd -p aog-noded --bins \
    && mkdir -p /out \
    && cp target/release/aogd      /out/aogd \
    && cp target/release/aog-noded /out/aog-noded

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /out/aogd      /usr/local/bin/aogd
COPY --from=builder /out/aog-noded /usr/local/bin/aog-noded
RUN useradd -r -u 10001 loom
USER loom
# No ENTRYPOINT: the harness compose sets the command per service (aogd for the
# control plane, aog-noded for the edges). Default to the control-plane daemon.
CMD ["/usr/local/bin/aogd"]
