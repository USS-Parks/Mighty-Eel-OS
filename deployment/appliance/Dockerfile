# syntax=docker/dockerfile:1
#
# AOG/WSF appliance image — builds the wsf-api, wsf-seed, and aog-gateway
# binaries and ships them on a slim runtime. Build context = the mai workspace
# root (so the Cargo workspace + Cargo.lock resolve). target/ is .dockerignore'd.

FROM rust:1-bookworm AS builder
WORKDIR /build
COPY . .
RUN cargo build --release -p wsf-api -p aog-gateway --bins

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/wsf-api    /usr/local/bin/wsf-api
COPY --from=builder /build/target/release/wsf-seed   /usr/local/bin/wsf-seed
COPY --from=builder /build/target/release/aog-gateway /usr/local/bin/aog-gateway
# Non-root by default; the one-shot seed service overrides to root so it can
# write the shared /seed volume.
RUN useradd -r -u 10001 appliance
USER appliance
# No ENTRYPOINT: compose fully replaces this command per service (the services
# source /seed/appliance.env via a sh wrapper before exec).
CMD ["/usr/local/bin/wsf-api", "serve"]
