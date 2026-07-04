# Signed supply chain (D3)

The appliance ships with a verifiable supply chain: every image is **cosign-signed**,
carries an **SBOM** (syft), is built from a **minimal, non-root** base, and makes
**zero phone-home**. This document is the posture + how to verify it.

The pipeline is deliberately **offline-honest**: the scripts and the CI lane below
produce and verify these artifacts, but the *signing key* and a *real registry* are
owner-gated (a signing key is a Tier-0 secret, CANON II.8 / doctrine I-7). Nothing
here contacts a third party at build time beyond the base-image pulls.

## 1. What is signed / attested

| Image | Base | Contents | Signed | SBOM |
|---|---|---|---|---|
| `im-appliance` | `debian:bookworm-slim` (non-root `appliance` uid 10001) | `wsf-api`, `wsf-seed`, `aog-gateway` release binaries + `ca-certificates` | cosign | syft SPDX + CycloneDX |
| `im-console` | `nginx:1.27-alpine` | the built Vite SPA + `nginx.conf` (same-origin reverse proxy) | cosign | syft SPDX + CycloneDX |

Base images are minimal by design. `distroless/cc-debian12:nonroot` is the further
hardening for the Rust image (glibc + libssl + ca-certificates, no shell/apt); the
stanza is in §5. It is **not** the default until a build proves it on target hardware
(the current `debian-slim` runtime is the D1b-proven path).

## 2. Verify a release (consumer-facing)

```bash
# 1. Signature — the image was signed by Island Mountain's key.
cosign verify --key deployment/supply-chain/cosign.pub im-appliance:vX.Y.Z

# 2. SBOM — the bill of materials is present and attached.
cosign download sbom im-appliance:vX.Y.Z        # or read deployment/supply-chain/sbom/*.spdx.json

# 3. Zero phone-home — the running appliance makes no unexpected egress.
bash deployment/supply-chain/no-phone-home.sh   # static; runtime monitor in §4
```

All three are the D3 gate: **signatures verify · SBOM present · egress monitor shows
no outbound telemetry.**

## 3. Produce the artifacts (release-time)

```bash
# Build the images (10–30 min; owner-gated Docker build).
docker compose -f deployment/appliance/docker-compose.yml build

# SBOM per image (syft):
bash deployment/supply-chain/sbom.sh im-appliance:vX.Y.Z im-console:vX.Y.Z

# Sign + verify (cosign):
#   CI: keyless OIDC (no key material).  Local: COSIGN_KEY=<key> (owner-gated).
COSIGN_KEY=cosign.key bash deployment/supply-chain/sign.sh im-appliance:vX.Y.Z im-console:vX.Y.Z
```

## 4. Zero phone-home

Two layers, defence in depth:

- **Static (enforced, offline).** "Zero phone-home" is not "no hostnames in source" —
  the services must reach a customer's *configured* OpenBao / STS / provider when not
  air-gapped. `no-phone-home.sh` enforces the real property: **(a)** no call-home to
  our own domain and no telemetry/analytics SDK (sentry, segment, mixpanel, posthog,
  datadog, GA…), and **(b)** every external host in shipped `src/` is a known,
  env/config-overridable provider/STS endpoint (openai/anthropic/amazonaws/
  microsoftonline/googleapis/azure) — a new public host must be added deliberately.
  Runs clean today; wired into CI so a regression (a surprise host, a telemetry dep)
  fails the build.
- **Runtime (owner-gated).** Bring the appliance up on an isolated bridge network and
  watch it serve a governed `vk_demo` completion while a monitor records egress:

  ```bash
  docker network create --internal im-airgap        # no NAT to the host/internet
  # attach the appliance services to im-airgap, then:
  docker run --rm --net container:aog-gateway nicolaka/netshoot \
    timeout 60 tcpdump -n 'tcp and not (dst net 172.16.0.0/12 or 127.0.0.0/8)'
  # expected: zero packets to any non-cluster address.
  ```

  In air-gap mode the WSF egress guard (W5 / doctrine I-8) additionally denies every
  cloud route at the policy layer, so even a mis-set provider URL cannot leak.

## 5. Distroless hardening (owner-gated option)

Swap the appliance runtime stage to distroless and drop the apt/useradd steps:

```dockerfile
FROM gcr.io/distroless/cc-debian12:nonroot AS runtime
COPY --from=builder /build/target/release/wsf-api     /usr/local/bin/wsf-api
COPY --from=builder /build/target/release/wsf-seed    /usr/local/bin/wsf-seed
COPY --from=builder /build/target/release/aog-gateway /usr/local/bin/aog-gateway
USER nonroot
CMD ["/usr/local/bin/wsf-api", "serve"]
```

`distroless/cc-debian12` is bookworm-based (glibc 2.36, matching the `rust:1-bookworm`
build) and ships `ca-certificates` + a `nonroot` (uid 65532) user. Verify the build on
target hardware before switching the default.

## 6. Status

- `.dockerignore` — present; excludes `target/`, `node_modules/`, `.env`, `.git/`,
  `docs/`, caches (no secrets or build cruft in the context). ✓
- Base images — `debian:bookworm-slim` (non-root) + `nginx:1.27-alpine`. ✓
- SBOM — `sbom.sh` (syft SPDX + CycloneDX per image). Owner runs at release.
- Signing — `sign.sh` (cosign keyless in CI / key-based owner-gated). Key is Tier-0.
- Phone-home — static check **clean + CI-gated**; runtime monitor owner-gated.
- Distroless — documented option (§5); default stays the D1b-proven slim base.
